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
use okf_parser::{Concept, ConceptKind, Language, Location, RelationKind, Relationship};
use okf_tree_sitter::CallCandidate;
use std::collections::HashMap;
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
                okf_tree_sitter::extract_source(&source, file)
                    .with_context(|| format!("failed to analyze {}", file.relative_path))?
            }
        };
        fresh_cache.insert(&file.relative_path, hash, extraction.clone());
        for call in extraction.calls {
            calls.push((call, file.language, file.relative_path.clone()));
        }
        concepts.extend(extraction.concepts);
        if use_lsp {
            file_sources.insert(file.relative_path.clone(), source);
        }
    }
    *cache = fresh_cache;

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

    let mut resolved_edges: Vec<(String, String)> = Vec::new();
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
            resolved_edges.push((call.caller_id.clone(), callee_id));
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
            concept.relationships.push(Relationship {
                kind: RelationKind::MemberOf,
                target: package_id.clone(),
                target_display: package_name.clone(),
            });
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
