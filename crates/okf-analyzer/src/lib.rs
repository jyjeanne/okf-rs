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
//! with that name project-wide) — with no type information available yet
//! (that arrives with LSP integration in Phase 2), resolving an ambiguous
//! name would risk drawing a wrong edge, so it is deliberately left
//! unresolved rather than guessed.
//!
//! Per-file extraction (the expensive tree-sitter parse) can be skipped
//! for files that haven't changed since a previous run — see
//! [`analyze_with_cache`] and [`AnalysisCache`].

mod cache;

pub use cache::AnalysisCache;

use anyhow::{Context, Result};
use okf_core::{ManifestKind, Project};
use okf_parser::{Concept, ConceptKind, Language, Location, RelationKind, Relationship};
use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;

/// The full semantic model for an analyzed project: every extracted
/// concept (optionally including one `Package` concept derived from the
/// project manifest), with import and call relationships already attached.
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
    let mut concepts = Vec::new();
    if let Some(package) = detect_package(project)? {
        concepts.push(package);
    }

    let mut calls = Vec::new();
    let mut stats = IncrementalStats::default();
    let mut fresh_cache = AnalysisCache::default();

    for file in &project.files {
        let source = fs::read_to_string(&file.absolute_path)
            .with_context(|| format!("failed to read {}", file.relative_path))?;
        let hash = cache::hash_content(&source);

        let extraction = match cache.get(&file.relative_path, hash) {
            Some(extraction) => {
                stats.reused += 1;
                extraction
            }
            None => {
                stats.reparsed += 1;
                okf_tree_sitter::extract_file(file)
                    .with_context(|| format!("failed to analyze {}", file.relative_path))?
            }
        };
        fresh_cache.insert(&file.relative_path, hash, extraction.clone());
        calls.extend(extraction.calls);
        concepts.extend(extraction.concepts);
    }
    *cache = fresh_cache;

    let mut symbol_table: HashMap<&str, Vec<&str>> = HashMap::new();
    for concept in &concepts {
        if matches!(concept.kind, ConceptKind::Function | ConceptKind::Method) {
            symbol_table
                .entry(concept.name.as_str())
                .or_default()
                .push(concept.id.as_str());
        }
    }

    let mut resolved_edges: Vec<(String, String)> = Vec::new();
    for call in &calls {
        let Some(candidates) = symbol_table.get(call.callee_name.as_str()) else {
            continue;
        };
        if candidates.len() != 1 {
            continue;
        }
        let callee_id = candidates[0].to_string();
        if callee_id != call.caller_id {
            resolved_edges.push((call.caller_id.clone(), callee_id));
        }
    }

    let index_of: HashMap<String, usize> = concepts
        .iter()
        .enumerate()
        .map(|(i, c)| (c.id.clone(), i))
        .collect();

    for (caller_id, callee_id) in resolved_edges {
        let (Some(&caller_idx), Some(&callee_idx)) =
            (index_of.get(&caller_id), index_of.get(&callee_id))
        else {
            continue;
        };
        let caller_name = concepts[caller_idx].name.clone();
        let callee_name = concepts[callee_idx].name.clone();

        concepts[caller_idx].relationships.push(Relationship {
            kind: RelationKind::Calls,
            target: callee_id.clone(),
            target_display: callee_name,
        });
        concepts[callee_idx].relationships.push(Relationship {
            kind: RelationKind::CalledBy,
            target: caller_id.clone(),
            target_display: caller_name,
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

/// A concept present in both snapshots being diffed, but whose signature
/// or relationships changed between them.
#[derive(Debug, Clone)]
pub struct ChangedConcept {
    pub id: String,
    pub kind: ConceptKind,
    pub before_signature: Option<String>,
    pub after_signature: Option<String>,
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
        if before_concept.signature != concept.signature
            || relationship_set(before_concept) != relationship_set(concept)
        {
            report.changed.push(ChangedConcept {
                id: concept.id.clone(),
                kind: concept.kind,
                before_signature: before_concept.signature.clone(),
                after_signature: concept.signature.clone(),
            });
        }
    }

    report.added.sort_by(|a, b| a.id.cmp(&b.id));
    report.removed.sort_by(|a, b| a.id.cmp(&b.id));
    report.changed.sort_by(|a, b| a.id.cmp(&b.id));
    report
}

/// A comparable, order-independent view of a concept's relationships, so
/// `diff` treats two relationship lists as equal regardless of extraction
/// order.
fn relationship_set(concept: &Concept) -> std::collections::BTreeSet<(RelationKind, &str)> {
    concept
        .relationships
        .iter()
        .map(|r| (r.kind, r.target.as_str()))
        .collect()
}

/// Derives a single `Package` concept from the project's root manifest
/// (`Cargo.toml`, `package.json`, `pyproject.toml`, or `go.mod`), if one
/// was detected during scanning. Multi-package workspace/monorepo
/// aggregation is a Phase 2 feature; Phase 1 covers the single-package
/// case only.
fn detect_package(project: &Project) -> Result<Option<Concept>> {
    let Some(kind) = project.manifest else {
        return Ok(None);
    };
    let (file_name, name, language) = match kind {
        ManifestKind::Cargo => ("Cargo.toml", read_cargo_name(project)?, Language::Rust),
        ManifestKind::Npm => (
            "package.json",
            read_npm_name(project)?,
            Language::JavaScript,
        ),
        ManifestKind::PyProject => (
            "pyproject.toml",
            read_pyproject_name(project)?,
            Language::Python,
        ),
        ManifestKind::GoModule => ("go.mod", read_gomod_name(project)?, Language::Go),
    };
    let Some(name) = name else {
        return Ok(None);
    };

    Ok(Some(Concept {
        id: Concept::make_id(ConceptKind::Package, &name),
        kind: ConceptKind::Package,
        language,
        name: name.clone(),
        qualified_name: name,
        description: None,
        location: Location {
            file: file_name.to_string(),
            start_line: 1,
            end_line: 1,
        },
        signature: None,
        tags: Vec::new(),
        is_public: true,
        timestamp: None,
        relationships: Vec::new(),
    }))
}

fn read_cargo_name(project: &Project) -> Result<Option<String>> {
    let content = fs::read_to_string(project.root.join("Cargo.toml"))?;
    let value: toml::Value = toml::from_str(&content).context("failed to parse Cargo.toml")?;
    Ok(value
        .get("package")
        .and_then(|p| p.get("name"))
        .and_then(|n| n.as_str())
        .map(str::to_string))
}

fn read_npm_name(project: &Project) -> Result<Option<String>> {
    let content = fs::read_to_string(project.root.join("package.json"))?;
    let value: serde_json::Value =
        serde_json::from_str(&content).context("failed to parse package.json")?;
    Ok(value
        .get("name")
        .and_then(|n| n.as_str())
        .map(str::to_string))
}

fn read_pyproject_name(project: &Project) -> Result<Option<String>> {
    let content = fs::read_to_string(project.root.join("pyproject.toml"))?;
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

fn read_gomod_name(project: &Project) -> Result<Option<String>> {
    let content = fs::read_to_string(project.root.join("go.mod"))?;
    Ok(content
        .lines()
        .find_map(|line| line.trim().strip_prefix("module "))
        .map(|m| m.trim().to_string()))
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
        assert!(verify
            .relationships
            .iter()
            .any(|r| r.kind == RelationKind::Calls && r.target_display == "decode_jwt"));

        let decode = result
            .concepts
            .iter()
            .find(|c| c.name == "decode_jwt")
            .unwrap();
        assert!(decode
            .relationships
            .iter()
            .any(|r| r.kind == RelationKind::CalledBy && r.target_display == "verify_token"));
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
            timestamp: None,
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
        before.relationships.push(Relationship {
            kind: RelationKind::Calls,
            target: "functions/x".to_string(),
            target_display: "x".to_string(),
        });
        before.relationships.push(Relationship {
            kind: RelationKind::Calls,
            target: "functions/y".to_string(),
            target_display: "y".to_string(),
        });

        let mut after = make_concept("functions/a", "fn a()");
        after.relationships.push(Relationship {
            kind: RelationKind::Calls,
            target: "functions/y".to_string(),
            target_display: "y".to_string(),
        });
        after.relationships.push(Relationship {
            kind: RelationKind::Calls,
            target: "functions/x".to_string(),
            target_display: "x".to_string(),
        });

        let report = diff(&[before], &[after]);
        assert!(
            report.is_empty(),
            "reordered relationships should not count as a change"
        );
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
}
