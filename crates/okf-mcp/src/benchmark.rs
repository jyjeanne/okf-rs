//! `okf-mcp --benchmark`: a local, offline session-level cost report,
//! answering the "Measure session-level MCP performance" item from
//! [`ROADMAP.md`](../../../ROADMAP.md)'s AI-native platform maturity
//! improvement plan.
//!
//! Per-query token counts (this server's existing `graph_callers`-style
//! comparisons, e.g. in the README) only tell half the story: every tool
//! this server registers also contributes its JSON Schema to the system
//! prompt for the *whole session*, used or not. That's a fixed cost paid
//! once per session, set against savings that only accrue per structural
//! query asked — so whether registering this server is worth it at all
//! depends on how many structural questions a session actually asks, not
//! on the per-query ratio alone.
//!
//! This report computes both sides from data already on disk, no LLM
//! call or tokenizer dependency involved: tokens are estimated via the
//! common ~4-characters-per-token rule of thumb, the same heuristic this
//! project's README already uses for its one hand-picked worked example
//! (`cmd_generate` → `run`). What it does *not* attempt is a cost
//! comparison against RAG-based retrieval — that needs a real
//! vector-store/chunking/re-ranking baseline to be a fair number, not
//! something to fabricate here; it's left as an explicit, documented gap
//! (see `ROADMAP.md`).

use crate::tools;
use anyhow::{Context, Result};
use okf_core::config::resolve_bundle;
use okf_parser::{Concept, ConceptKind};
use std::fmt::Write as _;
use std::path::Path;

/// The common "roughly 4 characters per token" estimate this project's
/// own README already uses for its hand-picked worked example — good
/// enough for an order-of-magnitude comparison, not a real tokenizer.
const CHARS_PER_TOKEN: usize = 4;

fn approx_tokens(bytes: usize) -> usize {
    bytes / CHARS_PER_TOKEN
}

/// One sampled concept's "grep-and-read by hand" cost against this
/// server's one-tool-call cost for the same "who calls this?" question.
pub struct QueryCost {
    pub concept_id: String,
    pub naive_files: usize,
    pub naive_tokens: usize,
    pub mcp_tokens: usize,
}

impl QueryCost {
    /// Positive: the MCP call was cheaper. Negative (possible on a
    /// concept whose name matches inside a handful of tiny files) means
    /// the naive path happened to be cheaper for this one sample —
    /// reported honestly rather than excluded, since a real session mixes
    /// both kinds of query.
    pub fn saved_tokens(&self) -> i64 {
        self.naive_tokens as i64 - self.mcp_tokens as i64
    }
}

/// The full session-level report: the fixed registration cost, plus a
/// sample of per-query comparisons it's weighed against.
pub struct Report {
    pub tool_count: usize,
    pub schema_bytes: usize,
    pub schema_tokens: usize,
    pub queries: Vec<QueryCost>,
}

impl Report {
    fn average_saved_tokens(&self) -> Option<f64> {
        if self.queries.is_empty() {
            return None;
        }
        let total: i64 = self.queries.iter().map(QueryCost::saved_tokens).sum();
        Some(total as f64 / self.queries.len() as f64)
    }

    /// How many structural queries (at the sampled average savings rate)
    /// it takes for the tokens saved to recoup the fixed schema
    /// registration cost. `None` when there's no sample to average, or
    /// the average saving isn't positive (registering this server never
    /// pays for itself against this particular sample).
    fn break_even_queries(&self) -> Option<usize> {
        let avg = self.average_saved_tokens()?;
        if avg <= 0.0 {
            return None;
        }
        Some((self.schema_tokens as f64 / avg).ceil() as usize)
    }

    /// Renders the human-readable report `okf-mcp --benchmark` prints.
    pub fn render(&self) -> String {
        let mut out = String::new();
        let _ = writeln!(out, "okf-mcp session-level cost report");
        let _ = writeln!(out, "==================================");
        let _ = writeln!(out);
        let _ = writeln!(
            out,
            "Tool registration overhead (paid once, every session, whether used or not):"
        );
        let _ = writeln!(
            out,
            "  tools/list: {} tools, {} bytes (~{} tokens by the ~4 chars/token rule of thumb)",
            self.tool_count, self.schema_bytes, self.schema_tokens
        );
        let _ = writeln!(out);

        if self.queries.is_empty() {
            let _ = writeln!(
                out,
                "No sampled queries (no Function/Method concept in this bundle has a caller) — nothing to compare against."
            );
            return out;
        }

        let _ = writeln!(
            out,
            "Per-query savings, \"who calls this?\" (grep-and-read every file containing the name, by hand, vs. one `graph` tool call):"
        );
        let _ = writeln!(
            out,
            "  (naive cost = every file with an unqualified substring match on the name project-wide — deliberately pessimistic: a short, common name like `insert` or `save` picks up unrelated call sites too, which inflates the naive number in exactly the cases where an agent would also waste tokens opening the wrong file; a rarer, more specific name gives a tighter comparison.)"
        );
        for q in &self.queries {
            let _ = writeln!(
                out,
                "  {}: naive ~{} tokens ({} file{}) vs. mcp ~{} tokens -> saved ~{} tokens",
                q.concept_id,
                q.naive_tokens,
                q.naive_files,
                if q.naive_files == 1 { "" } else { "s" },
                q.mcp_tokens,
                q.saved_tokens(),
            );
        }
        let _ = writeln!(out);

        match self.average_saved_tokens() {
            Some(avg) if avg > 0.0 => {
                let _ = writeln!(out, "Average saved per structural query: ~{avg:.0} tokens");
                if let Some(break_even) = self.break_even_queries() {
                    let _ = writeln!(
                        out,
                        "Break-even: ~{break_even} structural queries to recoup the {} schema tokens above.",
                        self.schema_tokens
                    );
                    let _ = writeln!(
                        out,
                        "  A session asking fewer structural questions than that may not be worth registering this server for; one asking more comes out ahead, and every query past break-even is pure savings."
                    );
                }
            }
            _ => {
                let _ = writeln!(
                    out,
                    "Average saved per structural query: not positive on this sample — registering this server didn't pay for itself against these queries alone."
                );
            }
        }
        let _ = writeln!(out);
        let _ = writeln!(
            out,
            "Not measured here: cost comparison against RAG-based retrieval. That needs a real vector-store/chunking/re-ranking baseline to be a fair number, not one to fabricate from the bundle alone — see the roadmap's \"Measure session-level MCP performance\" item."
        );
        out
    }
}

/// Walks `project_root` (honoring `.gitignore`, the same way
/// `okf_core::Project::load` does) and sums the byte size of every file
/// whose content contains `needle` — the same "grep for the name, then
/// open every matching file" cost a human/agent pays without structural
/// tools, made repeatable here instead of hand-computed for one example.
fn naive_grep_and_read_cost(project_root: &Path, needle: &str) -> (usize, usize) {
    let mut files = 0usize;
    let mut bytes = 0usize;
    let walker = ignore::WalkBuilder::new(project_root)
        .require_git(false)
        .build();
    for entry in walker.flatten() {
        if !entry.file_type().map(|t| t.is_file()).unwrap_or(false) {
            continue;
        }
        let Ok(content) = std::fs::read_to_string(entry.path()) else {
            continue;
        };
        if content.contains(needle) {
            files += 1;
            bytes += content.len();
        }
    }
    (files, bytes)
}

/// Deterministically samples up to `sample_size` `Function`/`Method`
/// concepts that have at least one caller (nothing to ask "who calls
/// this?" about otherwise), ordered by concept id so the report is
/// stable run to run on an unchanged bundle.
fn sample_concepts(concepts: &[Concept], sample_size: usize) -> Vec<&Concept> {
    let graph = okf_graph::Graph::build(concepts);
    let mut candidates: Vec<&Concept> = concepts
        .iter()
        .filter(|c| matches!(c.kind, ConceptKind::Function | ConceptKind::Method))
        .filter(|c| !graph.callers(&c.id).is_empty())
        .collect();
    candidates.sort_by(|a, b| a.id.cmp(&b.id));
    candidates.truncate(sample_size);
    candidates
}

/// Runs the full benchmark against `project_root`'s bundle.
pub fn run(project_root: &Path, sample_size: usize) -> Result<Report> {
    let bundle = resolve_bundle(project_root, None);
    let concepts = okf_parser::read_bundle(&bundle)
        .with_context(|| format!("reading bundle at {}", bundle.display()))?;

    let tools = tools::list();
    let schema_bytes = serde_json::to_string(&tools).map(|s| s.len()).unwrap_or(0);
    let schema_tokens = approx_tokens(schema_bytes);

    // A one-shot process (this whole command runs once and exits), so a
    // throwaway cache is fine -- it's here only because `tools::call`
    // requires one, not because there's anything to amortize within a
    // single benchmark run.
    let cache = crate::cache::BundleCache::new();
    let mut queries = Vec::new();
    for c in sample_concepts(&concepts, sample_size) {
        let (naive_files, naive_bytes) =
            naive_grep_and_read_cost(project_root, &format!("{}(", c.name));
        let mcp_text = tools::call(
            "graph",
            &serde_json::json!({ "relation": "callers", "id": c.id }),
            &bundle,
            &cache,
        )?;
        queries.push(QueryCost {
            concept_id: c.id.clone(),
            naive_files,
            naive_tokens: approx_tokens(naive_bytes),
            mcp_tokens: approx_tokens(mcp_text.len()),
        });
    }

    Ok(Report {
        tool_count: tools.len(),
        schema_bytes,
        schema_tokens,
        queries,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn write(dir: &Path, relative: &str, content: &str) {
        let path = dir.join(relative);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, content).unwrap();
    }

    /// A tiny synthetic project: a bundle under `knowledge/` (what
    /// `resolve_bundle` finds by default) describing two functions, and
    /// real source under `src/` a naive grep would actually search.
    fn sample_project() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        write(
            dir.path(),
            "knowledge/functions/auth/verify_token.md",
            "---\ntype: Rust Function\ntitle: verify_token\nresource: src/auth.rs#L3\nrelationships:\n  calls:\n    - functions/auth/decode_jwt\n---\n\nbody\n",
        );
        write(
            dir.path(),
            "knowledge/functions/auth/decode_jwt.md",
            "---\ntype: Rust Function\ntitle: decode_jwt\nresource: src/auth.rs#L1\nrelationships:\n  called_by:\n    - functions/auth/verify_token\n---\n\nbody\n",
        );
        write(
            dir.path(),
            "src/auth.rs",
            "fn decode_jwt(token: &str) -> bool { true }\n\nfn verify_token(token: &str) -> bool {\n    decode_jwt(token)\n}\n",
        );
        dir
    }

    #[test]
    fn naive_cost_finds_the_one_file_containing_a_real_call_site() {
        let dir = sample_project();
        let (files, bytes) = naive_grep_and_read_cost(dir.path(), "decode_jwt(");
        assert_eq!(files, 1);
        assert!(bytes > 0);
    }

    #[test]
    fn naive_cost_skips_gitignored_directories() {
        let dir = sample_project();
        write(dir.path(), ".gitignore", "ignored_dir/\n");
        write(
            dir.path(),
            "ignored_dir/noise.rs",
            "fn decode_jwt(x: i32) {}\n",
        );
        let (files, _) = naive_grep_and_read_cost(dir.path(), "decode_jwt(");
        // Only src/auth.rs counts -- the .gitignore'd file is skipped,
        // the same way a real grep-based search over a checked-out repo
        // wouldn't turn up build artifacts or vendored code either.
        assert_eq!(files, 1);
    }

    #[test]
    fn sample_concepts_only_picks_functions_with_a_caller() {
        let dir = sample_project();
        let bundle = resolve_bundle(dir.path(), None);
        let concepts = okf_parser::read_bundle(&bundle).unwrap();
        let sampled = sample_concepts(&concepts, 5);
        // decode_jwt has a caller (verify_token); verify_token has none
        // in this fixture -- only decode_jwt is worth a "who calls
        // this?" benchmark query.
        assert_eq!(sampled.len(), 1);
        assert_eq!(sampled[0].id, "functions/auth/decode_jwt");
    }

    #[test]
    fn sample_concepts_respects_the_size_limit() {
        let dir = sample_project();
        let bundle = resolve_bundle(dir.path(), None);
        let concepts = okf_parser::read_bundle(&bundle).unwrap();
        assert_eq!(sample_concepts(&concepts, 0).len(), 0);
    }

    #[test]
    fn run_reports_schema_overhead_and_a_positive_saving_on_the_sample() {
        let dir = sample_project();
        let report = run(dir.path(), 5).unwrap();

        assert_eq!(report.tool_count, 6);
        assert!(report.schema_tokens > 0);
        assert_eq!(report.queries.len(), 1);
        let q = &report.queries[0];
        assert_eq!(q.concept_id, "functions/auth/decode_jwt");
        // The MCP call returns one short line; the naive path re-reads
        // the whole (admittedly tiny, in this fixture) source file --
        // the MCP call is still cheaper.
        assert!(q.saved_tokens() >= 0);
    }

    #[test]
    fn render_includes_the_headline_numbers_and_the_rag_caveat() {
        let dir = sample_project();
        let report = run(dir.path(), 5).unwrap();
        let text = report.render();
        assert!(text.contains("6 tools"));
        assert!(text.contains("functions/auth/decode_jwt"));
        assert!(text.contains("Not measured here"));
        assert!(text.contains("RAG"));
    }

    #[test]
    fn render_on_an_empty_bundle_reports_no_sample_instead_of_dividing_by_zero() {
        let dir = tempfile::tempdir().unwrap();
        write(dir.path(), "knowledge/index.md", "# Index\n");
        let report = run(dir.path(), 5).unwrap();
        assert!(report.queries.is_empty());
        assert!(report.render().contains("No sampled queries"));
    }
}
