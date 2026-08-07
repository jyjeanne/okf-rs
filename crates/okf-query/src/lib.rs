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
//! # Caching
//!
//! Every concept-consuming function here (everything except `search`/
//! `search_ranked`, which build their index straight from `bundle`) comes
//! in two forms: a `bundle: &Path`-taking one that reads and parses the
//! bundle fresh every call (via [`load_concepts`]), and a
//! `*_from_concepts` sibling that runs the identical query against
//! already-loaded `concepts` instead. `okf-cli` only ever needs the
//! former — it's a fresh process per invocation, so there's nothing to
//! amortize. A long-lived caller fielding many queries against the same
//! bundle in one process (`okf-mcp`'s per-session cache, see its
//! `cache` module) instead loads once, keeps the `Vec<Concept>` around
//! for as long as the bundle on disk hasn't changed, and calls the
//! `*_from_concepts` form — skipping the walk-and-parse cost on every
//! query without ever risking stale results, since invalidation is the
//! caller's responsibility and entirely outside this crate.
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

/// Resolves `query` to a concept via a ranked full-text search fallback
/// (the same index [`search_ranked`] uses), for when it isn't an exact
/// concept id — see [`explore`], which checks that cheap exact-id path
/// first and only builds the index this needs when it's actually
/// required, rather than paying for it on every call regardless.
fn resolve_explore_target_by_search<'a>(
    graph: &okf_graph::Graph<'a>,
    concepts: &[Concept],
    query: &str,
) -> Result<&'a Concept> {
    let index = okf_search::FullTextIndex::build_from_concepts(concepts)?;
    let hit = index
        .search(query, 1)?
        .into_iter()
        .next()
        .ok_or_else(|| anyhow!("no concept found matching `{query}`"))?;
    graph
        .get(&hit.id)
        .ok_or_else(|| anyhow!("internal error: search hit `{}` not in the bundle", hit.id))
}

fn id_list(ids: &[&Concept]) -> String {
    if ids.is_empty() {
        "none".to_string()
    } else {
        ids.iter()
            .map(|c| c.id.as_str())
            .collect::<Vec<_>>()
            .join(", ")
    }
}

/// One-call context for a concept: signature, description, direct
/// callers/callees, blast radius (every concept transitively affected if
/// this one changes — see [`okf_graph::Graph::transitive_callers`]),
/// public-API membership, and call-cycle membership — everything an
/// agent asking "what does this do, and what does touching it put at
/// risk" would otherwise need several separate `search`/`graph_*` calls
/// to assemble, in one response.
///
/// `query` is resolved the same way [`resolve_explore_target`] does: an
/// exact concept id wins; otherwise the top hit of a ranked full-text
/// search (so a natural-language or approximate query still resolves to
/// something, the same as `search --ranked`).
pub fn explore(bundle: &Path, query: &str) -> Result<String> {
    let concepts = load_concepts(bundle)?;
    explore_from_concepts(&concepts, query)
}

/// Same as [`explore`], but runs against `concepts` already loaded in
/// memory instead of reading `bundle` fresh — see the crate-level
/// `# Caching` note.
pub fn explore_from_concepts(concepts: &[Concept], query: &str) -> Result<String> {
    let graph = okf_graph::Graph::build(concepts);
    // The common case -- an agent chaining `search`/a previous `explore`
    // call, which already has a concept id in hand -- never needs the
    // full-text index at all, so it's only built on the free-text
    // fallback path below, not unconditionally on every call.
    let concept = match graph.get(query) {
        Some(concept) => concept,
        None => resolve_explore_target_by_search(&graph, concepts, query)?,
    };

    let is_public_api = okf_graph::is_public_api(concept);
    let in_cycle = graph
        .cycles()
        .into_iter()
        .flatten()
        .any(|id| id == concept.id);
    let callers = graph.callers(&concept.id);
    let callees = graph.callees(&concept.id);
    let blast_radius = graph.transitive_callers(&concept.id, None);

    let mut out = format!("{} — {}\n", concept.id, concept.frontmatter_type());
    if let Some(signature) = &concept.signature {
        out.push_str(&format!("\n{signature}\n"));
    }
    if let Some(description) = &concept.description {
        out.push_str(&format!("\n{description}\n"));
    }
    out.push_str(&format!(
        "\nCallers ({}): {}\n",
        callers.len(),
        id_list(&callers)
    ));
    out.push_str(&format!(
        "Callees ({}): {}\n",
        callees.len(),
        id_list(&callees)
    ));
    out.push_str(&format!(
        "Blast radius ({} concept(s) transitively depend on this): {}\n",
        blast_radius.len(),
        id_list(&blast_radius)
    ));
    out.push_str(&format!(
        "Public API: {}\n",
        if is_public_api { "yes" } else { "no" }
    ));
    out.push_str(&format!(
        "In a call cycle: {}",
        if in_cycle { "yes" } else { "no" }
    ));
    Ok(out)
}

/// Concepts that directly call `id`.
pub fn graph_callers(bundle: &Path, id: &str) -> Result<String> {
    let concepts = load_concepts(bundle)?;
    graph_callers_from_concepts(&concepts, id)
}

/// Same as [`graph_callers`], but runs against `concepts` already loaded
/// in memory instead of reading `bundle` fresh — see the crate-level
/// `# Caching` note.
pub fn graph_callers_from_concepts(concepts: &[Concept], id: &str) -> Result<String> {
    let graph = okf_graph::Graph::build(concepts);
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
    graph_callees_from_concepts(&concepts, id)
}

/// Same as [`graph_callees`], but runs against `concepts` already loaded
/// in memory instead of reading `bundle` fresh — see the crate-level
/// `# Caching` note.
pub fn graph_callees_from_concepts(concepts: &[Concept], id: &str) -> Result<String> {
    let graph = okf_graph::Graph::build(concepts);
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
    graph_api_from_concepts(&concepts)
}

/// Same as [`graph_api`], but runs against `concepts` already loaded in
/// memory instead of reading `bundle` fresh — see the crate-level
/// `# Caching` note.
pub fn graph_api_from_concepts(concepts: &[Concept]) -> Result<String> {
    let graph = okf_graph::Graph::build(concepts);
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
    graph_cycles_from_concepts(&concepts)
}

/// Same as [`graph_cycles`], but runs against `concepts` already loaded
/// in memory instead of reading `bundle` fresh — see the crate-level
/// `# Caching` note.
pub fn graph_cycles_from_concepts(concepts: &[Concept]) -> Result<String> {
    let graph = okf_graph::Graph::build(concepts);
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
    graph_isolated_from_concepts(&concepts)
}

/// Same as [`graph_isolated`], but runs against `concepts` already loaded
/// in memory instead of reading `bundle` fresh — see the crate-level
/// `# Caching` note.
pub fn graph_isolated_from_concepts(concepts: &[Concept]) -> Result<String> {
    let graph = okf_graph::Graph::build(concepts);
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
/// The call-graph-participation metric excludes `Module`/`Package`/
/// `Document` concepts from its denominator, matching
/// `Graph::isolated_concepts`'s own exclusion: they're structural
/// containers or documentation that are never expected to carry a
/// `Calls`/`CalledBy` edge, so counting them would understate coverage
/// for reasons that have nothing to do with documentation or analysis
/// quality.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CoverageReport {
    /// Total number of concepts in the bundle.
    pub total_concepts: usize,
    /// How many concepts have a non-empty `description`.
    pub with_description: usize,
    /// How many concepts have at least one tag.
    pub with_tags: usize,
    /// Call-graph participation, or `None` when there are no non-
    /// `Module`/`Package`/`Document` concepts to measure it against (the
    /// "N/A" case in [`coverage`]'s rendered text) — a bundle made only
    /// of structural containers/documentation has nothing this metric
    /// could report on.
    pub graph_participation: Option<GraphParticipation>,
}

/// How much of a bundle's call-graph-eligible concepts (everything
/// except `Module`/`Package`/`Document`) actually participate in the
/// `Calls`/`CalledBy` graph — see [`CoverageReport::graph_participation`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct GraphParticipation {
    /// Concepts with at least one `Calls`/`CalledBy` edge.
    pub connected: usize,
    /// Concepts eligible to participate at all (excludes
    /// `Module`/`Package`/`Document`).
    pub eligible: usize,
}

impl fmt::Display for CoverageReport {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.total_concepts == 0 {
            return write!(f, "Bundle has no concepts");
        }
        let graph_line = match self.graph_participation {
            None => "  N/A (no non-Module/Package/Document concepts to measure) participate in the call graph"
                .to_string(),
            Some(GraphParticipation { connected, eligible }) => format!(
                "  {}% ({connected}/{eligible}) participate in the call graph (excludes Module/Package/Document; see `graph isolated` for the rest)",
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

/// Same as [`coverage`], but runs against `concepts` already loaded in
/// memory instead of reading `bundle` fresh — see the crate-level
/// `# Caching` note.
pub fn coverage_from_concepts(concepts: &[Concept]) -> String {
    coverage_report_from_concepts(concepts).to_string()
}

/// The data behind [`coverage`], as a [`CoverageReport`] instead of text.
pub fn coverage_report(bundle: &Path) -> Result<CoverageReport> {
    let concepts = load_concepts(bundle)?;
    Ok(coverage_report_from_concepts(&concepts))
}

/// Same as [`coverage_report`], but runs against `concepts` already
/// loaded in memory instead of reading `bundle` fresh — see the
/// crate-level `# Caching` note.
pub fn coverage_report_from_concepts(concepts: &[Concept]) -> CoverageReport {
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

    let graph = okf_graph::Graph::build(concepts);
    let isolated: std::collections::HashSet<&str> = graph
        .isolated_concepts()
        .iter()
        .map(|c| c.id.as_str())
        .collect();
    let eligible = concepts
        .iter()
        .filter(|c| !okf_graph::is_structural(c.kind))
        .count();
    let connected = concepts
        .iter()
        .filter(|c| !okf_graph::is_structural(c.kind))
        .filter(|c| !isolated.contains(c.id.as_str()))
        .count();

    let graph_participation = if eligible == 0 {
        None
    } else {
        Some(GraphParticipation {
            connected,
            eligible,
        })
    };

    CoverageReport {
        total_concepts,
        with_description,
        with_tags,
        graph_participation,
    }
}

fn percent(part: usize, total: usize) -> usize {
    (part * 100).checked_div(total).unwrap_or(0)
}

/// Ranks concepts by embedding-cosine similarity to `query` — "find by
/// meaning," layered on top of (not replacing) [`search`]/[`search_ranked`],
/// via any OpenAI-compatible `/embeddings` endpoint. Like [`suggest_links`],
/// this needs an already-configured [`okf_enrich::EnrichClient`] rather
/// than just a bundle path, since it makes network calls — one per
/// described concept in the bundle, plus one for `query`.
pub fn search_semantic(
    bundle: &Path,
    client: &okf_enrich::EnrichClient,
    query: &str,
    limit: usize,
) -> Result<String> {
    let concepts = load_concepts(bundle)?;
    search_semantic_from_concepts(&concepts, client, query, limit)
}

/// Same as [`search_semantic`], but runs against `concepts` already
/// loaded in memory instead of reading `bundle` fresh — see the
/// crate-level `# Caching` note.
pub fn search_semantic_from_concepts(
    concepts: &[Concept],
    client: &okf_enrich::EnrichClient,
    query: &str,
    limit: usize,
) -> Result<String> {
    let hits = okf_enrich::semantic_search(client, concepts, query, limit)?;
    if hits.is_empty() {
        return Ok(format!(
            "No semantic matches for `{query}` (no concept in the bundle has a description to embed and compare against — run `generate --enrich` first, or add descriptions by hand)"
        ));
    }
    Ok(hits
        .iter()
        .map(|hit| {
            format!(
                "{:>6.3}  {:<24} {:<20} {}",
                hit.score, hit.title, hit.concept_type, hit.id
            )
        })
        .collect::<Vec<_>>()
        .join("\n"))
}

/// Cross-module call dependency edges: which modules call into which.
pub fn graph_modules(bundle: &Path) -> Result<String> {
    let concepts = load_concepts(bundle)?;
    graph_modules_from_concepts(&concepts)
}

/// Same as [`graph_modules`], but runs against `concepts` already loaded
/// in memory instead of reading `bundle` fresh — see the crate-level
/// `# Caching` note.
pub fn graph_modules_from_concepts(concepts: &[Concept]) -> Result<String> {
    let graph = okf_graph::Graph::build(concepts);
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

/// Same as [`graph_stats`], but runs against `concepts` already loaded in
/// memory instead of reading `bundle` fresh — see the crate-level
/// `# Caching` note.
pub fn graph_stats_from_concepts(concepts: &[Concept]) -> String {
    graph_stats_report_from_concepts(concepts).to_string()
}

/// The data behind [`graph_stats`], as a [`GraphStatsReport`] instead of
/// text.
pub fn graph_stats_report(bundle: &Path) -> Result<GraphStatsReport> {
    let concepts = load_concepts(bundle)?;
    Ok(graph_stats_report_from_concepts(&concepts))
}

/// Same as [`graph_stats_report`], but runs against `concepts` already
/// loaded in memory instead of reading `bundle` fresh — see the
/// crate-level `# Caching` note.
pub fn graph_stats_report_from_concepts(concepts: &[Concept]) -> GraphStatsReport {
    let graph = okf_graph::Graph::build(concepts);

    let mut by_kind: BTreeMap<ConceptKind, usize> = Default::default();
    for c in concepts {
        *by_kind.entry(c.kind).or_default() += 1;
    }

    let mut by_relation: BTreeMap<RelationKind, usize> = Default::default();
    for c in concepts {
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

    GraphStatsReport {
        total_concepts: concepts.len(),
        by_kind,
        by_relation,
        components,
        isolated_count,
    }
}

/// The shortest call path between two concept ids.
pub fn graph_path(bundle: &Path, from: &str, to: &str) -> Result<String> {
    let concepts = load_concepts(bundle)?;
    graph_path_from_concepts(&concepts, from, to)
}

/// Same as [`graph_path`], but runs against `concepts` already loaded in
/// memory instead of reading `bundle` fresh — see the crate-level
/// `# Caching` note.
pub fn graph_path_from_concepts(concepts: &[Concept], from: &str, to: &str) -> Result<String> {
    let graph = okf_graph::Graph::build(concepts);
    require_concept(&graph, from)?;
    require_concept(&graph, to)?;
    match graph.shortest_call_path(from, to) {
        Some(steps) => Ok(steps.join(" -> ")),
        None => Ok(format!("No call path found from `{from}` to `{to}`")),
    }
}

/// Explains *why* a relationship exists between two concepts: which kind
/// it is, and a human-readable reason derived from its provenance (see
/// [`okf_parser::Relationship::reason`]) — the "Explainability" roadmap
/// item, built directly on the edge provenance/confidence it names as
/// its foundation. Looks for a direct relationship first, checking both
/// concepts' own relationship lists (a kind like `Calls`/`CalledBy` is
/// only ever recorded on one side by the extractor that produced it, so
/// `from`'s list alone isn't guaranteed to have it even when a real edge
/// exists in the other direction). When there's no single direct edge,
/// falls back to explaining the shortest call *path* between them, one
/// hop at a time.
pub fn explain(bundle: &Path, from: &str, to: &str) -> Result<String> {
    let concepts = load_concepts(bundle)?;
    explain_from_concepts(&concepts, from, to)
}

/// Same as [`explain`], but runs against `concepts` already loaded in
/// memory instead of reading `bundle` fresh — see the crate-level
/// `# Caching` note.
pub fn explain_from_concepts(concepts: &[Concept], from: &str, to: &str) -> Result<String> {
    let graph = okf_graph::Graph::build(concepts);
    require_concept(&graph, from)?;
    require_concept(&graph, to)?;

    if let Some(text) = direct_relationship_explanation(&graph, from, to) {
        return Ok(text);
    }

    match graph.shortest_call_path(from, to) {
        Some(steps) if steps.len() >= 2 => Ok(steps
            .windows(2)
            .map(|pair| {
                direct_relationship_explanation(&graph, pair[0], pair[1]).unwrap_or_else(|| {
                    format!(
                        "{}\n    |\n    v\n{}\n\nReason: part of the shortest call path, but no single direct relationship recorded between these two specifically.",
                        pair[0], pair[1]
                    )
                })
            })
            .collect::<Vec<_>>()
            .join("\n\n")),
        _ => Ok(format!(
            "No direct relationship or call path found between `{from}` and `{to}`"
        )),
    }
}

/// Looks for a relationship directly connecting `a` and `b`, in either
/// direction, and renders it as an explanation if found. Checks `a`'s
/// own relationships for one targeting `b` first, then `b`'s for one
/// targeting `a` — the actual direction of whichever edge is found (not
/// necessarily `a -> b`) is what gets rendered, so e.g. a `CalledBy`
/// edge only recorded on the callee's page still explains correctly.
fn direct_relationship_explanation(
    graph: &okf_graph::Graph<'_>,
    a: &str,
    b: &str,
) -> Option<String> {
    if let Some(concept) = graph.get(a) {
        if let Some(rel) = concept.relationships.iter().find(|r| r.target == b) {
            return Some(render_explanation(a, rel, b));
        }
    }
    if let Some(concept) = graph.get(b) {
        if let Some(rel) = concept.relationships.iter().find(|r| r.target == a) {
            return Some(render_explanation(b, rel, a));
        }
    }
    None
}

fn render_explanation(source: &str, rel: &okf_parser::Relationship, target: &str) -> String {
    format!(
        "{source}\n    |\n    {}\n    v\n{target}\n\nReason: {}",
        rel.kind.label().to_lowercase(),
        rel.reason()
    )
}

/// Each package's position in the layered architecture derived from the
/// package dependency graph (`okf_arch::layers`) — layer 0 is a package
/// with no dependency on any other package in the bundle. Unlike
/// [`coverage`]/[`graph_stats`], `okf-arch`'s own [`okf_arch::PackageLayer`]
/// is already the structured form; there's no separate `*_report`
/// function here to duplicate it.
pub fn graph_layers(bundle: &Path) -> Result<String> {
    let concepts = load_concepts(bundle)?;
    graph_layers_from_concepts(&concepts)
}

/// Same as [`graph_layers`], but runs against `concepts` already loaded
/// in memory instead of reading `bundle` fresh — see the crate-level
/// `# Caching` note.
pub fn graph_layers_from_concepts(concepts: &[Concept]) -> Result<String> {
    let graph = okf_graph::Graph::build(concepts);
    let layers = okf_arch::layers(&graph);
    if layers.is_empty() {
        return Ok("No packages found to layer (no Package concepts in the bundle)".to_string());
    }
    Ok(layers
        .iter()
        .map(|l| format!("{:<3} {}", l.layer, l.package_id))
        .collect::<Vec<_>>()
        .join("\n"))
}

/// Clusters of packages that depend on each other, directly or
/// transitively (`okf_arch::domains`) — a package with no cross-package
/// dependency in or out is still its own singleton domain.
pub fn graph_domains(bundle: &Path) -> Result<String> {
    let concepts = load_concepts(bundle)?;
    graph_domains_from_concepts(&concepts)
}

/// Same as [`graph_domains`], but runs against `concepts` already loaded
/// in memory instead of reading `bundle` fresh — see the crate-level
/// `# Caching` note.
pub fn graph_domains_from_concepts(concepts: &[Concept]) -> Result<String> {
    let graph = okf_graph::Graph::build(concepts);
    let domains = okf_arch::domains(&graph);
    if domains.is_empty() {
        return Ok("No packages found (no Package concepts in the bundle)".to_string());
    }
    Ok(domains
        .iter()
        .enumerate()
        .map(|(i, d)| format!("[{}] {}", i + 1, d.package_ids.join(", ")))
        .collect::<Vec<_>>()
        .join("\n"))
}

/// Clusters of packages found by modularity-optimization community
/// detection (`okf_arch::communities`) — a finer-grained signal than
/// [`graph_domains`]'s plain connected components: two packages that are
/// technically reachable from each other can still land in different
/// communities here if the connection between them is weak relative to
/// how strongly each already collaborates within its own cluster.
pub fn graph_communities(bundle: &Path) -> Result<String> {
    let concepts = load_concepts(bundle)?;
    graph_communities_from_concepts(&concepts)
}

/// Same as [`graph_communities`], but runs against `concepts` already
/// loaded in memory instead of reading `bundle` fresh — see the
/// crate-level `# Caching` note.
pub fn graph_communities_from_concepts(concepts: &[Concept]) -> Result<String> {
    let graph = okf_graph::Graph::build(concepts);
    let communities = okf_arch::communities(&graph);
    if communities.is_empty() {
        return Ok("No packages found (no Package concepts in the bundle)".to_string());
    }
    Ok(communities
        .iter()
        .enumerate()
        .map(|(i, c)| format!("[{}] {}", i + 1, c.package_ids.join(", ")))
        .collect::<Vec<_>>()
        .join("\n"))
}

/// Design patterns detected via structural/naming heuristics
/// (`okf_arch::detect_patterns`) — see that function's own docs for what
/// this does and doesn't claim to detect.
pub fn graph_patterns(bundle: &Path) -> Result<String> {
    let concepts = load_concepts(bundle)?;
    Ok(graph_patterns_from_concepts(&concepts))
}

/// Same as [`graph_patterns`], but runs against `concepts` already
/// loaded in memory instead of reading `bundle` fresh — see the
/// crate-level `# Caching` note.
pub fn graph_patterns_from_concepts(concepts: &[Concept]) -> String {
    let found = okf_arch::detect_patterns(concepts);
    if found.is_empty() {
        return "No design patterns detected".to_string();
    }
    found
        .iter()
        .map(|p| format!("{:<10} {} — {}", p.kind.as_str(), p.concept_id, p.evidence))
        .collect::<Vec<_>>()
        .join("\n")
}

/// REST endpoints, database models, and event-flow participants detected
/// via structural/naming heuristics (`okf_arch::detect_features`) — see
/// that function's own docs for what this does and doesn't claim to
/// detect.
pub fn graph_features(bundle: &Path) -> Result<String> {
    let concepts = load_concepts(bundle)?;
    Ok(graph_features_from_concepts(&concepts))
}

/// Same as [`graph_features`], but runs against `concepts` already
/// loaded in memory instead of reading `bundle` fresh — see the
/// crate-level `# Caching` note.
pub fn graph_features_from_concepts(concepts: &[Concept]) -> String {
    let found = okf_arch::detect_features(concepts);
    if found.is_empty() {
        return "No REST endpoints, database models, or event-flow participants detected"
            .to_string();
    }
    found
        .iter()
        .map(|f| format!("{:<13} {} — {}", f.kind.as_str(), f.concept_id, f.evidence))
        .collect::<Vec<_>>()
        .join("\n")
}

/// AI-suggested missing links between semantically close concepts —
/// `okf_enrich::suggest_missing_links`, the one query-layer operation
/// that needs an already-configured [`okf_enrich::EnrichClient`] rather
/// than just a bundle path, since (unlike every other function here) it
/// makes network calls to an OpenAI-compatible endpoint. Building that
/// client (resolving `--enrich-*`/`OKF_ENRICH_*` config) is the caller's
/// job, the same way `okf-cli`'s `generate --enrich` already does it —
/// this function's role is only to load the bundle and format the
/// result, matching every other function here.
pub fn suggest_links(
    bundle: &Path,
    client: &okf_enrich::EnrichClient,
    max_candidates: usize,
) -> Result<String> {
    let concepts = load_concepts(bundle)?;
    suggest_links_from_concepts(&concepts, client, max_candidates)
}

/// Same as [`suggest_links`], but runs against `concepts` already loaded
/// in memory instead of reading `bundle` fresh — see the crate-level
/// `# Caching` note.
pub fn suggest_links_from_concepts(
    concepts: &[Concept],
    client: &okf_enrich::EnrichClient,
    max_candidates: usize,
) -> Result<String> {
    let suggestions = okf_enrich::suggest_missing_links(client, concepts, max_candidates)?;
    if suggestions.is_empty() {
        return Ok("No missing links suggested".to_string());
    }
    Ok(suggestions
        .iter()
        .map(|s| format!("{} <-> {}: {}", s.from_id, s.to_id, s.reason))
        .collect::<Vec<_>>()
        .join("\n"))
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
    fn explore_resolves_an_exact_id_and_reports_callers_callees_and_blast_radius() {
        let dir = sample_bundle();
        let text = explore(dir.path(), "functions/auth/decode_jwt").unwrap();
        assert!(text.starts_with("functions/auth/decode_jwt — Rust Function"));
        assert!(text.contains("Callers (1): functions/auth/verify_token"));
        assert!(text.contains("Callees (0): none"));
        assert!(text.contains(
            "Blast radius (1 concept(s) transitively depend on this): functions/auth/verify_token"
        ));
        assert!(text.contains("Public API: yes"));
        assert!(text.contains("In a call cycle: no"));
    }

    #[test]
    fn explore_resolves_a_natural_language_query_via_ranked_search() {
        let dir = sample_bundle();
        write(
            dir.path(),
            "functions/format/currency.md",
            "---\ntype: Rust Function\ntitle: format_currency\ndescription: Formats a currency amount for locale-aware display.\nresource: src/format.rs#L1\n---\n\nbody\n",
        );
        let text = explore(dir.path(), "currency amount for display").unwrap();
        assert!(text.starts_with("functions/format/currency — Rust Function"));
    }

    #[test]
    fn explore_errors_clearly_when_nothing_matches() {
        let dir = sample_bundle();
        let err = explore(dir.path(), "no such thing anywhere").unwrap_err();
        assert!(err.to_string().contains("no concept found matching"));
    }

    #[test]
    fn search_semantic_ranks_the_closer_embedding_first() {
        use okf_enrich::test_support::{embedding_client_for, start_embedding_mock_server};
        use std::collections::HashMap;

        let dir = sample_bundle();
        write(
            dir.path(),
            "functions/format/currency.md",
            "---\ntype: Rust Function\ntitle: format_currency\ndescription: Formats a currency amount for display.\nresource: src/format.rs#L1\n---\n\nbody\n",
        );
        // Overwrite verify_token's own file so it carries a description
        // (sample_bundle's copy has none, and an undescribed concept is
        // never embedded).
        write(
            dir.path(),
            "functions/auth/verify_token.md",
            "---\ntype: Rust Function\ntitle: verify_token\ndescription: Verifies a signed authentication token.\nresource: src/auth.rs#L1\nrelationships:\n  calls:\n    - functions/auth/decode_jwt\n---\n\nbody\n",
        );

        let mut responses = HashMap::new();
        responses.insert(
            "auth::verify_token (Rust Function): Verifies a signed authentication token."
                .to_string(),
            vec![1.0, 0.0],
        );
        responses.insert(
            "format::currency (Rust Function): Formats a currency amount for display.".to_string(),
            vec![0.0, 1.0],
        );
        responses.insert("authentication".to_string(), vec![1.0, 0.0]);

        let server = start_embedding_mock_server(responses);
        let client = embedding_client_for(&server);

        let text = search_semantic(dir.path(), &client, "authentication", 10).unwrap();
        let first_line = text.lines().next().unwrap();
        assert!(
            first_line.contains("functions/auth/verify_token"),
            "expected the closer embedding first: {text}"
        );
    }

    #[test]
    fn search_semantic_reports_no_matches_without_any_described_concept() {
        use okf_enrich::test_support::{embedding_client_for, start_embedding_mock_server};
        use std::collections::HashMap;

        let dir = sample_bundle();
        let server = start_embedding_mock_server(HashMap::new());
        let client = embedding_client_for(&server);

        let text = search_semantic(dir.path(), &client, "anything", 10).unwrap();
        assert!(text.starts_with("No semantic matches for `anything`"));
        assert_eq!(
            server
                .request_count
                .load(std::sync::atomic::Ordering::SeqCst),
            0
        );
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
    fn explain_renders_a_direct_edge_with_its_reason() {
        let dir = sample_bundle();
        let text = explain(
            dir.path(),
            "functions/auth/verify_token",
            "functions/auth/decode_jwt",
        )
        .unwrap();
        assert!(text.starts_with("functions/auth/verify_token"));
        assert!(text.contains("calls"));
        assert!(text.contains("functions/auth/decode_jwt"));
        assert!(text.contains("Reason:"));
        assert!(text.contains("Tree-sitter"));
    }

    #[test]
    fn explain_works_regardless_of_which_order_the_ids_are_given_in() {
        let dir = sample_bundle();
        // Asked in the reverse order, it finds `decode_jwt`'s own
        // `called_by: verify_token` entry -- a real, correctly-recorded
        // relationship in its own right (the same underlying fact,
        // recorded from the other side), rendered in *that* direction
        // rather than forcing a canonical one that doesn't match what's
        // actually on either concept's page.
        let text = explain(
            dir.path(),
            "functions/auth/decode_jwt",
            "functions/auth/verify_token",
        )
        .unwrap();
        assert!(text.starts_with("functions/auth/decode_jwt"));
        assert!(text.contains("called by"));
        assert!(text.contains("functions/auth/verify_token"));
        assert!(text.contains("Reason:"));
    }

    #[test]
    fn explain_names_the_real_lsp_server_for_a_semantically_resolved_edge() {
        let dir = tempfile::tempdir().unwrap();
        write(
            dir.path(),
            "functions/a.md",
            "---\ntype: Rust Function\ntitle: a\nresource: src/lib.rs#L1\nrelationships:\n  calls:\n    - target: functions/b\n      resolved_by: rust-analyzer\n      confidence: semantic\n---\n\nbody\n",
        );
        write(
            dir.path(),
            "functions/b.md",
            "---\ntype: Rust Function\ntitle: b\nresource: src/lib.rs#L2\n---\n\nbody\n",
        );

        let text = explain(dir.path(), "functions/a", "functions/b").unwrap();
        assert!(text.contains("rust-analyzer"));
        assert!(text.contains("more than one candidate"));
    }

    #[test]
    fn explain_reports_clearly_when_there_is_no_relationship_or_path() {
        let dir = tempfile::tempdir().unwrap();
        write(
            dir.path(),
            "functions/a.md",
            "---\ntype: Rust Function\ntitle: a\nresource: src/lib.rs#L1\n---\n\nbody\n",
        );
        write(
            dir.path(),
            "functions/b.md",
            "---\ntype: Rust Function\ntitle: b\nresource: src/lib.rs#L2\n---\n\nbody\n",
        );

        let text = explain(dir.path(), "functions/a", "functions/b").unwrap();
        assert!(text.contains("No direct relationship or call path found"));
    }

    #[test]
    fn explain_unknown_concept_id_is_a_clear_error() {
        let dir = sample_bundle();
        let err = explain(dir.path(), "functions/nope", "functions/auth/decode_jwt").unwrap_err();
        assert!(err.to_string().contains("no concept with id"));
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
            text.contains("N/A (no non-Module/Package/Document concepts to measure) participate in the call graph"),
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

    #[test]
    fn graph_layers_and_domains_report_none_found_without_any_package() {
        let dir = sample_bundle();
        assert_eq!(
            graph_layers(dir.path()).unwrap(),
            "No packages found to layer (no Package concepts in the bundle)"
        );
        assert_eq!(
            graph_domains(dir.path()).unwrap(),
            "No packages found (no Package concepts in the bundle)"
        );
    }

    #[test]
    fn graph_layers_and_domains_report_a_two_package_dependency() {
        let dir = sample_bundle();
        write(
            dir.path(),
            "packages/core.md",
            "---\ntype: Rust Package\ntitle: core\nresource: core/Cargo.toml\n---\n\nbody\n",
        );
        write(
            dir.path(),
            "packages/app.md",
            "---\ntype: Rust Package\ntitle: app\nresource: app/Cargo.toml\n---\n\nbody\n",
        );
        write(
            dir.path(),
            "modules/core.md",
            "---\ntype: Rust Module\ntitle: core\nresource: core/src/lib.rs#L1\nrelationships:\n  member_of:\n    - packages/core\n---\n\nbody\n",
        );
        write(
            dir.path(),
            "modules/app.md",
            "---\ntype: Rust Module\ntitle: app\nresource: app/src/lib.rs#L1\nrelationships:\n  member_of:\n    - packages/app\n---\n\nbody\n",
        );
        write(
            dir.path(),
            "functions/app_fn.md",
            "---\ntype: Rust Function\ntitle: app_fn\nresource: app/src/lib.rs#L1\nrelationships:\n  calls:\n    - functions/core_fn\n---\n\nbody\n",
        );
        write(
            dir.path(),
            "functions/core_fn.md",
            "---\ntype: Rust Function\ntitle: core_fn\nresource: core/src/lib.rs#L1\n---\n\nbody\n",
        );

        let layers = graph_layers(dir.path()).unwrap();
        assert!(layers.contains("0   packages/core"));
        assert!(layers.contains("1   packages/app"));

        let domains = graph_domains(dir.path()).unwrap();
        assert!(domains.contains("packages/app, packages/core"));

        let communities = graph_communities(dir.path()).unwrap();
        assert!(communities.contains("packages/app, packages/core"));
    }

    #[test]
    fn graph_communities_reports_none_found_without_any_package() {
        let dir = sample_bundle();
        assert_eq!(
            graph_communities(dir.path()).unwrap(),
            "No packages found (no Package concepts in the bundle)"
        );
    }

    #[test]
    fn graph_patterns_finds_a_builder() {
        let dir = sample_bundle();
        write(
            dir.path(),
            "classes/RequestBuilder.md",
            "---\ntype: Rust Class\ntitle: RequestBuilder\nresource: src/req.rs#L1\n---\n\nbody\n",
        );
        write(
            dir.path(),
            "functions/RequestBuilder/build.md",
            "---\ntype: Rust Method\ntitle: build\nresource: src/req.rs#L5\n---\n\nbody\n",
        );

        let text = graph_patterns(dir.path()).unwrap();
        assert!(text.contains("Builder"));
        assert!(text.contains("classes/RequestBuilder"));
    }

    #[test]
    fn graph_patterns_reports_none_found_when_there_are_no_matches() {
        let dir = sample_bundle();
        assert_eq!(
            graph_patterns(dir.path()).unwrap(),
            "No design patterns detected"
        );
    }

    #[test]
    fn graph_features_finds_a_database_model() {
        let dir = sample_bundle();
        write(
            dir.path(),
            "classes/UserModel.md",
            "---\ntype: Rust Class\ntitle: UserModel\nresource: src/models.rs#L1\n---\n\nbody\n",
        );

        let text = graph_features(dir.path()).unwrap();
        assert!(text.contains("DatabaseModel"));
        assert!(text.contains("classes/UserModel"));
    }

    #[test]
    fn graph_features_reports_none_found_when_there_are_no_matches() {
        let dir = sample_bundle();
        assert_eq!(
            graph_features(dir.path()).unwrap(),
            "No REST endpoints, database models, or event-flow participants detected"
        );
    }

    #[test]
    fn suggest_links_reports_a_suggestion_from_the_endpoint() {
        use okf_enrich::test_support::{client_for, start_mock_server};

        let dir = sample_bundle();
        write(
            dir.path(),
            "functions/auth/other.md",
            "---\ntype: Rust Function\ntitle: other\ndescription: Reads an auth token from the request header.\nresource: src/auth.rs#L20\n---\n\nbody\n",
        );

        let server = start_mock_server("looks related");
        let client = client_for(&server);

        let text = suggest_links(dir.path(), &client, 5).unwrap();
        assert!(text.contains("looks related"));
    }

    #[test]
    fn suggest_links_reports_none_found_when_the_model_says_no() {
        use okf_enrich::test_support::{client_for, start_mock_server};

        let dir = sample_bundle();
        write(
            dir.path(),
            "functions/auth/other.md",
            "---\ntype: Rust Function\ntitle: other\ndescription: Reads an auth token from the request header.\nresource: src/auth.rs#L20\n---\n\nbody\n",
        );

        let server = start_mock_server("NO");
        let client = client_for(&server);

        assert_eq!(
            suggest_links(dir.path(), &client, 5).unwrap(),
            "No missing links suggested"
        );
    }

    // The `*_from_concepts` siblings exist purely so a long-lived caller
    // (see okf-mcp's cache) can amortize parsing across calls -- each one
    // must produce exactly the same output as its `bundle: &Path`-taking
    // counterpart given the same concepts, or the cache would silently
    // change what an agent sees. These spot-check that parity across a
    // representative sample rather than every single function pair,
    // since the two forms differ only in "read from disk first or not".
    #[test]
    fn from_concepts_variants_match_their_path_based_counterparts() {
        let dir = sample_bundle();
        let concepts = load_concepts(dir.path()).unwrap();

        assert_eq!(
            explore(dir.path(), "functions/auth/decode_jwt").unwrap(),
            explore_from_concepts(&concepts, "functions/auth/decode_jwt").unwrap()
        );
        assert_eq!(
            graph_callers(dir.path(), "functions/auth/decode_jwt").unwrap(),
            graph_callers_from_concepts(&concepts, "functions/auth/decode_jwt").unwrap()
        );
        assert_eq!(
            graph_callees(dir.path(), "functions/auth/verify_token").unwrap(),
            graph_callees_from_concepts(&concepts, "functions/auth/verify_token").unwrap()
        );
        assert_eq!(
            graph_api(dir.path()).unwrap(),
            graph_api_from_concepts(&concepts).unwrap()
        );
        assert_eq!(
            graph_cycles(dir.path()).unwrap(),
            graph_cycles_from_concepts(&concepts).unwrap()
        );
        assert_eq!(
            graph_isolated(dir.path()).unwrap(),
            graph_isolated_from_concepts(&concepts).unwrap()
        );
        assert_eq!(
            coverage(dir.path()).unwrap(),
            coverage_from_concepts(&concepts)
        );
        assert_eq!(
            graph_modules(dir.path()).unwrap(),
            graph_modules_from_concepts(&concepts).unwrap()
        );
        assert_eq!(
            graph_stats(dir.path()).unwrap(),
            graph_stats_from_concepts(&concepts)
        );
        assert_eq!(
            graph_path(
                dir.path(),
                "functions/auth/verify_token",
                "functions/auth/decode_jwt"
            )
            .unwrap(),
            graph_path_from_concepts(
                &concepts,
                "functions/auth/verify_token",
                "functions/auth/decode_jwt"
            )
            .unwrap()
        );
        assert_eq!(
            explain(
                dir.path(),
                "functions/auth/verify_token",
                "functions/auth/decode_jwt"
            )
            .unwrap(),
            explain_from_concepts(
                &concepts,
                "functions/auth/verify_token",
                "functions/auth/decode_jwt"
            )
            .unwrap()
        );
        assert_eq!(
            graph_patterns(dir.path()).unwrap(),
            graph_patterns_from_concepts(&concepts)
        );
        assert_eq!(
            graph_features(dir.path()).unwrap(),
            graph_features_from_concepts(&concepts)
        );
    }

    #[test]
    fn layers_domains_communities_from_concepts_match_the_path_based_form() {
        let dir = sample_bundle();
        write(
            dir.path(),
            "packages/core.md",
            "---\ntype: Rust Package\ntitle: core\nresource: core/Cargo.toml\n---\n\nbody\n",
        );
        write(
            dir.path(),
            "packages/app.md",
            "---\ntype: Rust Package\ntitle: app\nresource: app/Cargo.toml\n---\n\nbody\n",
        );
        write(
            dir.path(),
            "modules/core.md",
            "---\ntype: Rust Module\ntitle: core\nresource: core/src/lib.rs#L1\nrelationships:\n  member_of:\n    - packages/core\n---\n\nbody\n",
        );
        write(
            dir.path(),
            "modules/app.md",
            "---\ntype: Rust Module\ntitle: app\nresource: app/src/lib.rs#L1\nrelationships:\n  member_of:\n    - packages/app\n---\n\nbody\n",
        );
        write(
            dir.path(),
            "functions/app_fn.md",
            "---\ntype: Rust Function\ntitle: app_fn\nresource: app/src/lib.rs#L1\nrelationships:\n  calls:\n    - functions/core_fn\n---\n\nbody\n",
        );
        write(
            dir.path(),
            "functions/core_fn.md",
            "---\ntype: Rust Function\ntitle: core_fn\nresource: core/src/lib.rs#L1\n---\n\nbody\n",
        );
        let concepts = load_concepts(dir.path()).unwrap();

        assert_eq!(
            graph_layers(dir.path()).unwrap(),
            graph_layers_from_concepts(&concepts).unwrap()
        );
        assert_eq!(
            graph_domains(dir.path()).unwrap(),
            graph_domains_from_concepts(&concepts).unwrap()
        );
        assert_eq!(
            graph_communities(dir.path()).unwrap(),
            graph_communities_from_concepts(&concepts).unwrap()
        );
    }

    #[test]
    fn search_semantic_from_concepts_matches_the_path_based_form() {
        use okf_enrich::test_support::{embedding_client_for, start_embedding_mock_server};
        use std::collections::HashMap;

        let dir = sample_bundle();
        write(
            dir.path(),
            "functions/auth/verify_token.md",
            "---\ntype: Rust Function\ntitle: verify_token\ndescription: Verifies a signed authentication token.\nresource: src/auth.rs#L1\nrelationships:\n  calls:\n    - functions/auth/decode_jwt\n---\n\nbody\n",
        );
        let mut responses = HashMap::new();
        responses.insert(
            "auth::verify_token (Rust Function): Verifies a signed authentication token."
                .to_string(),
            vec![1.0, 0.0],
        );
        responses.insert("authentication".to_string(), vec![1.0, 0.0]);

        let server = start_embedding_mock_server(responses);
        let client = embedding_client_for(&server);
        let concepts = load_concepts(dir.path()).unwrap();

        assert_eq!(
            search_semantic(dir.path(), &client, "authentication", 10).unwrap(),
            search_semantic_from_concepts(&concepts, &client, "authentication", 10).unwrap()
        );
    }

    #[test]
    fn suggest_links_from_concepts_matches_the_path_based_form() {
        use okf_enrich::test_support::{client_for, start_mock_server};

        let dir = sample_bundle();
        write(
            dir.path(),
            "functions/auth/other.md",
            "---\ntype: Rust Function\ntitle: other\ndescription: Reads an auth token from the request header.\nresource: src/auth.rs#L20\n---\n\nbody\n",
        );
        let server = start_mock_server("looks related");
        let client = client_for(&server);
        let concepts = load_concepts(dir.path()).unwrap();

        assert_eq!(
            suggest_links(dir.path(), &client, 5).unwrap(),
            suggest_links_from_concepts(&concepts, &client, 5).unwrap()
        );
    }
}
