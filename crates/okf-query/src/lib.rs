//! Shared query layer wrapping `okf-search`/`okf-graph`/
//! `okf_parser::read_bundle`, so `okf-cli` and `okf-mcp` express the same
//! seven operations (search, and `okf_graph::Graph`'s six queries) exactly
//! once — same bundle-loading, same "unknown concept id" check, same
//! result text — instead of two independently maintained copies that can
//! silently drift. Each caller decides how to surface the `Result` this
//! returns: `okf-cli` prints the `Ok` text and exits non-zero on `Err`;
//! `okf-mcp` wraps either into an MCP tool response.

use anyhow::{anyhow, Result};
use okf_parser::{Concept, ConceptKind, RelationKind};
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

/// Bundle-wide content-completeness metrics: how much of the knowledge
/// base is actually filled in, as distinct from `okf-rs validate`'s
/// pass/fail structural checks. Computed entirely from the bundle
/// already on disk (via [`load_concepts`]/[`okf_graph::Graph`]) — no
/// re-scan of the source project, so there's no second source of truth
/// to keep in sync.
///
/// The call-graph-participation metric excludes `Module`/`Package`
/// concepts from its denominator, matching `Graph::isolated_concepts`'s
/// own exclusion: they're structural containers that are never expected
/// to carry a `Calls`/`CalledBy` edge, so counting them would understate
/// coverage for reasons that have nothing to do with documentation or
/// analysis quality.
pub fn coverage(bundle: &Path) -> Result<String> {
    let concepts = load_concepts(bundle)?;
    if concepts.is_empty() {
        return Ok("Bundle has no concepts".to_string());
    }
    let total = concepts.len();
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
    let graph_eligible = concepts
        .iter()
        .filter(|c| !matches!(c.kind, ConceptKind::Module | ConceptKind::Package))
        .count();
    let graph_connected = concepts
        .iter()
        .filter(|c| !matches!(c.kind, ConceptKind::Module | ConceptKind::Package))
        .filter(|c| !isolated.contains(c.id.as_str()))
        .count();

    let graph_line = if graph_eligible == 0 {
        "  N/A (no non-Module/Package concepts to measure) participate in the call graph"
            .to_string()
    } else {
        format!(
            "  {}% ({graph_connected}/{graph_eligible}) participate in the call graph (excludes Module/Package; see `graph isolated` for the rest)",
            percent(graph_connected, graph_eligible)
        )
    };

    Ok(format!(
        "{total} concepts\n  {}% ({with_description}/{total}) have a description\n  {}% ({with_tags}/{total}) have at least one tag\n{graph_line}",
        percent(with_description, total),
        percent(with_tags, total),
    ))
}

fn percent(part: usize, total: usize) -> usize {
    if total == 0 {
        0
    } else {
        part * 100 / total
    }
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

/// Bundle-wide graph topology metrics: concept-kind breakdown, edge
/// counts per relationship kind, and connected components of the
/// `Calls`/`CalledBy` graph. Edges are counted per kind rather than as a
/// single total, to sidestep the ambiguous question of whether a
/// resolved `Calls`/`CalledBy` pair is one edge or two.
///
/// Deliberately doesn't report a "depth" metric: the call graph isn't
/// guaranteed acyclic (see `graph_cycles`), so there's no single
/// well-defined notion of depth to report without first committing to a
/// much narrower definition than the word implies.
pub fn graph_stats(bundle: &Path) -> Result<String> {
    let concepts = load_concepts(bundle)?;
    let graph = okf_graph::Graph::build(&concepts);

    let mut by_kind: std::collections::BTreeMap<ConceptKind, usize> = Default::default();
    for c in &concepts {
        *by_kind.entry(c.kind).or_default() += 1;
    }

    let mut by_relation: std::collections::BTreeMap<RelationKind, usize> = Default::default();
    for c in &concepts {
        for rel in &c.relationships {
            *by_relation.entry(rel.kind).or_default() += 1;
        }
    }

    let components = graph.connected_components();
    let isolated_count = graph.isolated_concepts().len();

    let mut out = format!("{} concepts\n\nBy kind:\n", concepts.len());
    for (kind, count) in &by_kind {
        out.push_str(&format!("  {:<12} {count}\n", kind.as_str()));
    }

    out.push_str("\nRelationship edges by kind:\n");
    if by_relation.is_empty() {
        out.push_str("  (none)\n");
    } else {
        for (kind, count) in &by_relation {
            out.push_str(&format!("  {:<12} {count}\n", kind.label()));
        }
    }

    out.push_str(&format!(
        "\nCall graph: {} connected component(s) with at least one Calls/CalledBy edge (sizes shown below — a lone self-recursive concept forms its own size-1 component), {isolated_count} isolated concept(s) with no edge at all (see `graph isolated`)\n",
        components.len()
    ));
    for component in &components {
        out.push_str(&format!(
            "  [{}] {}\n",
            component.len(),
            component.join(", ")
        ));
    }

    Ok(out.trim_end().to_string())
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
