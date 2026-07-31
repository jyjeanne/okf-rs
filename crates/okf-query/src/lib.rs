//! Shared query layer wrapping `okf-search`/`okf-graph`/
//! `okf_parser::read_bundle`, so `okf-cli` and `okf-mcp` express the same
//! operations exactly once — same bundle-loading, same "unknown concept
//! id" check, same result text — instead of two independently maintained
//! copies that can silently drift. Each caller decides how to surface the
//! `Result` this returns: `okf-cli` prints the `Ok` text and exits
//! non-zero on `Err`; `okf-mcp` wraps either into an MCP tool response.
//!
//! # Stability
//!
//! Public, documented, and versioned together with the rest of the
//! `okf-rs` workspace, the same as `okf-graph` (see that crate's own
//! `# Stability` section) — not just an internal detail of
//! `okf-cli`/`okf-mcp`. Most of these functions (`search`,
//! `graph_callers`, ...) return pre-formatted text, which is the right
//! shape for a CLI/MCP caller but not for a Rust tool that wants to work
//! with the data directly. The two operations that compute real
//! aggregated data rather than just relaying `okf-graph`/`okf-search`
//! results — [`coverage`] and [`graph_stats`] — additionally expose that
//! data as a plain struct (`Serialize`, so it's JSON-embeddable too) via
//! [`coverage_report`] and [`graph_stats_report`]; everywhere else, embed
//! `okf_graph::Graph`/`okf_search::SearchIndex`/`FullTextIndex` directly,
//! which already return structured, borrowed data rather than text.
//!
//! # Example
//!
//! ```
//! # use std::fs;
//! # let dir = std::env::temp_dir().join(format!("okf-query-doctest-{}", std::process::id()));
//! # let _ = fs::remove_dir_all(&dir);
//! # fs::create_dir_all(dir.join("functions")).unwrap();
//! # fs::write(
//! #     dir.join("functions/f.md"),
//! #     "---\ntype: Rust Function\ntitle: f\nresource: src/lib.rs#L1\n---\n\nbody\n",
//! # ).unwrap();
//! // A Rust tool embedding okf-query directly gets a typed report, not
//! // text to re-parse.
//! let report = okf_query::coverage_report(&dir)?;
//! assert_eq!(report.total_concepts, 1);
//! # fs::remove_dir_all(&dir).unwrap();
//! # Ok::<(), anyhow::Error>(())
//! ```

#![deny(missing_docs)]

use anyhow::{anyhow, Result};
use okf_parser::{Concept, ConceptKind, RelationKind};
use serde::Serialize;
use std::collections::BTreeMap;
use std::fmt;
use std::path::Path;

/// Errors if `bundle` doesn't exist yet, with a pointer to `okf-rs
/// generate` rather than an opaque "no such file or directory" error.
pub fn require_bundle(bundle: &Path) -> Result<()> {
    if bundle.is_dir() {
        Ok(())
    } else {
        Err(anyhow!(
            "no bundle found at {} — run `okf-rs generate` first",
            bundle.display()
        ))
    }
}

/// Reads a bundle's concepts (relationships included) back off disk,
/// after confirming it exists (see [`require_bundle`]).
pub fn load_concepts(bundle: &Path) -> Result<Vec<Concept>> {
    require_bundle(bundle)?;
    okf_parser::read_bundle(bundle)
}

fn require_concept(graph: &okf_graph::Graph<'_>, id: &str) -> Result<()> {
    if graph.get(id).is_some() {
        Ok(())
    } else {
        Err(anyhow!(
            "no concept with id `{id}` (use `okf-rs search`/the `search` tool to find valid ids)"
        ))
    }
}

/// Free-text search over the bundle by symbol, package, module, type, or
/// tag.
pub fn search(bundle: &Path, query: &str) -> Result<String> {
    require_bundle(bundle)?;
    let index = okf_search::SearchIndex::build(bundle)?;
    let hits = index.search(query);
    if hits.is_empty() {
        return Ok(format!("No matches for `{query}`"));
    }
    Ok(hits
        .iter()
        .map(|hit| {
            format!(
                "{:>3}  {:<24} {:<20} {}",
                hit.score, hit.entry.title, hit.entry.concept_type, hit.entry.id
            )
        })
        .collect::<Vec<_>>()
        .join("\n"))
}

/// Ranked, relevance-scored full-text search over the bundle's title,
/// type, description, signature, and tags (via `okf_search::FullTextIndex`,
/// backed by Tantivy). Distinct from [`search`]'s exact/substring matching:
/// this also searches description/signature prose and orders results by
/// relevance rather than a fixed field-priority score, so a
/// natural-language query (e.g. "parses a jwt") can surface a concept
/// whose *description* mentions it even when the query matches no title,
/// type, or tag at all.
pub fn search_ranked(bundle: &Path, query: &str) -> Result<String> {
    require_bundle(bundle)?;
    let index = okf_search::FullTextIndex::build(bundle)?;
    let hits = index.search(query, 25)?;
    if hits.is_empty() {
        return Ok(format!("No matches for `{query}`"));
    }
    Ok(hits
        .iter()
        .map(|hit| {
            format!(
                "{:>6.2}  {:<24} {:<20} {}",
                hit.score, hit.title, hit.concept_type, hit.id
            )
        })
        .collect::<Vec<_>>()
        .join("\n"))
}

/// Concepts that directly call `id`.
pub fn graph_callers(bundle: &Path, id: &str) -> Result<String> {
    let concepts = load_concepts(bundle)?;
    let graph = okf_graph::Graph::build(&concepts);
    require_concept(&graph, id)?;
    let callers = graph.callers(id);
    if callers.is_empty() {
        return Ok(format!("No callers found for `{id}`"));
    }
    Ok(concept_lines(&callers))
}

/// Concepts `id` directly calls.
pub fn graph_callees(bundle: &Path, id: &str) -> Result<String> {
    let concepts = load_concepts(bundle)?;
    let graph = okf_graph::Graph::build(&concepts);
    require_concept(&graph, id)?;
    let callees = graph.callees(id);
    if callees.is_empty() {
        return Ok(format!(
            "`{id}` doesn't call anything (or only calls unresolved/ambiguous targets)"
        ));
    }
    Ok(concept_lines(&callees))
}

/// The project's public API surface (public functions/methods/types).
pub fn graph_api(bundle: &Path) -> Result<String> {
    let concepts = load_concepts(bundle)?;
    let graph = okf_graph::Graph::build(&concepts);
    let api = graph.public_api();
    if api.is_empty() {
        return Ok("No public concepts found".to_string());
    }
    let mut out = format!("{} public concepts:\n", api.len());
    out.push_str(
        &api.iter()
            .map(|c| format!("  {:<12} {}", c.frontmatter_type(), c.id))
            .collect::<Vec<_>>()
            .join("\n"),
    );
    Ok(out)
}

/// Groups of concepts that call each other in a cycle.
pub fn graph_cycles(bundle: &Path) -> Result<String> {
    let concepts = load_concepts(bundle)?;
    let graph = okf_graph::Graph::build(&concepts);
    let cycles = graph.cycles();
    if cycles.is_empty() {
        return Ok("No cycles found in the call graph".to_string());
    }
    Ok(cycles
        .into_iter()
        .map(|cycle| cycle.join(" -> "))
        .collect::<Vec<_>>()
        .join("\n"))
}

/// Concepts with no `Calls`/`CalledBy` edge in either direction (never
/// observed calling anything, and never observed being called).
pub fn graph_isolated(bundle: &Path) -> Result<String> {
    let concepts = load_concepts(bundle)?;
    let graph = okf_graph::Graph::build(&concepts);
    let isolated = graph.isolated_concepts();
    if isolated.is_empty() {
        return Ok("No isolated concepts found".to_string());
    }
    Ok(concept_lines(&isolated))
}

/// Bundle-wide content-completeness metrics, as a plain struct rather
/// than the pre-formatted text [`coverage`] returns — for a Rust tool
/// that wants the numbers directly (e.g. to serve as JSON) instead of
/// re-parsing a report meant for a terminal. `Serialize`, so it can be
/// handed straight to `serde_json`/an HTTP response without another
/// conversion step.
///
/// The call-graph-participation metric excludes `Module`/`Package`
/// concepts from its denominator, matching `Graph::isolated_concepts`'s
/// own exclusion: they're structural containers that are never expected
/// to carry a `Calls`/`CalledBy` edge, so counting them would understate
/// coverage for reasons that have nothing to do with documentation or
/// analysis quality.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CoverageReport {
    /// Total number of concepts in the bundle.
    pub total_concepts: usize,
    /// How many concepts have a non-empty `description`.
    pub with_description: usize,
    /// How many concepts have at least one tag.
    pub with_tags: usize,
    /// Call-graph participation, or `None` when there are no non-
    /// `Module`/`Package` concepts to measure it against (the "N/A" case
    /// in [`coverage`]'s rendered text) — a bundle made only of
    /// structural containers has nothing this metric could report on.
    pub graph_participation: Option<GraphParticipation>,
}

/// How much of a bundle's call-graph-eligible concepts (everything
/// except `Module`/`Package`) actually participate in the `Calls`/
/// `CalledBy` graph — see [`CoverageReport::graph_participation`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct GraphParticipation {
    /// Concepts with at least one `Calls`/`CalledBy` edge.
    pub connected: usize,
    /// Concepts eligible to participate at all (excludes `Module`/`Package`).
    pub eligible: usize,
}

impl fmt::Display for CoverageReport {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.total_concepts == 0 {
            return write!(f, "Bundle has no concepts");
        }
        let graph_line = match self.graph_participation {
            None => "  N/A (no non-Module/Package concepts to measure) participate in the call graph"
                .to_string(),
            Some(GraphParticipation { connected, eligible }) => format!(
                "  {}% ({connected}/{eligible}) participate in the call graph (excludes Module/Package; see `graph isolated` for the rest)",
                percent(connected, eligible)
            ),
        };
        write!(
            f,
            "{} concepts\n  {}% ({}/{}) have a description\n  {}% ({}/{}) have at least one tag\n{graph_line}",
            self.total_concepts,
            percent(self.with_description, self.total_concepts),
            self.with_description,
            self.total_concepts,
            percent(self.with_tags, self.total_concepts),
            self.with_tags,
            self.total_concepts,
        )
    }
}

/// Bundle-wide content-completeness metrics: how much of the knowledge
/// base is actually filled in, as distinct from `okf-rs validate`'s
/// pass/fail structural checks. Computed entirely from the bundle
/// already on disk (via [`load_concepts`]/[`okf_graph::Graph`]) — no
/// re-scan of the source project, so there's no second source of truth
/// to keep in sync. See [`coverage_report`] for the same data as a typed
/// struct instead of this rendered text.
pub fn coverage(bundle: &Path) -> Result<String> {
    Ok(coverage_report(bundle)?.to_string())
}

/// The data behind [`coverage`], as a [`CoverageReport`] instead of text.
pub fn coverage_report(bundle: &Path) -> Result<CoverageReport> {
    let concepts = load_concepts(bundle)?;
    let total_concepts = concepts.len();
    let with_description = concepts
        .iter()
        .filter(|c| {
            c.description
                .as_deref()
                .is_some_and(|d| !d.trim().is_empty())
        })
        .count();
    let with_tags = concepts.iter().filter(|c| !c.tags.is_empty()).count();

    let graph = okf_graph::Graph::build(&concepts);
    let isolated: std::collections::HashSet<&str> = graph
        .isolated_concepts()
        .iter()
        .map(|c| c.id.as_str())
        .collect();
    let eligible = concepts
        .iter()
        .filter(|c| !matches!(c.kind, ConceptKind::Module | ConceptKind::Package))
        .count();
    let connected = concepts
        .iter()
        .filter(|c| !matches!(c.kind, ConceptKind::Module | ConceptKind::Package))
        .filter(|c| !isolated.contains(c.id.as_str()))
        .count();

    let graph_participation = if eligible == 0 {
        None
    } else {
        Some(GraphParticipation { connected, eligible })
    };

    Ok(CoverageReport {
        total_concepts,
        with_description,
        with_tags,
        graph_participation,
    })
}

fn percent(part: usize, total: usize) -> usize {
    (part * 100).checked_div(total).unwrap_or(0)
}

/// Cross-module call dependency edges: which modules call into which.
pub fn graph_modules(bundle: &Path) -> Result<String> {
    let concepts = load_concepts(bundle)?;
    let graph = okf_graph::Graph::build(&concepts);
    let deps = graph.module_dependencies();
    if deps.is_empty() {
        return Ok("No cross-module call dependencies found".to_string());
    }
    Ok(deps
        .into_iter()
        .map(|(from, to)| format!("{from} -> {to}"))
        .collect::<Vec<_>>()
        .join("\n"))
}

/// Bundle-wide graph topology metrics, as a plain struct rather than the
/// pre-formatted text [`graph_stats`] returns — see [`CoverageReport`]
/// for why. Edges are counted per kind rather than as a single total, to
/// sidestep the ambiguous question of whether a resolved `Calls`/
/// `CalledBy` pair is one edge or two.
///
/// Deliberately doesn't report a "depth" metric: the call graph isn't
/// guaranteed acyclic (see `graph_cycles`), so there's no single
/// well-defined notion of depth to report without first committing to a
/// much narrower definition than the word implies.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct GraphStatsReport {
    /// Total number of concepts in the bundle.
    pub total_concepts: usize,
    /// Concept count by kind (`Function`, `Struct`, `Module`, ...).
    pub by_kind: BTreeMap<ConceptKind, usize>,
    /// Relationship edge count by kind (`Calls`, `Imports`, ...).
    pub by_relation: BTreeMap<RelationKind, usize>,
    /// Connected components of the undirected `Calls`/`CalledBy` graph
    /// (see `Graph::connected_components`), each as a sorted list of
    /// concept ids.
    pub components: Vec<Vec<String>>,
    /// Concepts with no `Calls`/`CalledBy` edge in either direction.
    pub isolated_count: usize,
}

impl fmt::Display for GraphStatsReport {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut out = format!("{} concepts\n\nBy kind:\n", self.total_concepts);
        for (kind, count) in &self.by_kind {
            out.push_str(&format!("  {:<12} {count}\n", kind.as_str()));
        }

        out.push_str("\nRelationship edges by kind:\n");
        if self.by_relation.is_empty() {
            out.push_str("  (none)\n");
        } else {
            for (kind, count) in &self.by_relation {
                out.push_str(&format!("  {:<12} {count}\n", kind.label()));
            }
        }

        out.push_str(&format!(
            "\nCall graph: {} connected component(s) with at least one Calls/CalledBy edge (sizes shown below — a lone self-recursive concept forms its own size-1 component), {} isolated concept(s) with no edge at all (see `graph isolated`)\n",
            self.components.len(),
            self.isolated_count
        ));
        for component in &self.components {
            out.push_str(&format!(
                "  [{}] {}\n",
                component.len(),
                component.join(", ")
            ));
        }

        f.write_str(out.trim_end())
    }
}

/// Bundle-wide graph topology metrics: concept-kind breakdown, edge
/// counts per relationship kind, and connected components of the
/// `Calls`/`CalledBy` graph. See [`graph_stats_report`] for the same
/// data as a typed struct instead of this rendered text.
pub fn graph_stats(bundle: &Path) -> Result<String> {
    Ok(graph_stats_report(bundle)?.to_string())
}

/// The data behind [`graph_stats`], as a [`GraphStatsReport`] instead of
/// text.
pub fn graph_stats_report(bundle: &Path) -> Result<GraphStatsReport> {
    let concepts = load_concepts(bundle)?;
    let graph = okf_graph::Graph::build(&concepts);

    let mut by_kind: BTreeMap<ConceptKind, usize> = Default::default();
    for c in &concepts {
        *by_kind.entry(c.kind).or_default() += 1;
    }

    let mut by_relation: BTreeMap<RelationKind, usize> = Default::default();
    for c in &concepts {
        for rel in &c.relationships {
            *by_relation.entry(rel.kind).or_default() += 1;
        }
    }

    let components = graph
        .connected_components()
        .into_iter()
        .map(|component| component.into_iter().map(str::to_string).collect())
        .collect();
    let isolated_count = graph.isolated_concepts().len();

    Ok(GraphStatsReport {
        total_concepts: concepts.len(),
        by_kind,
        by_relation,
        components,
        isolated_count,
    })
}

/// The shortest call path between two concept ids.
pub fn graph_path(bundle: &Path, from: &str, to: &str) -> Result<String> {
    let concepts = load_concepts(bundle)?;
    let graph = okf_graph::Graph::build(&concepts);
    require_concept(&graph, from)?;
    require_concept(&graph, to)?;
    match graph.shortest_call_path(from, to) {
        Some(steps) => Ok(steps.join(" -> ")),
        None => Ok(format!("No call path found from `{from}` to `{to}`")),
    }
}

fn concept_lines(concepts: &[&Concept]) -> String {
    concepts
        .iter()
        .map(|c| format!("{} — {}", c.id, c.frontmatter_type()))
        .collect::<Vec<_>>()
        .join("\n")
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

    fn sample_bundle() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        write(
            dir.path(),
            "functions/auth/verify_token.md",
            "---\ntype: Rust Function\ntitle: verify_token\nresource: src/auth.rs#L1\nrelationships:\n  calls:\n    - functions/auth/decode_jwt\n---\n\nbody\n",
        );
        write(
            dir.path(),
            "functions/auth/decode_jwt.md",
            "---\ntype: Rust Function\ntitle: decode_jwt\nresource: src/auth.rs#L5\nrelationships:\n  called_by:\n    - functions/auth/verify_token\n---\n\nbody\n",
        );
        dir
    }

    #[test]
    fn search_finds_a_concept_by_title() {
        let dir = sample_bundle();
        let text = search(dir.path(), "decode_jwt").unwrap();
        assert!(text.contains("decode_jwt"));
        assert!(text.contains("functions/auth/decode_jwt"));
    }

    #[test]
    fn search_ranked_finds_a_concept_by_description_text() {
        let dir = sample_bundle();
        write(
            dir.path(),
            "functions/auth/other.md",
            "---\ntype: Rust Function\ntitle: other\ndescription: Verifies the signature on a JSON Web Token.\nresource: src/auth.rs#L20\n---\n\nbody\n",
        );

        let text = search_ranked(dir.path(), "signature").unwrap();
        assert!(text.contains("functions/auth/other"));
    }

    #[test]
    fn coverage_report_exposes_typed_fields_matching_the_rendered_text() {
        let dir = sample_bundle();
        let report = coverage_report(dir.path()).unwrap();
        assert_eq!(report.total_concepts, 2);
        assert_eq!(report.with_description, 0);
        assert_eq!(report.with_tags, 0);
        assert_eq!(
            report.graph_participation,
            Some(GraphParticipation {
                connected: 2,
                eligible: 2
            })
        );
        // The struct and the text `coverage()` builds from it must agree.
        assert_eq!(report.to_string(), coverage(dir.path()).unwrap());
    }

    #[test]
    fn coverage_report_serializes_to_json_for_an_embedding_tool() {
        let dir = sample_bundle();
        let report = coverage_report(dir.path()).unwrap();
        let json = serde_json::to_value(&report).unwrap();
        assert_eq!(json["total_concepts"], 2);
        assert_eq!(json["graph_participation"]["connected"], 2);
    }

    #[test]
    fn graph_stats_report_exposes_typed_fields_matching_the_rendered_text() {
        let dir = sample_bundle();
        let report = graph_stats_report(dir.path()).unwrap();
        assert_eq!(report.total_concepts, 2);
        assert_eq!(report.by_kind.get(&ConceptKind::Function), Some(&2));
        assert_eq!(report.by_relation.get(&RelationKind::Calls), Some(&1));
        assert_eq!(report.isolated_count, 0);
        assert_eq!(report.components.len(), 1);
        assert_eq!(report.to_string(), graph_stats(dir.path()).unwrap());
    }

    #[test]
    fn graph_stats_report_serializes_to_json_for_an_embedding_tool() {
        let dir = sample_bundle();
        let report = graph_stats_report(dir.path()).unwrap();
        let json = serde_json::to_value(&report).unwrap();
        assert_eq!(json["total_concepts"], 2);
        assert_eq!(json["isolated_count"], 0);
    }

    #[test]
    fn search_ranked_missing_bundle_points_at_generate() {
        let err = search_ranked(Path::new("/nonexistent"), "x").unwrap_err();
        assert!(err.to_string().contains("okf-rs generate"));
    }

    #[test]
    fn missing_bundle_points_at_generate() {
        let err = search(Path::new("/nonexistent"), "x").unwrap_err();
        assert!(err.to_string().contains("okf-rs generate"));
    }

    #[test]
    fn graph_callers_and_callees_round_trip() {
        let dir = sample_bundle();
        let callers = graph_callers(dir.path(), "functions/auth/decode_jwt").unwrap();
        assert!(callers.contains("functions/auth/verify_token"));

        let callees = graph_callees(dir.path(), "functions/auth/verify_token").unwrap();
        assert!(callees.contains("functions/auth/decode_jwt"));
    }

    #[test]
    fn graph_path_finds_the_direct_edge() {
        let dir = sample_bundle();
        let text = graph_path(
            dir.path(),
            "functions/auth/verify_token",
            "functions/auth/decode_jwt",
        )
        .unwrap();
        assert_eq!(
            text,
            "functions/auth/verify_token -> functions/auth/decode_jwt"
        );
    }

    #[test]
    fn unknown_concept_id_is_a_clear_error() {
        let dir = sample_bundle();
        let err = graph_callers(dir.path(), "functions/nope").unwrap_err();
        assert!(err.to_string().contains("no concept with id"));
    }

    #[test]
    fn graph_api_reports_public_concepts() {
        let dir = sample_bundle();
        let text = graph_api(dir.path()).unwrap();
        assert!(text.contains("2 public concepts:"));
        assert!(text.contains("functions/auth/verify_token"));
    }

    #[test]
    fn coverage_reports_zero_description_and_tag_coverage_but_full_graph_participation() {
        let dir = sample_bundle();
        let text = coverage(dir.path()).unwrap();
        assert!(text.starts_with("2 concepts"));
        assert!(text.contains("0% (0/2) have a description"));
        assert!(text.contains("0% (0/2) have at least one tag"));
        assert!(text.contains("100% (2/2) participate in the call graph"));
    }

    #[test]
    fn coverage_counts_descriptions_tags_and_isolated_concepts() {
        let dir = sample_bundle();
        write(
            dir.path(),
            "functions/auth/unused.md",
            "---\ntype: Rust Function\ntitle: unused\ndescription: Dead code, never called.\ntags: [dead-code]\nresource: src/auth.rs#L20\n---\n\nbody\n",
        );

        let text = coverage(dir.path()).unwrap();
        assert!(text.starts_with("3 concepts"));
        assert!(text.contains("33% (1/3) have a description"));
        assert!(text.contains("33% (1/3) have at least one tag"));
        assert!(text.contains("66% (2/3) participate in the call graph"));
    }

    #[test]
    fn coverage_reports_not_applicable_when_no_concept_is_call_graph_eligible() {
        let dir = tempfile::tempdir().unwrap();
        write(
            dir.path(),
            "packages/demo.md",
            "---\ntype: Rust Package\ntitle: demo\nresource: Cargo.toml\n---\n\nbody\n",
        );
        write(
            dir.path(),
            "modules/lib.md",
            "---\ntype: Rust Module\ntitle: lib\nresource: src/lib.rs#L1\n---\n\nbody\n",
        );

        let text = coverage(dir.path()).unwrap();
        assert!(text.starts_with("2 concepts"));
        assert!(
            text.contains("N/A (no non-Module/Package concepts to measure) participate in the call graph"),
            "a bundle with nothing but Module/Package concepts should report N/A, not a misleading 0%: {text}"
        );
        assert!(!text.contains("0% (0/0)"));
    }

    #[test]
    fn graph_stats_reports_kind_and_relation_counts_and_the_one_component() {
        let dir = sample_bundle();
        let text = graph_stats(dir.path()).unwrap();
        assert!(text.starts_with("2 concepts"));
        assert!(text.contains("Function     2"));
        assert!(text.contains("Calls        1"));
        assert!(text.contains("Called by    1"));
        assert!(text.contains("1 connected component(s) with at least one Calls/CalledBy edge"));
        assert!(text.contains("0 isolated concept(s) with no edge at all"));
        assert!(text.contains("functions/auth/decode_jwt, functions/auth/verify_token"));
    }

    #[test]
    fn graph_stats_counts_an_isolated_concept_separately_from_components() {
        let dir = sample_bundle();
        write(
            dir.path(),
            "functions/auth/unused.md",
            "---\ntype: Rust Function\ntitle: unused\nresource: src/auth.rs#L20\n---\n\nbody\n",
        );

        let text = graph_stats(dir.path()).unwrap();
        assert!(text.starts_with("3 concepts"));
        assert!(text.contains("1 connected component(s) with at least one Calls/CalledBy edge"));
        assert!(text.contains("1 isolated concept(s) with no edge at all"));
    }

    #[test]
    fn graph_stats_counts_a_self_recursive_concept_as_its_own_component_not_isolated() {
        let dir = sample_bundle();
        write(
            dir.path(),
            "functions/auth/recursive.md",
            "---\ntype: Rust Function\ntitle: recursive\nresource: src/auth.rs#L30\nrelationships:\n  calls:\n    - functions/auth/recursive\n  called_by:\n    - functions/auth/recursive\n---\n\nbody\n",
        );

        let text = graph_stats(dir.path()).unwrap();
        assert!(text.starts_with("3 concepts"));
        // The self-recursive concept and the original a<->b pair are two
        // separate components — the self-loop must not be dropped as a
        // singleton, nor merged into the unrelated pair, nor counted as
        // isolated (it has a real Calls/CalledBy edge).
        assert!(text.contains("2 connected component(s) with at least one Calls/CalledBy edge"));
        assert!(text.contains("0 isolated concept(s) with no edge at all"));
        assert!(text.contains("[1] functions/auth/recursive"));
    }

    #[test]
    fn graph_cycles_and_modules_report_none_found() {
        let dir = sample_bundle();
        assert_eq!(
            graph_cycles(dir.path()).unwrap(),
            "No cycles found in the call graph"
        );
        // Both concepts live in the same module (src/auth.rs), so there
        // are no cross-module edges to report.
        assert_eq!(
            graph_modules(dir.path()).unwrap(),
            "No cross-module call dependencies found"
        );
    }

    #[test]
    fn graph_isolated_reports_a_concept_with_no_call_edges() {
        let dir = sample_bundle();
        write(
            dir.path(),
            "functions/auth/unused.md",
            "---\ntype: Rust Function\ntitle: unused\nresource: src/auth.rs#L20\n---\n\nbody\n",
        );

        let text = graph_isolated(dir.path()).unwrap();
        assert!(text.contains("functions/auth/unused"));
        assert!(!text.contains("functions/auth/verify_token"));
    }

    #[test]
    fn graph_isolated_reports_none_found_when_fully_connected() {
        let dir = sample_bundle();
        assert_eq!(
            graph_isolated(dir.path()).unwrap(),
            "No isolated concepts found"
        );
    }
}
