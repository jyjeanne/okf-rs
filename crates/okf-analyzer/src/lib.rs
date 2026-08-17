//! Orchestrates repository scanning (`okf-core`) and per-file extraction
//! (`okf-tree-sitter`) into one project-wide semantic model: every concept
//! plus resolved `Calls`/`CalledBy` relationships.
//!
//! Import relationships are resolved eagerly by the language extractors
//! (they don't need project-wide information). Call relationships need a
//! project-wide symbol table, so they're resolved here: after every file
//! has been extracted, each call candidate's callee name is looked up
//! against every known function/method name in the project. A call is only
//! resolved when the name is *unambiguous* (exactly one function/method
//! with that name project-wide) — resolving an ambiguous name by guessing
//! would risk drawing a wrong edge, so by default it's left unresolved
//! instead. [`analyze_with_cache_lsp`] can do better for an ambiguous call
//! by asking the project's real language server (`okf-lsp`) exactly which
//! definition that specific call site resolves to — real type/scope
//! resolution Tree-sitter's own name-matching has no way to approximate.
//!
//! Per-file extraction (the expensive tree-sitter parse) can be skipped
//! for files that haven't changed since a previous run — see
//! [`analyze_with_cache`] and [`AnalysisCache`].

mod cache;
mod lsp;

pub use cache::AnalysisCache;

use anyhow::{Context, Result};
use okf_core::{ManifestKind, Project};
use okf_parser::{
    Concept, ConceptKind, Confidence, Language, Location, RelationKind, Relationship,
};
use okf_tree_sitter::CallCandidate;
use rayon::prelude::*;
use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};

/// The full semantic model for an analyzed project: every extracted
/// concept (including one `Package` concept per manifest discovered in
/// the project — see [`okf_core::Project::packages`] — for a
/// multi-package workspace or monorepo, that's more than one), with
/// import and call relationships already attached.
#[derive(Debug, Clone)]
pub struct AnalysisResult {
    pub root: PathBuf,
    pub concepts: Vec<Concept>,
}

/// Counts of files reused from the cache vs. freshly re-parsed by one
/// [`analyze_with_cache`] run, so a caller (e.g. `okf-rs generate`) can
/// report how much work incremental indexing actually saved.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct IncrementalStats {
    pub reused: usize,
    pub reparsed: usize,
}

/// A `Calls` edge resolved by either path — Tree-sitter's own unambiguous
/// name match, or (with `--lsp`) a real language server confirming which
/// definition an ambiguous call site resolves to — carrying enough
/// provenance to build both directions' [`Relationship`]s from. Shared
/// between the main resolution loop in this module and `lsp`, so an
/// LSP-resolved edge's real resolver name and [`Confidence::Semantic`]
/// survive all the way to the `Relationship`s pushed onto the two
/// concepts involved, instead of being flattened into an undifferentiated
/// `(caller, callee)` pair partway through.
pub(crate) struct ResolvedEdge {
    pub(crate) caller: String,
    pub(crate) callee: String,
    pub(crate) resolved_by: String,
    pub(crate) confidence: Confidence,
    /// The resolver's own reported version, when `resolved_by` names a
    /// real language server that reported one — see
    /// `okf_lsp::LspClient::server_version`. Always `None` for a
    /// `tree-sitter`-resolved edge.
    pub(crate) resolver_version: Option<String>,
}

/// Scans and analyzes `project`, producing the full concept + relationship
/// set. Deterministic: running this twice over unchanged source produces
/// byte-identical results (no wall-clock timestamps, no unordered maps
/// affecting output order).
pub fn analyze(project: &Project) -> Result<AnalysisResult> {
    let mut cache = AnalysisCache::default();
    Ok(analyze_with_cache(project, &mut cache)?.0)
}

/// Like [`analyze`], but skips re-parsing any file whose content hash
/// matches an entry already in `cache`, reusing that entry's extraction
/// instead — the cache hit produces exactly the same result a fresh parse
/// of that unchanged file would have, so this is a pure performance
/// optimization, never a source of different output.
///
/// `cache` is replaced with a fresh cache reflecting exactly this run's
/// file set on return: entries for files no longer in the project are
/// dropped rather than left to accumulate. Callers that want the cache to
/// persist across process invocations (e.g. `okf-rs generate`, re-run as
/// source changes during local development) are responsible for loading
/// it beforehand and saving it afterward with [`AnalysisCache::load`]/
/// [`AnalysisCache::save`].
pub fn analyze_with_cache(
    project: &Project,
    cache: &mut AnalysisCache,
) -> Result<(AnalysisResult, IncrementalStats)> {
    analyze_with_cache_lsp(project, cache, false)
}

/// Like [`analyze_with_cache`], but when `use_lsp` is true, also attempts
/// to resolve calls whose callee name matches more than one candidate
/// project-wide by asking that call site's real language server exactly
/// which definition it resolves to (`textDocument/definition`, via
/// `okf-lsp`) — real type/scope resolution Tree-sitter's own name-matching
/// has no way to approximate. Entirely additive and best-effort: a
/// language with no available server, or a call the server can't answer
/// for, is simply left unresolved exactly as [`analyze_with_cache`] (which
/// passes `use_lsp: false`) always leaves it — this can only resolve
/// *more* edges than the base algorithm, never fewer or different ones.
/// Spawning and querying real language server processes makes this
/// meaningfully slower than [`analyze_with_cache`]; it's opt-in for that
/// reason, not just because a server may be unavailable.
pub fn analyze_with_cache_lsp(
    project: &Project,
    cache: &mut AnalysisCache,
    use_lsp: bool,
) -> Result<(AnalysisResult, IncrementalStats)> {
    let mut concepts = detect_packages(project)?;

    let mut calls: Vec<(CallCandidate, Language, String)> = Vec::new();
    let mut stats = IncrementalStats::default();
    let mut fresh_cache = AnalysisCache::default();
    // Only populated when `use_lsp` is set, so a plain `generate` doesn't
    // pay the extra memory of retaining every file's source text: lets
    // `resolve_ambiguous_calls` reuse the source already read here instead
    // of reading each file a second time itself.
    let mut file_sources: HashMap<String, String> = HashMap::new();

    // The expensive part -- reading and (on a cache miss) tree-sitter
    // parsing each file -- is independent per file, so it runs across a
    // rayon thread pool; only the cache lookup (`AnalysisCache::get`,
    // `&self`, safe to call concurrently) happens inside the parallel
    // step. Everything that must stay sequential for deterministic output
    // (populating `fresh_cache`, appending to `calls`/`concepts` in file
    // order) happens in the merge loop below, over `.collect()`'s
    // input-order-preserving `Vec`, so output is byte-identical to the
    // sequential version regardless of which thread finished first.
    //
    // Collecting into `Result<Vec<_>, _>` (rather than `Vec<Result<_>>`
    // plus a manual first-error scan) uses rayon's own short-circuiting
    // `FromParallelIterator` impl for `Result`: once any item produces an
    // `Err`, rayon stops handing out further work to idle threads, so a
    // file that fails early (a permission error, a deleted file) no
    // longer guarantees every other file in the project gets read and
    // parsed first — closer to the old sequential loop's fail-fast
    // behavior than an unconditional "process everything, then report
    // the first error" would be.
    let file_results: Vec<FileParseResult> = project
        .files
        .par_iter()
        .map(|file| -> Result<FileParseResult> {
            let source = fs::read_to_string(&file.absolute_path)
                .with_context(|| format!("failed to read {}", file.relative_path))?;
            let hash = cache::hash_content(&source);

            let (extraction, reused) = match cache.get(&file.relative_path, hash) {
                Some(extraction) => (extraction, true),
                None => {
                    let extraction = okf_tree_sitter::extract_source(&source, file)
                        .with_context(|| format!("failed to analyze {}", file.relative_path))?;
                    (extraction, false)
                }
            };
            Ok(FileParseResult {
                relative_path: file.relative_path.clone(),
                language: file.language,
                hash,
                extraction,
                reused,
                source: if use_lsp { Some(source) } else { None },
            })
        })
        .collect::<Result<Vec<_>>>()?;

    for result in file_results {
        if result.reused {
            stats.reused += 1;
        } else {
            stats.reparsed += 1;
        }
        fresh_cache.insert(
            &result.relative_path,
            result.hash,
            result.extraction.clone(),
        );
        for call in result.extraction.calls {
            calls.push((call, result.language, result.relative_path.clone()));
        }
        concepts.extend(result.extraction.concepts);
        if let Some(source) = result.source {
            file_sources.insert(result.relative_path, source);
        }
    }
    *cache = fresh_cache;

    // Must happen before anything below indexes concepts by id (the
    // `index_of` map built after call resolution, in particular): two
    // concepts sharing an id — e.g. a `#[cfg(feature = "x")]` /
    // `#[cfg(not(feature = "x"))]` stub pair — would otherwise stay
    // indistinguishable there, and a resolved Calls/CalledBy edge would
    // get attributed to an arbitrary one of the pair instead of the one
    // whose body actually made the call. See `Concept::disambiguate_ids`.
    Concept::disambiguate_ids(&mut concepts);

    link_modules_to_packages(&mut concepts);

    let mut symbol_table: HashMap<&str, Vec<&str>> = HashMap::new();
    for concept in &concepts {
        if matches!(concept.kind, ConceptKind::Function | ConceptKind::Method) {
            symbol_table
                .entry(concept.name.as_str())
                .or_default()
                .push(concept.id.as_str());
        }
    }

    let mut resolved_edges: Vec<ResolvedEdge> = Vec::new();
    let mut ambiguous: Vec<&(CallCandidate, Language, String)> = Vec::new();
    for entry in &calls {
        let (call, _, _) = entry;
        let Some(candidates) = symbol_table.get(call.callee_name.as_str()) else {
            continue;
        };
        if candidates.len() != 1 {
            if use_lsp {
                ambiguous.push(entry);
            }
            continue;
        }
        let callee_id = candidates[0].to_string();
        if callee_id != call.caller_id {
            resolved_edges.push(ResolvedEdge {
                caller: call.caller_id.clone(),
                callee: callee_id,
                resolved_by: "tree-sitter".to_string(),
                confidence: Confidence::Exact,
                resolver_version: None,
            });
        }
    }

    if use_lsp && !ambiguous.is_empty() {
        let id_to_location: HashMap<&str, &Location> = concepts
            .iter()
            .map(|c| (c.id.as_str(), &c.location))
            .collect();
        lsp::resolve_ambiguous_calls(
            project,
            &ambiguous,
            &file_sources,
            &id_to_location,
            &symbol_table,
            &mut resolved_edges,
        );
    }

    let index_of: HashMap<String, usize> = concepts
        .iter()
        .enumerate()
        .map(|(i, c)| (c.id.clone(), i))
        .collect();

    for edge in resolved_edges {
        let (Some(&caller_idx), Some(&callee_idx)) =
            (index_of.get(&edge.caller), index_of.get(&edge.callee))
        else {
            continue;
        };
        let caller_name = concepts[caller_idx].name.clone();
        let callee_name = concepts[callee_idx].name.clone();

        concepts[caller_idx].relationships.push(Relationship {
            kind: RelationKind::Calls,
            target: edge.callee.clone(),
            target_display: callee_name,
            resolved_by: edge.resolved_by.clone(),
            confidence: edge.confidence,
            resolver_version: edge.resolver_version.clone(),
        });
        concepts[callee_idx].relationships.push(Relationship {
            kind: RelationKind::CalledBy,
            target: edge.caller.clone(),
            target_display: caller_name,
            resolved_by: edge.resolved_by,
            confidence: edge.confidence,
            resolver_version: edge.resolver_version,
        });
    }

    Ok((
        AnalysisResult {
            root: project.root.clone(),
            concepts,
        },
        stats,
    ))
}

/// One file's read+extract outcome, produced in parallel by
/// [`analyze_with_cache_lsp`]'s per-file rayon step and merged back
/// sequentially afterward.
struct FileParseResult {
    relative_path: String,
    language: Language,
    hash: u64,
    extraction: okf_tree_sitter::FileExtraction,
    reused: bool,
    source: Option<String>,
}

/// A concept present in both snapshots being diffed, but whose signature
/// or relationships changed between them.
#[derive(Debug, Clone)]
pub struct ChangedConcept {
    pub id: String,
    pub kind: ConceptKind,
    pub before_signature: Option<String>,
    pub after_signature: Option<String>,
    /// Per-relationship detail behind this concept's `Changed` status,
    /// one entry per `(kind, target)` pair that actually differs between
    /// `before`/`after` — see [`diff_relationships`] and
    /// [`RelationshipChangeKind`]. Empty when the concept's relationships
    /// are identical (a signature-only change), never includes a pair
    /// classified [`RelationshipChangeKind::Unchanged`]. Purely additive
    /// detail: nothing about `id`/`kind`/`before_signature`/
    /// `after_signature` above, or the top-level `ChangeKind` a caller
    /// sees this concept under, depends on it.
    pub relationship_changes: Vec<(RelationKind, String, RelationshipChangeKind)>,
}

/// How one `(kind, target)` relationship pair changed between two
/// snapshots of the same concept — the provenance-aware detail
/// `okf-rs diff --ci` (see `ROADMAP.md`) classifies as a source-level
/// failure, a resolver-level warning, or a confidence-level note.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RelationshipChangeKind {
    /// Same `(kind, target)` pair on both sides, with identical
    /// `resolved_by`/`confidence`/`resolver_version` too — no change at
    /// all. Computed but never stored in [`ChangedConcept::relationship_changes`];
    /// only reachable by calling [`diff_relationships`] directly.
    Unchanged,
    /// The pair exists on only one side (added or removed), or the
    /// target/kind itself differs — a real structural rewire, regardless
    /// of what produced either side. Always a `--ci` failure.
    SourceChange,
    /// Same `(kind, target)` pair on both sides; `resolved_by` and/or
    /// `resolver_version` differ, `confidence` does not — the pure "same
    /// tool, different version" case (or a different resolver entirely,
    /// still at the same confidence level).
    ResolverChange,
    /// Same `(kind, target)` pair on both sides; `confidence` differs,
    /// `resolved_by`/`resolver_version` do not.
    ConfidenceChange,
    /// Same `(kind, target)` pair on both sides; `resolved_by`/
    /// `resolver_version` *and* `confidence` both differ together — the
    /// shape a source-level change *elsewhere* in the project produces
    /// (a previously-unambiguous call becoming ambiguous, or vice versa,
    /// flips a call between `tree-sitter`/`exact` and a real
    /// resolver/`semantic` even though this edge's own target never
    /// moved). Deliberately distinct from `ResolverChange`/
    /// `ConfidenceChange` rather than folded into either — two fields
    /// moved together, not one.
    ProvenanceChange,
}

/// The result of comparing two analyzed snapshots of the same project
/// (typically two git refs) at the concept level.
#[derive(Debug, Clone, Default)]
pub struct DiffReport {
    pub added: Vec<Concept>,
    pub removed: Vec<Concept>,
    pub changed: Vec<ChangedConcept>,
}

impl DiffReport {
    pub fn is_empty(&self) -> bool {
        self.added.is_empty() && self.removed.is_empty() && self.changed.is_empty()
    }
}

/// Compares two concept snapshots by id: concepts only in `after` are
/// additions, concepts only in `before` are removals, and concepts in both
/// whose signature or relationship set differs are changes. Pure and
/// git-agnostic — the caller is responsible for producing `before`/`after`
/// (e.g. by analyzing two git refs), which keeps this testable without a
/// repository.
pub fn diff(before: &[Concept], after: &[Concept]) -> DiffReport {
    let before_by_id: HashMap<&str, &Concept> = before.iter().map(|c| (c.id.as_str(), c)).collect();
    let after_by_id: HashMap<&str, &Concept> = after.iter().map(|c| (c.id.as_str(), c)).collect();

    let mut report = DiffReport::default();

    for concept in after {
        if !before_by_id.contains_key(concept.id.as_str()) {
            report.added.push(concept.clone());
        }
    }
    for concept in before {
        if !after_by_id.contains_key(concept.id.as_str()) {
            report.removed.push(concept.clone());
        }
    }
    for concept in after {
        let Some(&before_concept) = before_by_id.get(concept.id.as_str()) else {
            continue;
        };
        let relationship_changes = diff_relationships(before_concept, concept);
        // A provenance-only change (e.g. a resolver-version bump with the
        // same target) previously went undetected entirely: comparing
        // just the (kind, target) *set* — this project's earlier
        // determinism-focused equality check — can't see it, since that
        // set is identical on both sides. `diff_relationships` sees it
        // (a non-`Unchanged` classification for that pair), so gating on
        // its emptiness rather than a separate set-equality check is what
        // actually closes that gap, not just what classifies it once
        // caught.
        if before_concept.signature != concept.signature || !relationship_changes.is_empty() {
            report.changed.push(ChangedConcept {
                id: concept.id.clone(),
                kind: concept.kind,
                before_signature: before_concept.signature.clone(),
                after_signature: concept.signature.clone(),
                relationship_changes,
            });
        }
    }

    report.added.sort_by(|a, b| a.id.cmp(&b.id));
    report.removed.sort_by(|a, b| a.id.cmp(&b.id));
    report.changed.sort_by(|a, b| a.id.cmp(&b.id));
    report
}

/// The three counts `okf-rs diff --ci` (see `ROADMAP.md`) classifies a
/// [`DiffReport`] into and evaluates against `okf.toml`'s `[diff]`
/// policy. Pure aggregation with no policy awareness of its own — an
/// exit code is a `--ci`-flag concern, computed by the caller from these
/// counts plus its own `okf_core::config::DiffPolicy` (this crate doesn't
/// depend on `okf-core`'s config module, and doesn't need to: the same
/// counts could just as easily drive a different policy, or none).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct CiSummary {
    /// A concept added or removed (counted as `relationships.len()`, or
    /// `1` for a concept with none — its own existence is the change),
    /// a `Changed` concept's signature differing, or a
    /// [`RelationshipChangeKind::SourceChange`] relationship pair.
    /// Always CI-failure-worthy — never gated by policy.
    pub source_changes: usize,
    /// [`RelationshipChangeKind::ResolverChange`] and
    /// [`RelationshipChangeKind::ProvenanceChange`] pairs — "the resolver
    /// that produced this edge changed identity and/or version" (a
    /// `ProvenanceChange` also touches confidence, but it's still
    /// fundamentally a resolver-provenance change, not a second category
    /// of its own — see `docs/improvement-plan-provenance-diff.md`).
    pub resolver_changes: usize,
    /// [`RelationshipChangeKind::ConfidenceChange`] pairs — same
    /// resolver/version, same target, only the confidence level differs.
    pub confidence_changes: usize,
}

impl CiSummary {
    /// No changes in any category — `okf-rs diff --ci` reports "no
    /// changes" and always exits `0` in this case, regardless of policy.
    pub fn is_empty(&self) -> bool {
        self.source_changes == 0 && self.resolver_changes == 0 && self.confidence_changes == 0
    }

    /// The share of all relationship-level changes in this diff that were
    /// `resolver_changes` alone — the empirical number
    /// `docs/improvement-plan-provenance-diff.md`'s Phase G asks a project
    /// to watch on its own corpus: external review argued that if this
    /// rate stays near zero across a real resolver-version bump,
    /// resolver-class changes can safely default to `ignore` in that
    /// project's own `okf.toml` (`DiffPolicy::resolver_changes`) instead
    /// of `warn`, and a rate that *isn't* near zero is itself a resolver
    /// finding worth reporting upstream. `None` when there's no
    /// relationship-level change to take a rate over (an empty diff, or
    /// one containing only whole-concept adds/removes).
    ///
    /// Most informative computed over a diff whose two sides are the exact
    /// same source re-analyzed with two different resolver versions (no
    /// concept-level adds/removes at all, matching the fixture Phase A's
    /// own plan describes) — `source_changes` also counts a concept's own
    /// existence changing, which a diff spanning real commits mixes in
    /// alongside relationship-pair rewires, understating this rate for
    /// reasons that have nothing to do with resolver behavior.
    pub fn resolver_only_rate(&self) -> Option<f64> {
        let total = self.source_changes + self.resolver_changes + self.confidence_changes;
        if total == 0 {
            return None;
        }
        Some(self.resolver_changes as f64 / total as f64)
    }
}

/// Reduces a [`DiffReport`] to the three counts [`CiSummary`] holds.
pub fn ci_summary(report: &DiffReport) -> CiSummary {
    let mut summary = CiSummary::default();

    for concept in report.added.iter().chain(report.removed.iter()) {
        // A concept with no relationships of its own still counts as 1:
        // the concept's own existence is the change, not an edge.
        summary.source_changes += concept.relationships.len().max(1);
    }
    for changed in &report.changed {
        if changed.before_signature != changed.after_signature {
            summary.source_changes += 1;
        }
        for (_, _, kind) in &changed.relationship_changes {
            match kind {
                RelationshipChangeKind::SourceChange => summary.source_changes += 1,
                RelationshipChangeKind::ResolverChange
                | RelationshipChangeKind::ProvenanceChange => summary.resolver_changes += 1,
                RelationshipChangeKind::ConfidenceChange => summary.confidence_changes += 1,
                // Never actually stored in `relationship_changes` (see
                // `diff_relationships`), but matched explicitly rather
                // than wildcarded so a future variant can't silently
                // fall through uncounted.
                RelationshipChangeKind::Unchanged => {}
            }
        }
    }
    summary
}

/// Classifies how each `(kind, target)` relationship pair differs between
/// two snapshots of the same concept — see [`RelationshipChangeKind`].
/// Order-independent (relationships are paired by key, not position, so
/// `diff` still treats a reordered-but-otherwise-identical relationship
/// list as unchanged) and duplicate-tolerant (a concept calling the same
/// target from multiple call sites collapses to one pair, first
/// occurrence wins — the same convention
/// `okf-generator::unique_by_kind_and_target` establishes for rendering).
/// Only pairs that actually changed are returned —
/// [`RelationshipChangeKind::Unchanged`] pairs are computed internally
/// but filtered out, so two concepts with identical relationships (a
/// signature-only change, say) yield an empty `Vec`. Sorted by
/// `(kind, target)` for deterministic output.
pub fn diff_relationships(
    before: &Concept,
    after: &Concept,
) -> Vec<(RelationKind, String, RelationshipChangeKind)> {
    let before_by_key = dedup_relationships(&before.relationships);
    let after_by_key = dedup_relationships(&after.relationships);

    let keys: std::collections::BTreeSet<(RelationKind, &str)> = before_by_key
        .keys()
        .chain(after_by_key.keys())
        .copied()
        .collect();

    let mut changes = Vec::new();
    for key in keys {
        let change = match (before_by_key.get(&key), after_by_key.get(&key)) {
            (Some(b), Some(a)) => classify_provenance_change(b, a),
            // Present on only one side: the pair itself was added or
            // removed — a real structural change regardless of what
            // produced either side.
            _ => RelationshipChangeKind::SourceChange,
        };
        if change != RelationshipChangeKind::Unchanged {
            changes.push((key.0, key.1.to_string(), change));
        }
    }
    changes
}

/// Reduces `relationships` to one representative per distinct
/// `(kind, target)` pair (first occurrence wins), keyed for lookup by
/// [`diff_relationships`].
fn dedup_relationships(
    relationships: &[Relationship],
) -> HashMap<(RelationKind, &str), &Relationship> {
    let mut map = HashMap::new();
    for rel in relationships {
        map.entry((rel.kind, rel.target.as_str())).or_insert(rel);
    }
    map
}

/// Classifies a same-`(kind, target)` pair present on both sides by which
/// provenance fields actually moved — see [`RelationshipChangeKind`]'s
/// variants for what each combination means.
fn classify_provenance_change(
    before: &Relationship,
    after: &Relationship,
) -> RelationshipChangeKind {
    let resolver_changed = before.resolved_by != after.resolved_by
        || before.resolver_version != after.resolver_version;
    let confidence_changed = before.confidence != after.confidence;
    match (resolver_changed, confidence_changed) {
        (false, false) => RelationshipChangeKind::Unchanged,
        (true, false) => RelationshipChangeKind::ResolverChange,
        (false, true) => RelationshipChangeKind::ConfidenceChange,
        (true, true) => RelationshipChangeKind::ProvenanceChange,
    }
}

/// How a concept changed between two analyzed snapshots — see [`impact`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChangeKind {
    Added,
    Removed,
    Changed,
}

/// One concept affected by a change between two snapshots (see
/// [`impact`]), plus the deterministic, structural signals it's scored
/// by: how much of the bundle transitively depends on it, whether it's
/// public API, and whether it participates in a call-graph cycle.
#[derive(Debug, Clone)]
pub struct ImpactedConcept {
    pub id: String,
    pub kind: ConceptKind,
    pub change: ChangeKind,
    /// Ids of every concept transitively affected if this one changes —
    /// its transitive callers (see
    /// [`okf_graph::Graph::transitive_callers`]), sorted for
    /// deterministic output. This is the "blast radius": the bigger it
    /// is, the more of the bundle a reviewer should check before trusting
    /// this change.
    pub blast_radius: Vec<String>,
    pub is_public_api: bool,
    pub in_cycle: bool,
}

/// The result of [`impact`]: the underlying concept-level [`DiffReport`]
/// plus every added/removed/changed concept's blast radius and
/// structural criticality.
#[derive(Debug, Clone, Default)]
pub struct ImpactReport {
    pub diff: DiffReport,
    /// Sorted by blast radius size, descending (ties broken by id), so
    /// the highest-risk change — the one the most other code transitively
    /// depends on — is first, the same "what's most worth reviewing
    /// first" question a PR review would ask.
    pub impacted: Vec<ImpactedConcept>,
}

/// Change-impact ("blast radius") analysis between two analyzed
/// snapshots of the same project (typically two git refs — the same
/// `before`/`after` shape [`diff`] takes): for every added, removed, or
/// changed concept, who transitively depends on it, whether it's public
/// API, and whether it sits in a call-graph cycle.
///
/// Deliberately scored by structural signals already present in the
/// graph — caller-reachability, public-API membership, cycle membership
/// — rather than a model-inferred risk judgment: this stays inside
/// okf-rs's "deterministic core, AI layered on top only as an optional,
/// separate step" design (the same posture `okf-enrich`'s optional `--enrich`
/// already has relative to plain `generate`), so `impact` needs no
/// network access and produces the same report for the same two refs
/// every time.
///
/// A removed concept's blast radius/criticality is computed against
/// `before` — the only snapshot that still has it, and the graph its
/// former callers actually lived in; an added or changed concept's is
/// computed against `after`, the snapshot it currently exists in.
/// One of [`impact`]'s three scoring groups (added/removed/changed): the
/// ids to score, tagged with the [`ChangeKind`] and the graph/public-API/
/// cycle-membership triple to score them against.
struct ImpactGroup<'a> {
    ids: Vec<&'a str>,
    change: ChangeKind,
    graph: &'a okf_graph::Graph<'a>,
    public: &'a HashSet<&'a str>,
    cyclic: &'a HashSet<&'a str>,
}

pub fn impact(before: &[Concept], after: &[Concept]) -> ImpactReport {
    let diff_report = diff(before, after);
    let before_graph = okf_graph::Graph::build(before);
    let after_graph = okf_graph::Graph::build(after);

    let before_public: HashSet<&str> = before_graph
        .public_api()
        .iter()
        .map(|c| c.id.as_str())
        .collect();
    let after_public: HashSet<&str> = after_graph
        .public_api()
        .iter()
        .map(|c| c.id.as_str())
        .collect();
    let before_cyclic: HashSet<&str> = before_graph.cycles().into_iter().flatten().collect();
    let after_cyclic: HashSet<&str> = after_graph.cycles().into_iter().flatten().collect();

    // Each group scores its ids against the graph/public-API/cycle triple
    // those ids actually belong to: a removed concept only still exists
    // in `before` (and its former callers only live in that graph too);
    // an added or changed concept exists in `after`. One loop over these
    // three groups replaces what used to be three near-identical copies
    // of the same scoring loop.
    let groups = [
        ImpactGroup {
            ids: diff_report.added.iter().map(|c| c.id.as_str()).collect(),
            change: ChangeKind::Added,
            graph: &after_graph,
            public: &after_public,
            cyclic: &after_cyclic,
        },
        ImpactGroup {
            ids: diff_report.removed.iter().map(|c| c.id.as_str()).collect(),
            change: ChangeKind::Removed,
            graph: &before_graph,
            public: &before_public,
            cyclic: &before_cyclic,
        },
        ImpactGroup {
            ids: diff_report.changed.iter().map(|c| c.id.as_str()).collect(),
            change: ChangeKind::Changed,
            graph: &after_graph,
            public: &after_public,
            cyclic: &after_cyclic,
        },
    ];

    let mut impacted: Vec<ImpactedConcept> = Vec::new();
    for group in &groups {
        // Each id's blast radius is an independent graph traversal (no
        // shared mutable state), so scoring runs across a rayon thread
        // pool the same way per-file extraction does above -- a pure
        // wall-clock win on a diff that touches many concepts, not a
        // change in what's computed.
        impacted.par_extend(
            group
                .ids
                .par_iter()
                .map(|&id| score_impact(id, group.change, group.graph, group.public, group.cyclic)),
        );
    }

    impacted.sort_by(|a, b| {
        b.blast_radius
            .len()
            .cmp(&a.blast_radius.len())
            .then_with(|| a.id.cmp(&b.id))
    });

    ImpactReport {
        diff: diff_report,
        impacted,
    }
}

fn score_impact(
    id: &str,
    change: ChangeKind,
    graph: &okf_graph::Graph<'_>,
    public: &HashSet<&str>,
    cyclic: &HashSet<&str>,
) -> ImpactedConcept {
    // `id` always comes from a group scored against the graph that
    // concept actually belongs to (see `impact`), so this always finds
    // it -- deriving `kind` here instead of taking it as a separate
    // parameter means there's only ever one place a concept's kind comes
    // from, not two that could drift out of sync.
    let kind = graph
        .get(id)
        .expect("id passed to score_impact must belong to the graph it's scored against")
        .kind;
    let blast_radius: Vec<String> = graph
        .transitive_callers(id, None)
        .iter()
        .map(|c| c.id.clone())
        .collect();
    ImpactedConcept {
        id: id.to_string(),
        kind,
        change,
        blast_radius,
        is_public_api: public.contains(id),
        in_cycle: cyclic.contains(id),
    }
}

/// Derives one `Package` concept per manifest discovered in the project
/// (see [`okf_core::Project::packages`]) — a single-package project gets
/// one, a multi-package workspace/monorepo gets one per member. A
/// manifest with no declared name (e.g. a Cargo workspace root with no
/// `[package]` table of its own — a "virtual manifest") is skipped: there
/// is nothing to name the concept after.
fn detect_packages(project: &Project) -> Result<Vec<Concept>> {
    let mut packages = Vec::new();

    // How many manifests share each directory -- almost always one, but a
    // directory can legitimately hold more than one manifest kind (e.g. a
    // Rust crate with an npm-based docs build alongside it), in which case
    // the directory alone no longer uniquely identifies a package below.
    let mut dir_counts: HashMap<&str, usize> = HashMap::new();
    for pkg_root in &project.packages {
        *dir_counts
            .entry(pkg_root.relative_dir.as_str())
            .or_default() += 1;
    }

    for pkg_root in &project.packages {
        let manifest_dir = if pkg_root.relative_dir.is_empty() {
            project.root.clone()
        } else {
            project.root.join(&pkg_root.relative_dir)
        };
        let manifest_path = manifest_dir.join(pkg_root.manifest.file_name());

        let language = match pkg_root.manifest {
            ManifestKind::Cargo => Language::Rust,
            ManifestKind::Npm => Language::JavaScript,
            ManifestKind::PyProject => Language::Python,
            ManifestKind::GoModule => Language::Go,
        };
        // A manifest that fails to read or parse is skipped rather than
        // aborting analysis of the whole project -- one malformed or
        // mid-edit manifest shouldn't take every other, valid package in
        // the workspace down with it.
        let name = match pkg_root.manifest {
            ManifestKind::Cargo => read_cargo_name(&manifest_path),
            ManifestKind::Npm => read_npm_name(&manifest_path),
            ManifestKind::PyProject => read_pyproject_name(&manifest_path),
            ManifestKind::GoModule => read_gomod_name(&manifest_path),
        };
        let name = match name {
            Ok(Some(name)) => name,
            Ok(None) => continue,
            Err(e) => {
                eprintln!(
                    "warning: skipping unreadable manifest {}: {e:#}",
                    manifest_path.display()
                );
                continue;
            }
        };

        // A single-package project keeps exactly the id it always had
        // (just the package name); a member of a multi-package workspace
        // is identified by its directory instead, since names alone
        // aren't guaranteed unique across ecosystems, but a filesystem
        // path always is -- unless more than one manifest kind shares
        // that directory, in which case the manifest kind is appended too
        // so the two don't collide on the same id.
        let qualified_name = if pkg_root.relative_dir.is_empty() {
            name.clone()
        } else {
            let dir_id = pkg_root.relative_dir.replace('/', ".");
            if dir_counts[pkg_root.relative_dir.as_str()] > 1 {
                format!("{dir_id}.{}", pkg_root.manifest.short_tag())
            } else {
                dir_id
            }
        };
        let file = if pkg_root.relative_dir.is_empty() {
            pkg_root.manifest.file_name().to_string()
        } else {
            format!(
                "{}/{}",
                pkg_root.relative_dir,
                pkg_root.manifest.file_name()
            )
        };

        packages.push(Concept {
            id: Concept::make_id(ConceptKind::Package, &qualified_name),
            kind: ConceptKind::Package,
            language,
            name,
            qualified_name,
            description: None,
            location: Location {
                file,
                start_line: 1,
                end_line: 1,
            },
            signature: None,
            tags: Vec::new(),
            is_public: true,
            generated_at: None,
            relationships: Vec::new(),
        });
    }
    Ok(packages)
}

fn read_cargo_name(manifest_path: &Path) -> Result<Option<String>> {
    let content = fs::read_to_string(manifest_path)?;
    let value: toml::Value = toml::from_str(&content).context("failed to parse Cargo.toml")?;
    Ok(value
        .get("package")
        .and_then(|p| p.get("name"))
        .and_then(|n| n.as_str())
        .map(str::to_string))
}

fn read_npm_name(manifest_path: &Path) -> Result<Option<String>> {
    let content = fs::read_to_string(manifest_path)?;
    let value: serde_json::Value =
        serde_json::from_str(&content).context("failed to parse package.json")?;
    Ok(value
        .get("name")
        .and_then(|n| n.as_str())
        .map(str::to_string))
}

fn read_pyproject_name(manifest_path: &Path) -> Result<Option<String>> {
    let content = fs::read_to_string(manifest_path)?;
    let value: toml::Value = toml::from_str(&content).context("failed to parse pyproject.toml")?;
    let from_project = value
        .get("project")
        .and_then(|p| p.get("name"))
        .and_then(|n| n.as_str());
    let from_poetry = value
        .get("tool")
        .and_then(|t| t.get("poetry"))
        .and_then(|p| p.get("name"))
        .and_then(|n| n.as_str());
    Ok(from_project.or(from_poetry).map(str::to_string))
}

fn read_gomod_name(manifest_path: &Path) -> Result<Option<String>> {
    let content = fs::read_to_string(manifest_path)?;
    Ok(content
        .lines()
        .find_map(|line| line.trim().strip_prefix("module "))
        .map(|m| m.trim().to_string()))
}

/// Attaches a `MemberOf` relationship from each `Module` concept to the
/// `Package` concept that owns it: the package whose directory is the
/// longest (most specific) prefix of the module's file, so a nested
/// member package wins over an ancestor workspace-root package that's
/// also, itself, a named package. A module under no detected package
/// (e.g. loose files outside any manifest's directory) gets no
/// relationship at all, rather than a guessed one.
fn link_modules_to_packages(concepts: &mut [Concept]) {
    let mut packages: Vec<(String, String, String)> = concepts
        .iter()
        .filter(|c| c.kind == ConceptKind::Package)
        .map(|c| {
            (
                package_directory(&c.location.file).to_string(),
                c.id.clone(),
                c.name.clone(),
            )
        })
        .collect();
    if packages.is_empty() {
        return;
    }
    packages.sort_by_key(|(dir, _, _)| std::cmp::Reverse(dir.len()));

    for concept in concepts.iter_mut() {
        if concept.kind != ConceptKind::Module {
            continue;
        }
        let file = concept.location.file.as_str();
        let owner = packages.iter().find(|(dir, _, _)| {
            dir.is_empty() || file == dir || file.starts_with(&format!("{dir}/"))
        });
        if let Some((_, package_id, package_name)) = owner {
            concept.relationships.push(Relationship::new(
                RelationKind::MemberOf,
                package_id.clone(),
                package_name.clone(),
            ));
        }
    }
}

/// Reverses the `file` a `Package` concept's `location` was built with
/// (`<dir>/<manifest file name>`, or just the manifest file name at the
/// project root) back into that package's directory.
fn package_directory(file: &str) -> &str {
    file.rsplit_once('/').map(|(dir, _)| dir).unwrap_or("")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn resolves_unambiguous_calls_and_detects_package() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(
            dir.path().join("Cargo.toml"),
            "[package]\nname = \"demo\"\nversion = \"0.1.0\"\n",
        )
        .unwrap();
        fs::create_dir_all(dir.path().join("src")).unwrap();
        fs::write(
            dir.path().join("src/lib.rs"),
            "pub fn verify_token(t: &str) -> bool { decode_jwt(t) }\n\npub fn decode_jwt(t: &str) -> bool { true }\n",
        )
        .unwrap();

        let project = Project::load(dir.path()).unwrap();
        let result = analyze(&project).unwrap();

        let package = result
            .concepts
            .iter()
            .find(|c| c.kind == ConceptKind::Package)
            .unwrap();
        assert_eq!(package.name, "demo");

        let verify = result
            .concepts
            .iter()
            .find(|c| c.name == "verify_token")
            .unwrap();
        let calls_decode = verify
            .relationships
            .iter()
            .find(|r| r.kind == RelationKind::Calls && r.target_display == "decode_jwt")
            .unwrap();
        // Resolved by name-unambiguous Tree-sitter matching, not `--lsp`
        // (never passed here) -- exact confidence, tree-sitter provenance.
        assert_eq!(calls_decode.resolved_by, "tree-sitter");
        assert_eq!(calls_decode.confidence, Confidence::Exact);

        let decode = result
            .concepts
            .iter()
            .find(|c| c.name == "decode_jwt")
            .unwrap();
        let called_by_verify = decode
            .relationships
            .iter()
            .find(|r| r.kind == RelationKind::CalledBy && r.target_display == "verify_token")
            .unwrap();
        assert_eq!(called_by_verify.resolved_by, "tree-sitter");
        assert_eq!(called_by_verify.confidence, Confidence::Exact);
    }

    #[test]
    fn does_not_resolve_ambiguous_call_names() {
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir_all(dir.path().join("src")).unwrap();
        fs::write(
            dir.path().join("src/a.rs"),
            "pub fn run() {} pub fn caller() { run(); }",
        )
        .unwrap();
        fs::write(dir.path().join("src/b.rs"), "pub fn run() {}").unwrap();

        let project = Project::load(dir.path()).unwrap();
        let result = analyze(&project).unwrap();

        let caller = result.concepts.iter().find(|c| c.name == "caller").unwrap();
        assert!(!caller
            .relationships
            .iter()
            .any(|r| r.kind == RelationKind::Calls));
    }

    #[test]
    fn attributes_a_resolved_call_to_the_right_concept_when_two_share_one_id() {
        // Mirrors a `#[cfg(feature = "x")]` / `#[cfg(not(feature = "x"))]`
        // stub pair: two functions with the same name in the same file,
        // only one of which actually calls anything. Before concepts are
        // disambiguated up front, both shared the pre-disambiguation id
        // `functions/src/a/run`, so the id-to-index map built for call
        // resolution collapsed them to whichever was last in `concepts`
        // — attributing the `Calls`/`CalledBy` edge to an arbitrary one
        // of the pair instead of the one whose body made the call.
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir_all(dir.path().join("src")).unwrap();
        fs::write(
            dir.path().join("src/a.rs"),
            "pub fn helper() {}\n\npub fn run() { helper(); }\n\npub fn run() {}\n",
        )
        .unwrap();

        let project = Project::load(dir.path()).unwrap();
        let result = analyze(&project).unwrap();

        let runs: Vec<_> = result.concepts.iter().filter(|c| c.name == "run").collect();
        assert_eq!(runs.len(), 2, "both same-name concepts must survive");
        assert_ne!(
            runs[0].id, runs[1].id,
            "same-file same-name concepts must get distinct ids"
        );

        let caller = runs
            .iter()
            .find(|c| c.location.start_line == 3)
            .expect("the calling `run` is on line 3");
        let stub = runs
            .iter()
            .find(|c| c.location.start_line == 5)
            .expect("the empty `run` is on line 5");

        assert!(
            caller
                .relationships
                .iter()
                .any(|r| r.kind == RelationKind::Calls && r.target_display == "helper"),
            "the run that actually calls helper() must carry the Calls edge"
        );
        assert!(
            !stub
                .relationships
                .iter()
                .any(|r| r.kind == RelationKind::Calls),
            "the empty run must not inherit the other run's Calls edge"
        );

        let helper = result.concepts.iter().find(|c| c.name == "helper").unwrap();
        assert_eq!(
            helper
                .relationships
                .iter()
                .filter(|r| r.kind == RelationKind::CalledBy)
                .count(),
            1,
            "helper must be called-by exactly the caller run, not both"
        );
    }

    /// The same ambiguous-by-name shape as `does_not_resolve_ambiguous_call_names`
    /// (two `run` functions, one bare `run()` call), but this time real
    /// Rust scoping makes the call genuinely unambiguous: `caller` lives in
    /// module `a`, which never imports `b::run`, so a real compiler (and
    /// `rust-analyzer`) resolves the bare call to `a::run` specifically.
    /// `use_lsp: true` should recover that edge; skipped, not failed, when
    /// `rust-analyzer` isn't installed.
    #[test]
    fn resolves_an_ambiguous_call_via_a_real_rust_analyzer_when_scoping_disambiguates_it() {
        if !okf_lsp::is_available(Language::Rust) {
            eprintln!("skipping: rust-analyzer not installed");
            return;
        }

        let dir = tempfile::tempdir().unwrap();
        fs::write(
            dir.path().join("Cargo.toml"),
            "[package]\nname = \"demo\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
        )
        .unwrap();
        fs::create_dir_all(dir.path().join("src")).unwrap();
        fs::write(dir.path().join("src/lib.rs"), "mod a;\nmod b;\n").unwrap();
        fs::write(
            dir.path().join("src/a.rs"),
            "pub fn run() {}\npub fn caller() { run(); }\n",
        )
        .unwrap();
        fs::write(dir.path().join("src/b.rs"), "pub fn run() {}\n").unwrap();

        let project = Project::load(dir.path()).unwrap();
        let mut cache = AnalysisCache::default();
        // `analyze_with_cache_lsp` itself retries a slow-to-index server
        // internally, so one call is enough here.
        let (result, _) = analyze_with_cache_lsp(&project, &mut cache, true).unwrap();

        let caller = result.concepts.iter().find(|c| c.name == "caller").unwrap();
        let call = caller
            .relationships
            .iter()
            .find(|r| r.kind == RelationKind::Calls)
            .expect("caller should have exactly one resolved Calls edge");
        assert_eq!(
            call.target, "functions/src/a/run",
            "should resolve specifically to a::run, not b::run"
        );
        // LSP-resolved: real resolver name (not "tree-sitter"), semantic
        // confidence, not exact -- confirms provenance survives all the
        // way from `lsp::resolve_ambiguous_calls` to the `Relationship`
        // actually attached to the concept, not just the default path.
        assert_eq!(call.resolved_by, "rust-analyzer");
        assert_eq!(call.confidence, Confidence::Semantic);
    }

    fn make_concept(id: &str, signature: &str) -> Concept {
        Concept {
            id: id.to_string(),
            kind: ConceptKind::Function,
            language: Language::Rust,
            name: id.to_string(),
            qualified_name: id.to_string(),
            description: None,
            location: Location {
                file: "src/lib.rs".to_string(),
                start_line: 1,
                end_line: 1,
            },
            signature: Some(signature.to_string()),
            tags: Vec::new(),
            is_public: true,
            generated_at: None,
            relationships: Vec::new(),
        }
    }

    #[test]
    fn diff_detects_added_removed_and_changed() {
        let before = vec![
            make_concept("functions/kept", "fn kept()"),
            make_concept("functions/removed", "fn removed()"),
            make_concept("functions/changed", "fn changed(a: i32)"),
        ];
        let after = vec![
            make_concept("functions/kept", "fn kept()"),
            make_concept("functions/added", "fn added()"),
            make_concept("functions/changed", "fn changed(a: i32, b: i32)"),
        ];

        let report = diff(&before, &after);

        assert_eq!(report.added.len(), 1);
        assert_eq!(report.added[0].id, "functions/added");

        assert_eq!(report.removed.len(), 1);
        assert_eq!(report.removed[0].id, "functions/removed");

        assert_eq!(report.changed.len(), 1);
        assert_eq!(report.changed[0].id, "functions/changed");
        assert_eq!(
            report.changed[0].before_signature.as_deref(),
            Some("fn changed(a: i32)")
        );
        assert_eq!(
            report.changed[0].after_signature.as_deref(),
            Some("fn changed(a: i32, b: i32)")
        );
    }

    #[test]
    fn diff_is_empty_for_identical_snapshots() {
        let concepts = vec![make_concept("functions/a", "fn a()")];
        let report = diff(&concepts, &concepts.clone());
        assert!(report.is_empty());
    }

    #[test]
    fn diff_ignores_relationship_order() {
        let mut before = make_concept("functions/a", "fn a()");
        before
            .relationships
            .push(Relationship::new(RelationKind::Calls, "functions/x", "x"));
        before
            .relationships
            .push(Relationship::new(RelationKind::Calls, "functions/y", "y"));

        let mut after = make_concept("functions/a", "fn a()");
        after
            .relationships
            .push(Relationship::new(RelationKind::Calls, "functions/y", "y"));
        after
            .relationships
            .push(Relationship::new(RelationKind::Calls, "functions/x", "x"));

        let report = diff(&[before], &[after]);
        assert!(
            report.is_empty(),
            "reordered relationships should not count as a change"
        );
    }

    /// Builds a `Relationship` with explicit provenance, for the
    /// `RelationshipChangeKind` scenarios below — `Relationship::new`
    /// always gives the tree-sitter/exact/no-version default, which isn't
    /// enough to construct an `--lsp`-resolved edge.
    fn provenance_relationship(
        kind: RelationKind,
        target: &str,
        resolved_by: &str,
        confidence: Confidence,
        resolver_version: Option<&str>,
    ) -> Relationship {
        Relationship {
            kind,
            target: target.to_string(),
            target_display: target.to_string(),
            resolved_by: resolved_by.to_string(),
            confidence,
            resolver_version: resolver_version.map(str::to_string),
        }
    }

    /// Scenario 1 (added edge): `A -> B` becomes `A -> B, A -> C`.
    #[test]
    fn diff_relationships_flags_an_added_edge_as_source_change() {
        let mut before = make_concept("functions/a", "fn a()");
        before
            .relationships
            .push(Relationship::new(RelationKind::Calls, "functions/b", "b"));

        let mut after = before.clone();
        after
            .relationships
            .push(Relationship::new(RelationKind::Calls, "functions/c", "c"));

        let changes = diff_relationships(&before, &after);
        assert_eq!(
            changes,
            vec![(
                RelationKind::Calls,
                "functions/c".to_string(),
                RelationshipChangeKind::SourceChange
            )]
        );
    }

    /// Scenario 2 (removed edge): inverse of scenario 1.
    #[test]
    fn diff_relationships_flags_a_removed_edge_as_source_change() {
        let mut before = make_concept("functions/a", "fn a()");
        before
            .relationships
            .push(Relationship::new(RelationKind::Calls, "functions/b", "b"));
        before
            .relationships
            .push(Relationship::new(RelationKind::Calls, "functions/c", "c"));

        let mut after = before.clone();
        after.relationships.retain(|r| r.target != "functions/c");

        let changes = diff_relationships(&before, &after);
        assert_eq!(
            changes,
            vec![(
                RelationKind::Calls,
                "functions/c".to_string(),
                RelationshipChangeKind::SourceChange
            )]
        );
    }

    /// Scenario 3 (source-level rewire, same resolver): `Foo -> Bar`
    /// (tree-sitter) becomes `Foo -> Baz` (tree-sitter) — two independent
    /// `SourceChange` entries, never conflated into one "resolver
    /// changed" report, since the target genuinely changed.
    #[test]
    fn diff_relationships_a_rewired_target_is_two_source_changes_not_a_resolver_change() {
        let mut before = make_concept("functions/foo", "fn foo()");
        before.relationships.push(Relationship::new(
            RelationKind::Calls,
            "functions/bar",
            "bar",
        ));

        let mut after = make_concept("functions/foo", "fn foo()");
        after.relationships.push(Relationship::new(
            RelationKind::Calls,
            "functions/baz",
            "baz",
        ));

        let mut changes = diff_relationships(&before, &after);
        changes.sort_by(|a, b| a.1.cmp(&b.1));
        assert_eq!(
            changes,
            vec![
                (
                    RelationKind::Calls,
                    "functions/bar".to_string(),
                    RelationshipChangeKind::SourceChange
                ),
                (
                    RelationKind::Calls,
                    "functions/baz".to_string(),
                    RelationshipChangeKind::SourceChange
                ),
            ]
        );
    }

    /// Scenario 4 (resolver-only change): `Foo -> Bar` (`rust-analyzer`
    /// 1.88) becomes `Foo -> Bar` (`rust-analyzer` 1.89) — exactly one
    /// `ResolverChange` entry, not a remove+add pair. This is the case
    /// that was invisible to `diff` entirely before this phase: the
    /// (kind, target) set never changes, so nothing before
    /// `diff_relationships` existed would have even noticed.
    #[test]
    fn diff_relationships_flags_a_resolver_version_bump_as_resolver_change() {
        let mut before = make_concept("functions/foo", "fn foo()");
        before.relationships.push(provenance_relationship(
            RelationKind::Calls,
            "functions/bar",
            "rust-analyzer",
            Confidence::Semantic,
            Some("1.88.0"),
        ));

        let mut after = make_concept("functions/foo", "fn foo()");
        after.relationships.push(provenance_relationship(
            RelationKind::Calls,
            "functions/bar",
            "rust-analyzer",
            Confidence::Semantic,
            Some("1.89.0"),
        ));

        let changes = diff_relationships(&before, &after);
        assert_eq!(
            changes,
            vec![(
                RelationKind::Calls,
                "functions/bar".to_string(),
                RelationshipChangeKind::ResolverChange
            )]
        );

        // And the top-level `diff` gate now actually catches this --
        // previously the (kind, target)-only equality check meant this
        // concept never made it into `report.changed` at all.
        let report = diff(&[before], &[after]);
        assert_eq!(report.changed.len(), 1);
        assert_eq!(report.changed[0].id, "functions/foo");
        assert_eq!(report.changed[0].relationship_changes.len(), 1);
    }

    /// Scenario 5 (resolver changes which target resolves): `Foo -> Bar`
    /// (`rust-analyzer` 1.88) becomes `Foo -> Baz` (`rust-analyzer`
    /// 1.89) — `SourceChange` for both the removed `Bar` and added `Baz`
    /// pairs, even though the resolver version also happens to differ; a
    /// diff that reported this as only a resolver change would hide a
    /// real call-graph rewire.
    #[test]
    fn diff_relationships_a_resolver_change_that_also_rewires_the_target_is_source_change() {
        let mut before = make_concept("functions/foo", "fn foo()");
        before.relationships.push(provenance_relationship(
            RelationKind::Calls,
            "functions/bar",
            "rust-analyzer",
            Confidence::Semantic,
            Some("1.88.0"),
        ));

        let mut after = make_concept("functions/foo", "fn foo()");
        after.relationships.push(provenance_relationship(
            RelationKind::Calls,
            "functions/baz",
            "rust-analyzer",
            Confidence::Semantic,
            Some("1.89.0"),
        ));

        let mut changes = diff_relationships(&before, &after);
        changes.sort_by(|a, b| a.1.cmp(&b.1));
        assert_eq!(
            changes,
            vec![
                (
                    RelationKind::Calls,
                    "functions/bar".to_string(),
                    RelationshipChangeKind::SourceChange
                ),
                (
                    RelationKind::Calls,
                    "functions/baz".to_string(),
                    RelationshipChangeKind::SourceChange
                ),
            ]
        );
    }

    /// Scenario 6 (ambiguity newly introduced, same real target):
    /// `Foo -> Bar` (`tree-sitter`/`exact`) becomes `Foo -> Bar`
    /// (`rust-analyzer`/`semantic`) — the shape a same-named function
    /// added elsewhere in the project produces: the target didn't move,
    /// but the call that used to be unambiguous now needs `--lsp` to
    /// resolve. Neither `ResolverChange` (confidence also moved) nor
    /// `ConfidenceChange` (resolved_by also moved) fits -- this is
    /// `ProvenanceChange`.
    #[test]
    fn diff_relationships_flags_a_combined_resolver_and_confidence_change_as_provenance_change() {
        let mut before = make_concept("functions/foo", "fn foo()");
        before.relationships.push(Relationship::new(
            RelationKind::Calls,
            "functions/bar",
            "bar",
        ));

        let mut after = make_concept("functions/foo", "fn foo()");
        after.relationships.push(provenance_relationship(
            RelationKind::Calls,
            "functions/bar",
            "rust-analyzer",
            Confidence::Semantic,
            Some("1.88.0"),
        ));

        let changes = diff_relationships(&before, &after);
        assert_eq!(
            changes,
            vec![(
                RelationKind::Calls,
                "functions/bar".to_string(),
                RelationshipChangeKind::ProvenanceChange
            )]
        );
    }

    #[test]
    fn diff_relationships_is_order_independent() {
        let mut before = make_concept("functions/a", "fn a()");
        before
            .relationships
            .push(Relationship::new(RelationKind::Calls, "functions/x", "x"));
        before
            .relationships
            .push(Relationship::new(RelationKind::Calls, "functions/y", "y"));

        let mut after = make_concept("functions/a", "fn a()");
        after
            .relationships
            .push(Relationship::new(RelationKind::Calls, "functions/y", "y"));
        after
            .relationships
            .push(Relationship::new(RelationKind::Calls, "functions/x", "x"));

        assert!(diff_relationships(&before, &after).is_empty());
    }

    #[test]
    fn a_signature_only_change_reports_empty_relationship_changes() {
        let mut before = make_concept("functions/a", "fn a()");
        before
            .relationships
            .push(Relationship::new(RelationKind::Calls, "functions/b", "b"));

        let mut after = make_concept("functions/a", "fn a(x: i32)");
        after
            .relationships
            .push(Relationship::new(RelationKind::Calls, "functions/b", "b"));

        let report = diff(&[before], &[after]);
        assert_eq!(report.changed.len(), 1);
        assert!(report.changed[0].relationship_changes.is_empty());
    }

    #[test]
    fn ci_summary_is_empty_for_no_changes() {
        let concepts = vec![make_concept("functions/a", "fn a()")];
        let report = diff(&concepts, &concepts.clone());
        assert!(ci_summary(&report).is_empty());
    }

    #[test]
    fn ci_summary_counts_a_signature_only_change_as_a_source_change() {
        // A pure signature change (no relationship difference at all)
        // is unambiguously source-level and must not slip through
        // uncounted just because `relationship_changes` is empty.
        let before = make_concept("functions/a", "fn a()");
        let after = make_concept("functions/a", "fn a(x: i32)");

        let report = diff(&[before], &[after]);
        let summary = ci_summary(&report);
        assert_eq!(summary.source_changes, 1);
        assert_eq!(summary.resolver_changes, 0);
        assert_eq!(summary.confidence_changes, 0);
    }

    #[test]
    fn ci_summary_counts_an_added_edge_as_one_source_change() {
        let before = make_concept("functions/a", "fn a()");
        let mut after = before.clone();
        after
            .relationships
            .push(Relationship::new(RelationKind::Calls, "functions/b", "b"));

        let report = diff(&[before], &[after]);
        let summary = ci_summary(&report);
        assert_eq!(summary.source_changes, 1);
        assert_eq!(summary.resolver_changes, 0);
    }

    #[test]
    fn ci_summary_counts_a_removed_edge_as_one_source_change() {
        let mut before = make_concept("functions/a", "fn a()");
        before
            .relationships
            .push(Relationship::new(RelationKind::Calls, "functions/b", "b"));
        let after = make_concept("functions/a", "fn a()");

        let report = diff(&[before], &[after]);
        let summary = ci_summary(&report);
        assert_eq!(summary.source_changes, 1);
    }

    #[test]
    fn ci_summary_counts_a_target_rewire_as_two_source_changes() {
        let mut before = make_concept("functions/foo", "fn foo()");
        before.relationships.push(Relationship::new(
            RelationKind::Calls,
            "functions/bar",
            "bar",
        ));
        let mut after = make_concept("functions/foo", "fn foo()");
        after.relationships.push(Relationship::new(
            RelationKind::Calls,
            "functions/baz",
            "baz",
        ));

        let report = diff(&[before], &[after]);
        let summary = ci_summary(&report);
        assert_eq!(summary.source_changes, 2);
        assert_eq!(summary.resolver_changes, 0);
    }

    #[test]
    fn ci_summary_counts_a_resolver_only_change_without_any_source_changes() {
        let mut before = make_concept("functions/foo", "fn foo()");
        before.relationships.push(provenance_relationship(
            RelationKind::Calls,
            "functions/bar",
            "rust-analyzer",
            Confidence::Semantic,
            Some("1.88.0"),
        ));
        let mut after = make_concept("functions/foo", "fn foo()");
        after.relationships.push(provenance_relationship(
            RelationKind::Calls,
            "functions/bar",
            "rust-analyzer",
            Confidence::Semantic,
            Some("1.89.0"),
        ));

        let report = diff(&[before], &[after]);
        let summary = ci_summary(&report);
        assert_eq!(summary.source_changes, 0);
        assert_eq!(summary.resolver_changes, 1);
        assert_eq!(summary.confidence_changes, 0);
        // The whole diff is one resolver-only pair -- 100% of it.
        assert_eq!(summary.resolver_only_rate(), Some(1.0));
    }

    #[test]
    fn resolver_only_rate_is_none_on_an_empty_summary() {
        assert_eq!(CiSummary::default().resolver_only_rate(), None);
    }

    /// A diff mixing one genuine source rewire with one resolver-only pair
    /// reports the resolver-only share of relationship-level changes, not
    /// just a bare "some resolver changes happened" boolean -- the number
    /// `docs/improvement-plan-provenance-diff.md`'s Phase G asks a project
    /// to watch across real resolver-version bumps on its own corpus.
    #[test]
    fn resolver_only_rate_reports_the_share_of_relationship_level_changes() {
        let mut before = make_concept("functions/foo", "fn foo()");
        before.relationships.push(Relationship::new(
            RelationKind::Calls,
            "functions/bar",
            "bar",
        ));
        before.relationships.push(provenance_relationship(
            RelationKind::Calls,
            "functions/baz",
            "rust-analyzer",
            Confidence::Semantic,
            Some("1.88.0"),
        ));
        let mut after = make_concept("functions/foo", "fn foo()");
        // functions/bar -> functions/qux: a genuine rewire (2 SourceChange
        // pairs, remove + add).
        after.relationships.push(Relationship::new(
            RelationKind::Calls,
            "functions/qux",
            "qux",
        ));
        // functions/baz unchanged except the resolver version bumped.
        after.relationships.push(provenance_relationship(
            RelationKind::Calls,
            "functions/baz",
            "rust-analyzer",
            Confidence::Semantic,
            Some("1.89.0"),
        ));

        let report = diff(&[before], &[after]);
        let summary = ci_summary(&report);
        assert_eq!(summary.source_changes, 2);
        assert_eq!(summary.resolver_changes, 1);
        // 1 resolver-only pair out of 3 total relationship-level changes.
        let rate = summary.resolver_only_rate().unwrap();
        assert!((rate - (1.0 / 3.0)).abs() < 1e-9);
    }

    #[test]
    fn ci_summary_a_combined_resolver_and_confidence_change_counts_as_resolver_not_confidence() {
        let mut before = make_concept("functions/foo", "fn foo()");
        before.relationships.push(Relationship::new(
            RelationKind::Calls,
            "functions/bar",
            "bar",
        ));
        let mut after = make_concept("functions/foo", "fn foo()");
        after.relationships.push(provenance_relationship(
            RelationKind::Calls,
            "functions/bar",
            "rust-analyzer",
            Confidence::Semantic,
            Some("1.88.0"),
        ));

        let report = diff(&[before], &[after]);
        let summary = ci_summary(&report);
        assert_eq!(summary.source_changes, 0);
        assert_eq!(
            summary.resolver_changes, 1,
            "ProvenanceChange counts as resolver_changes"
        );
        assert_eq!(summary.confidence_changes, 0);
    }

    #[test]
    fn ci_summary_mixed_source_and_resolver_changes_both_count() {
        let mut before = make_concept("functions/foo", "fn foo()");
        before.relationships.push(provenance_relationship(
            RelationKind::Calls,
            "functions/bar",
            "rust-analyzer",
            Confidence::Semantic,
            Some("1.88.0"),
        ));
        before
            .relationships
            .push(Relationship::new(RelationKind::Calls, "functions/x", "x"));

        let mut after = make_concept("functions/foo", "fn foo()");
        after.relationships.push(provenance_relationship(
            RelationKind::Calls,
            "functions/bar",
            "rust-analyzer",
            Confidence::Semantic,
            Some("1.89.0"),
        ));
        // functions/x is gone -- a real source-level removal.

        let report = diff(&[before], &[after]);
        let summary = ci_summary(&report);
        assert_eq!(summary.source_changes, 1);
        assert_eq!(summary.resolver_changes, 1);
    }

    /// This repository's root, derived from `okf-analyzer`'s own manifest
    /// directory — the same technique `okf-cli`'s e2e tests and
    /// `okf-parser`'s golden-fixture test use to find `tests/fixtures/`
    /// regardless of `cargo test`'s working directory.
    fn repo_root() -> std::path::PathBuf {
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .and_then(std::path::Path::parent)
            .expect("crates/okf-analyzer should be two levels below the repo root")
            .to_path_buf()
    }

    /// Phase F's golden diff fixtures (`tests/fixtures/diff/*/{before,after}`
    /// — see `docs/improvement-plan-provenance-diff.md`), each round-tripped
    /// through the real `okf_parser::read_bundle` path (not the in-memory
    /// `Concept` literals every other test in this file builds by hand) and
    /// classified by the real `diff`/`ci_summary` pipeline. This is the
    /// grep target the guard test right below checks against — every
    /// subdirectory name here must also appear in this function.
    #[test]
    fn golden_diff_fixtures_classify_correctly_through_read_bundle() {
        let fixtures = repo_root().join("tests/fixtures/diff");
        let load = |scenario: &str, side: &str| {
            okf_parser::read_bundle(&fixtures.join(scenario).join(side)).unwrap()
        };

        // unchanged: byte-identical before/after -- no changes at all.
        let before = load("unchanged", "before");
        let after = load("unchanged", "after");
        let report = diff(&before, &after);
        assert!(report.is_empty());
        assert!(ci_summary(&report).is_empty());

        // added-edge: a relationship added to an existing concept.
        let before = load("added-edge", "before");
        let after = load("added-edge", "after");
        let report = diff(&before, &after);
        assert_eq!(report.changed.len(), 1);
        let summary = ci_summary(&report);
        assert_eq!(summary.source_changes, 1);
        assert_eq!(summary.resolver_changes, 0);
        assert_eq!(summary.confidence_changes, 0);

        // removed-edge: the inverse of added-edge.
        let before = load("removed-edge", "before");
        let after = load("removed-edge", "after");
        let report = diff(&before, &after);
        assert_eq!(report.changed.len(), 1);
        let summary = ci_summary(&report);
        assert_eq!(summary.source_changes, 1);

        // resolver-change: same target, resolver_version bumps 1.88 -> 1.89.
        let before = load("resolver-change", "before");
        let after = load("resolver-change", "after");
        let report = diff(&before, &after);
        assert_eq!(report.changed.len(), 1);
        let summary = ci_summary(&report);
        assert_eq!(summary.source_changes, 0);
        assert_eq!(summary.resolver_changes, 1);

        // semantic-change: the target itself is rewired (bar -> baz), even
        // though the resolver version also happens to differ -- this must
        // stay a source change, not get absorbed into a resolver change.
        let before = load("semantic-change", "before");
        let after = load("semantic-change", "after");
        let report = diff(&before, &after);
        assert_eq!(report.changed.len(), 1);
        let summary = ci_summary(&report);
        assert_eq!(summary.source_changes, 2);
        assert_eq!(summary.resolver_changes, 0);
    }

    /// Guards against a fixture directory silently going stale: every
    /// subdirectory under `tests/fixtures/diff/` must be named in the
    /// test above, the same "grep-based, not a runtime check" enforcement
    /// `docs/improvement-plan-provenance-diff.md`'s Phase F specifies —
    /// an unused fixture is worse than none, since nothing else would
    /// catch it silently drifting out of sync with what it claims to
    /// demonstrate.
    #[test]
    fn every_diff_fixture_subdirectory_is_referenced_by_a_test() {
        let fixtures = repo_root().join("tests/fixtures/diff");
        let this_file = std::fs::read_to_string(file!())
            .or_else(|_| {
                std::fs::read_to_string(repo_root().join("crates/okf-analyzer/src/lib.rs"))
            })
            .expect("should be able to read this file's own source to grep it");

        let mut checked = 0;
        for entry in std::fs::read_dir(&fixtures).unwrap() {
            let entry = entry.unwrap();
            if !entry.file_type().unwrap().is_dir() {
                continue;
            }
            let name = entry.file_name().to_string_lossy().into_owned();
            assert!(
                this_file.contains(&format!("\"{name}\"")),
                "tests/fixtures/diff/{name} exists but no test in this file references it by name -- \
                 add it to golden_diff_fixtures_classify_correctly_through_read_bundle"
            );
            checked += 1;
        }
        assert_eq!(
            checked, 5,
            "expected exactly the 5 scenarios docs/improvement-plan-provenance-diff.md's Phase F names"
        );
    }

    #[test]
    fn ci_summary_an_added_concept_with_no_relationships_still_counts_as_one_source_change() {
        let before: Vec<Concept> = vec![];
        let after = vec![make_concept("functions/new", "fn new()")];

        let report = diff(&before, &after);
        let summary = ci_summary(&report);
        assert_eq!(summary.source_changes, 1);
    }

    #[test]
    fn ci_summary_an_added_concept_with_relationships_counts_one_per_relationship() {
        let before: Vec<Concept> = vec![];
        let mut new_concept = make_concept("functions/new", "fn new()");
        new_concept
            .relationships
            .push(Relationship::new(RelationKind::Calls, "functions/a", "a"));
        new_concept
            .relationships
            .push(Relationship::new(RelationKind::Calls, "functions/b", "b"));
        let after = vec![new_concept];

        let report = diff(&before, &after);
        let summary = ci_summary(&report);
        assert_eq!(summary.source_changes, 2);
    }

    fn concept_with_edges(
        id: &str,
        calls: &[&str],
        called_by: &[&str],
        is_public: bool,
    ) -> Concept {
        let mut c = make_concept(id, &format!("fn {id}()"));
        c.is_public = is_public;
        for target in calls {
            c.relationships
                .push(Relationship::new(RelationKind::Calls, *target, *target));
        }
        for source in called_by {
            c.relationships
                .push(Relationship::new(RelationKind::CalledBy, *source, *source));
        }
        c
    }

    #[test]
    fn impact_reports_the_blast_radius_of_a_changed_concept() {
        // c -> b -> a: changing `a` transitively affects b and c.
        let a = concept_with_edges("functions/a", &[], &["functions/b"], true);
        let b = concept_with_edges("functions/b", &["functions/a"], &["functions/c"], true);
        let c = concept_with_edges("functions/c", &["functions/b"], &[], true);
        let before = vec![a.clone(), b.clone(), c.clone()];

        let mut a_changed = a.clone();
        a_changed.signature = Some("fn a(x: i32)".to_string());
        let after = vec![a_changed, b, c];

        let report = impact(&before, &after);
        assert_eq!(report.diff.changed.len(), 1);
        assert_eq!(report.impacted.len(), 1);
        let impacted = &report.impacted[0];
        assert_eq!(impacted.id, "functions/a");
        assert_eq!(impacted.change, ChangeKind::Changed);
        assert_eq!(impacted.blast_radius, vec!["functions/b", "functions/c"]);
        assert!(impacted.is_public_api);
    }

    #[test]
    fn impact_scores_a_removed_concept_against_the_before_graph() {
        let removed = concept_with_edges("functions/removed", &[], &["functions/caller"], true);
        let caller = concept_with_edges("functions/caller", &["functions/removed"], &[], true);
        let before = vec![removed, caller.clone()];
        let after = vec![caller];

        let report = impact(&before, &after);
        assert_eq!(report.impacted.len(), 1);
        assert_eq!(report.impacted[0].id, "functions/removed");
        assert_eq!(report.impacted[0].change, ChangeKind::Removed);
        assert_eq!(
            report.impacted[0].blast_radius,
            vec!["functions/caller"],
            "a removed concept's blast radius must come from the graph it actually had callers in"
        );
    }

    #[test]
    fn impact_flags_public_api_and_cycle_membership() {
        let mut public_fn = concept_with_edges("functions/public", &[], &[], true);
        public_fn.is_public = true;
        let private_fn = concept_with_edges("functions/private", &[], &[], false);
        let mut recursive = concept_with_edges("functions/recursive", &[], &[], true);
        recursive.relationships.push(Relationship::new(
            RelationKind::Calls,
            "functions/recursive",
            "recursive",
        ));

        let before: Vec<Concept> = Vec::new();
        let after = vec![public_fn, private_fn, recursive];

        let report = impact(&before, &after);
        let by_id = |id: &str| report.impacted.iter().find(|c| c.id == id).unwrap();

        assert!(by_id("functions/public").is_public_api);
        assert!(!by_id("functions/private").is_public_api);
        assert!(by_id("functions/recursive").in_cycle);
        assert!(!by_id("functions/public").in_cycle);
    }

    #[test]
    fn impact_sorts_by_blast_radius_size_descending() {
        // `hub` has two callers, `leaf` has none -- `hub` must sort first.
        let hub = concept_with_edges("functions/hub", &[], &["functions/x", "functions/y"], true);
        let x = concept_with_edges("functions/x", &["functions/hub"], &[], true);
        let y = concept_with_edges("functions/y", &["functions/hub"], &[], true);
        let leaf = concept_with_edges("functions/leaf", &[], &[], true);

        let before: Vec<Concept> = Vec::new();
        let after = vec![hub, x, y, leaf];

        let report = impact(&before, &after);
        assert_eq!(report.impacted[0].id, "functions/hub");
        assert_eq!(report.impacted[0].blast_radius.len(), 2);
    }

    #[test]
    fn impact_is_empty_for_identical_snapshots() {
        let concepts = vec![make_concept("functions/a", "fn a()")];
        let report = impact(&concepts, &concepts.clone());
        assert!(report.diff.is_empty());
        assert!(report.impacted.is_empty());
    }

    /// Sorts by id so cache-hit and cache-miss runs (which may extract
    /// files in a different order relative to each other) compare equal.
    fn sorted_ids(concepts: &[Concept]) -> Vec<&str> {
        let mut ids: Vec<&str> = concepts.iter().map(|c| c.id.as_str()).collect();
        ids.sort();
        ids
    }

    fn two_file_project() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir_all(dir.path().join("src")).unwrap();
        fs::write(dir.path().join("src/a.rs"), "pub fn caller() { callee() }").unwrap();
        fs::write(dir.path().join("src/b.rs"), "pub fn callee() {}").unwrap();
        dir
    }

    /// With extraction now parallelized across a rayon thread pool (see
    /// the `par_iter` step in `analyze_with_cache_lsp`), the one thing
    /// that must never change is output order: `concepts`/`calls` must
    /// still be merged in `project.files`'s own (sorted-by-path)
    /// order, regardless of which file's parse thread happens to finish
    /// first. A handful of files is enough concurrency to make a
    /// nondeterministic merge order likely to show up across repeated
    /// runs if one were introduced.
    #[test]
    fn parallel_extraction_produces_the_same_concept_order_on_every_run() {
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir_all(dir.path().join("src")).unwrap();
        for letter in ['a', 'b', 'c', 'd', 'e', 'f'] {
            fs::write(
                dir.path().join(format!("src/{letter}.rs")),
                format!("pub fn fn_{letter}() {{}}"),
            )
            .unwrap();
        }
        let project = Project::load(dir.path()).unwrap();

        let first = analyze(&project).unwrap();
        for _ in 0..5 {
            let repeat = analyze(&project).unwrap();
            assert_eq!(
                first
                    .concepts
                    .iter()
                    .map(|c| c.id.clone())
                    .collect::<Vec<_>>(),
                repeat
                    .concepts
                    .iter()
                    .map(|c| c.id.clone())
                    .collect::<Vec<_>>(),
                "concept order must be identical across runs, not just the same set"
            );
        }
    }

    #[test]
    fn cached_analysis_matches_uncached_analysis() {
        let dir = two_file_project();
        let project = Project::load(dir.path()).unwrap();

        let uncached = analyze(&project).unwrap();
        let mut cache = AnalysisCache::default();
        let (cached, stats) = analyze_with_cache(&project, &mut cache).unwrap();

        assert_eq!(stats.reparsed, 2);
        assert_eq!(stats.reused, 0);
        assert_eq!(sorted_ids(&uncached.concepts), sorted_ids(&cached.concepts));
    }

    #[test]
    fn second_run_with_a_warm_cache_reuses_every_unchanged_file() {
        let dir = two_file_project();
        let project = Project::load(dir.path()).unwrap();
        let mut cache = AnalysisCache::default();

        let (first, first_stats) = analyze_with_cache(&project, &mut cache).unwrap();
        assert_eq!(first_stats.reparsed, 2);
        assert_eq!(first_stats.reused, 0);

        let (second, second_stats) = analyze_with_cache(&project, &mut cache).unwrap();
        assert_eq!(second_stats.reparsed, 0);
        assert_eq!(second_stats.reused, 2);
        assert_eq!(sorted_ids(&first.concepts), sorted_ids(&second.concepts));
    }

    #[test]
    fn only_the_changed_file_is_reparsed() {
        let dir = two_file_project();
        let project = Project::load(dir.path()).unwrap();
        let mut cache = AnalysisCache::default();
        analyze_with_cache(&project, &mut cache).unwrap();

        fs::write(
            dir.path().join("src/b.rs"),
            "pub fn callee() {} pub fn extra() {}",
        )
        .unwrap();
        let project = Project::load(dir.path()).unwrap();
        let (_, stats) = analyze_with_cache(&project, &mut cache).unwrap();

        assert_eq!(stats.reparsed, 1, "only src/b.rs changed");
        assert_eq!(stats.reused, 1, "src/a.rs is untouched");
    }

    #[test]
    fn removed_files_are_pruned_from_the_cache_not_left_stale() {
        let dir = two_file_project();
        let project = Project::load(dir.path()).unwrap();
        let mut cache = AnalysisCache::default();
        analyze_with_cache(&project, &mut cache).unwrap();
        assert_eq!(cache.len(), 2);

        fs::remove_file(dir.path().join("src/b.rs")).unwrap();
        let project = Project::load(dir.path()).unwrap();
        analyze_with_cache(&project, &mut cache).unwrap();

        assert_eq!(cache.len(), 1, "src/b.rs should be dropped, not stale");
    }

    #[test]
    fn emits_one_package_per_workspace_member_and_links_modules_to_the_right_one() {
        let dir = tempfile::tempdir().unwrap();
        // A virtual workspace manifest: no `[package]` table of its own,
        // so no root Package concept, just the two real members below.
        fs::write(
            dir.path().join("Cargo.toml"),
            "[workspace]\nmembers = [\"crates/a\", \"crates/b\"]\n",
        )
        .unwrap();
        for member in ["a", "b"] {
            fs::create_dir_all(dir.path().join("crates").join(member).join("src")).unwrap();
            fs::write(
                dir.path().join("crates").join(member).join("Cargo.toml"),
                format!("[package]\nname = \"{member}\"\nversion = \"0.1.0\"\n"),
            )
            .unwrap();
            fs::write(
                dir.path().join("crates").join(member).join("src/lib.rs"),
                format!("pub fn {member}_fn() {{}}"),
            )
            .unwrap();
        }

        let project = Project::load(dir.path()).unwrap();
        let result = analyze(&project).unwrap();

        let packages: Vec<&Concept> = result
            .concepts
            .iter()
            .filter(|c| c.kind == ConceptKind::Package)
            .collect();
        assert_eq!(
            packages.len(),
            2,
            "the virtual workspace root has no [package] table and shouldn't get a concept: {packages:?}"
        );
        let names: Vec<&str> = packages.iter().map(|p| p.name.as_str()).collect();
        assert!(names.contains(&"a"));
        assert!(names.contains(&"b"));

        let module_a = result
            .concepts
            .iter()
            .find(|c| c.kind == ConceptKind::Module && c.location.file == "crates/a/src/lib.rs")
            .unwrap();
        let package_a = packages.iter().find(|p| p.name == "a").unwrap();
        assert!(
            module_a
                .relationships
                .iter()
                .any(|r| r.kind == RelationKind::MemberOf && r.target == package_a.id),
            "crate a's module should be linked to package a, not b: {:?}",
            module_a.relationships
        );

        let module_b = result
            .concepts
            .iter()
            .find(|c| c.kind == ConceptKind::Module && c.location.file == "crates/b/src/lib.rs")
            .unwrap();
        let package_b = packages.iter().find(|p| p.name == "b").unwrap();
        assert!(module_b
            .relationships
            .iter()
            .any(|r| r.kind == RelationKind::MemberOf && r.target == package_b.id));
        assert!(
            !module_b
                .relationships
                .iter()
                .any(|r| r.kind == RelationKind::MemberOf && r.target == package_a.id),
            "crate b's module must not be linked to package a"
        );
    }

    #[test]
    fn two_manifest_kinds_in_the_same_directory_get_distinct_package_ids() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(
            dir.path().join("Cargo.toml"),
            "[workspace]\nmembers = [\"crates/docgen\"]\n",
        )
        .unwrap();
        let member = dir.path().join("crates/docgen");
        fs::create_dir_all(member.join("src")).unwrap();
        fs::write(
            member.join("Cargo.toml"),
            "[package]\nname = \"docgen\"\nversion = \"0.1.0\"\n",
        )
        .unwrap();
        fs::write(member.join("package.json"), r#"{"name": "docgen-web"}"#).unwrap();
        fs::write(member.join("src/lib.rs"), "pub fn hello() {}").unwrap();

        let project = Project::load(dir.path()).unwrap();
        let result = analyze(&project).unwrap();

        let packages: Vec<&Concept> = result
            .concepts
            .iter()
            .filter(|c| c.kind == ConceptKind::Package)
            .collect();
        assert_eq!(packages.len(), 2, "{packages:?}");
        assert_ne!(
            packages[0].id, packages[1].id,
            "two different manifest kinds sharing a directory must not collide on id"
        );
    }

    #[test]
    fn a_malformed_manifest_does_not_abort_analysis_of_sibling_packages() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(
            dir.path().join("Cargo.toml"),
            "[workspace]\nmembers = [\"crates/a\", \"crates/b\"]\n",
        )
        .unwrap();
        fs::create_dir_all(dir.path().join("crates/a/src")).unwrap();
        // Truncated table header: fails to parse as TOML.
        fs::write(
            dir.path().join("crates/a/Cargo.toml"),
            "[package\nname = \"a\"\n",
        )
        .unwrap();
        fs::write(dir.path().join("crates/a/src/lib.rs"), "pub fn a_fn() {}").unwrap();

        fs::create_dir_all(dir.path().join("crates/b/src")).unwrap();
        fs::write(
            dir.path().join("crates/b/Cargo.toml"),
            "[package]\nname = \"b\"\nversion = \"0.1.0\"\n",
        )
        .unwrap();
        fs::write(dir.path().join("crates/b/src/lib.rs"), "pub fn b_fn() {}").unwrap();

        let project = Project::load(dir.path()).unwrap();
        let result = analyze(&project).expect("a bad manifest must not abort the whole analysis");

        let packages: Vec<&Concept> = result
            .concepts
            .iter()
            .filter(|c| c.kind == ConceptKind::Package)
            .collect();
        assert_eq!(
            packages.len(),
            1,
            "only the valid crate b should get a Package concept: {packages:?}"
        );
        assert_eq!(packages[0].name, "b");
        assert!(result.concepts.iter().any(|c| c.name == "a_fn"));
        assert!(result.concepts.iter().any(|c| c.name == "b_fn"));
    }
}
