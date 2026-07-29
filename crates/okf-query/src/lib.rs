//! Shared query layer wrapping `okf-search`/`okf-graph`/
//! `okf_parser::read_bundle`, so `okf-cli` and `okf-mcp` express the same
//! seven operations (search, and `okf_graph::Graph`'s six queries) exactly
//! once — same bundle-loading, same "unknown concept id" check, same
//! result text — instead of two independently maintained copies that can
//! silently drift. Each caller decides how to surface the `Result` this
//! returns: `okf-cli` prints the `Ok` text and exits non-zero on `Err`;
//! `okf-mcp` wraps either into an MCP tool response.

use anyhow::{anyhow, Result};
use okf_parser::Concept;
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
}
