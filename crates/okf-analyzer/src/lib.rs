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

/// Scans and analyzes `project`, producing the full concept + relationship
/// set. Deterministic: running this twice over unchanged source produces
/// byte-identical results (no wall-clock timestamps, no unordered maps
/// affecting output order).
pub fn analyze(project: &Project) -> Result<AnalysisResult> {
    let mut concepts = Vec::new();
    if let Some(package) = detect_package(project)? {
        concepts.push(package);
    }

    let mut calls = Vec::new();
    for file in &project.files {
        let extraction = okf_tree_sitter::extract_file(file)
            .with_context(|| format!("failed to analyze {}", file.relative_path))?;
        calls.extend(extraction.calls);
        concepts.extend(extraction.concepts);
    }

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

    Ok(AnalysisResult {
        root: project.root.clone(),
        concepts,
    })
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
}
