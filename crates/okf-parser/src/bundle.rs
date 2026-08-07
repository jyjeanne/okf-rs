//! Reads a previously written OKF bundle back into [`Concept`]s — the
//! reverse of `okf-generator::write_bundle`. Lets tools that need the full
//! relationship-rich concept model (like `okf-graph`) work directly off a
//! bundle on disk instead of re-analyzing the project from source.

use crate::{Concept, Confidence, Location, RelationKind, Relationship};
use anyhow::{Context, Result};
use std::collections::HashMap;
use std::fs;
use std::path::Path;

/// Walks `bundle_dir` and reconstructs every concept file's frontmatter
/// (and, where present, its `# Signature` body section) into a [`Concept`].
/// Skips `index.md` navigation pages, which carry no frontmatter, and any
/// file whose frontmatter can't be parsed (malformed bundles are
/// `okf-validator`'s job to flag, not this reader's).
///
/// Some fields written by `okf-generator` aren't recoverable from a bundle
/// alone and are filled in with best-effort placeholders: `qualified_name`
/// (not stored in frontmatter — the bundle only needs the derived `id`)
/// falls back to the id's own path segments, and a relationship target's
/// `target_display` is resolved by looking up the target concept's title
/// among the concepts just loaded, falling back to the raw target id for
/// links that point outside the bundle (e.g. external imports).
pub fn read_bundle(bundle_dir: &Path) -> Result<Vec<Concept>> {
    let mut concepts = Vec::new();
    for entry in walkdir::WalkDir::new(bundle_dir) {
        let entry = entry.context("failed to walk bundle directory")?;
        if !entry.file_type().is_file() {
            continue;
        }
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("md") {
            continue;
        }
        if path.file_name().and_then(|n| n.to_str()) == Some("index.md") {
            continue;
        }
        let relative = path
            .strip_prefix(bundle_dir)
            .unwrap_or(path)
            .to_string_lossy()
            .replace('\\', "/");
        let id = relative
            .strip_suffix(".md")
            .unwrap_or(&relative)
            .to_string();
        // Normalize CRLF to LF, matching okf-validator/okf-search, so a
        // bundle edited on Windows still has its frontmatter recognized.
        let content = fs::read_to_string(path)
            .with_context(|| format!("failed to read {relative}"))?
            .replace("\r\n", "\n");
        if let Some(concept) = parse_concept(id, &content) {
            concepts.push(concept);
        }
    }
    concepts.sort_by(|a, b| a.id.cmp(&b.id));

    let titles: HashMap<&str, &str> = concepts
        .iter()
        .map(|c| (c.id.as_str(), c.name.as_str()))
        .collect();
    let displays: Vec<Vec<String>> = concepts
        .iter()
        .map(|c| {
            c.relationships
                .iter()
                .map(|r| {
                    titles
                        .get(r.target.as_str())
                        .map(|&t| t.to_string())
                        .unwrap_or_else(|| r.target.clone())
                })
                .collect()
        })
        .collect();
    for (concept, displays) in concepts.iter_mut().zip(displays) {
        for (rel, display) in concept.relationships.iter_mut().zip(displays) {
            rel.target_display = display;
        }
    }

    Ok(concepts)
}

fn parse_concept(id: String, content: &str) -> Option<Concept> {
    let rest = content.strip_prefix("---\n")?;
    let end = rest.find("\n---\n")?;
    let yaml = &rest[..end];
    let body = &rest[end + 5..];
    let value: serde_yaml::Value = serde_yaml::from_str(yaml).ok()?;
    let mapping = value.as_mapping()?;

    let type_ = mapping.get("type")?.as_str()?;
    let (language, kind) = Concept::parse_frontmatter_type(type_)?;

    let name = mapping
        .get("title")
        .and_then(|v| v.as_str())
        .map(str::to_string)
        .unwrap_or_else(|| id.rsplit('/').next().unwrap_or(&id).to_string());

    let description = mapping
        .get("description")
        .and_then(|v| v.as_str())
        .map(str::to_string);

    let location = mapping
        .get("resource")
        .and_then(|v| v.as_str())
        .and_then(Location::parse)
        .unwrap_or(Location {
            file: String::new(),
            start_line: 0,
            end_line: 0,
        });

    let tags = mapping
        .get("tags")
        .and_then(|v| v.as_sequence())
        .map(|seq| {
            seq.iter()
                .filter_map(|t| t.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default();

    let is_public = mapping.get("visibility").and_then(|v| v.as_str()) != Some("private");

    // OKF v0.2 records this under `generated.at` (spec §5.2); a v0.1
    // bundle's flat `timestamp` is the sanctioned fallback (spec §13.1).
    let generated_at = mapping
        .get("generated")
        .and_then(|v| v.as_mapping())
        .and_then(|g| g.get("at"))
        .or_else(|| mapping.get("timestamp"))
        .and_then(|v| v.as_str())
        .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
        .map(|dt| dt.with_timezone(&chrono::Utc));

    let relationships = mapping
        .get("relationships")
        .and_then(|v| v.as_mapping())
        .map(|rel_map| {
            let mut rels = Vec::new();
            for (key, targets) in rel_map {
                let Some(kind) = key.as_str().and_then(RelationKind::from_frontmatter_key) else {
                    continue;
                };
                for entry in targets.as_sequence().into_iter().flatten() {
                    let Some((target, resolved_by, confidence)) = parse_relationship_entry(entry)
                    else {
                        continue;
                    };
                    rels.push(Relationship {
                        kind,
                        // Patched below, once every concept in the bundle
                        // has been loaded and can be looked up by id.
                        target_display: target.clone(),
                        target,
                        resolved_by,
                        confidence,
                    });
                }
            }
            rels
        })
        .unwrap_or_default();

    // Not stored in frontmatter (the bundle only needs the derived `id`);
    // best-effort reconstruction from the id's own path segments.
    let qualified_name = id
        .split_once('/')
        .map(|(_, rest)| rest.replace('/', "::"))
        .unwrap_or_else(|| id.clone());

    Some(Concept {
        id,
        kind,
        language,
        name,
        qualified_name,
        description,
        location,
        signature: parse_signature(body),
        tags,
        is_public,
        generated_at,
        relationships,
    })
}

/// Parses one entry of a `relationships.<kind>` sequence, accepting both
/// shapes `okf-generator` has ever written: a bare target-id string (every
/// bundle generated before edge provenance/confidence existed, or a
/// hand-edited entry someone kept simple), and the current
/// `{target, resolved_by, confidence}` mapping. Backward-compatible on
/// purpose — an old bundle doesn't need regenerating just to stay
/// readable by a newer `okf-rs`, and a missing/unrecognized
/// `resolved_by`/`confidence` on an otherwise-valid mapping falls back to
/// the same "produced by Tree-sitter, exact" default a bare string gets,
/// rather than failing to parse the entry at all.
fn parse_relationship_entry(entry: &serde_yaml::Value) -> Option<(String, String, Confidence)> {
    if let Some(target) = entry.as_str() {
        return Some((
            target.to_string(),
            "tree-sitter".to_string(),
            Confidence::Exact,
        ));
    }
    let mapping = entry.as_mapping()?;
    let target = mapping.get("target")?.as_str()?.to_string();
    let resolved_by = mapping
        .get("resolved_by")
        .and_then(|v| v.as_str())
        .unwrap_or("tree-sitter")
        .to_string();
    let confidence = mapping
        .get("confidence")
        .and_then(|v| v.as_str())
        .and_then(Confidence::parse)
        .unwrap_or(Confidence::Exact);
    Some((target, resolved_by, confidence))
}

fn parse_signature(body: &str) -> Option<String> {
    let marker = "# Signature\n\n`";
    let start = body.find(marker)? + marker.len();
    let rest = &body[start..];
    let end = rest.find('`')?;
    Some(rest[..end].to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{ConceptKind, Language};

    fn write(dir: &Path, relative: &str, content: &str) {
        let path = dir.join(relative);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, content).unwrap();
    }

    #[test]
    fn reads_frontmatter_signature_and_relationships() {
        let dir = tempfile::tempdir().unwrap();
        write(dir.path(), "index.md", "nav\n");
        write(
            dir.path(),
            "functions/auth/verify_token.md",
            "---\ntype: Rust Function\ntitle: verify_token\nresource: src/auth.rs#L4-L6\ntags:\n  - auth\nrelationships:\n  calls:\n    - functions/auth/decode_jwt\n---\n\n# Signature\n\n`fn verify_token(&self, token: &str) -> bool`\n\n# Calls\n\n- [decode_jwt](decode_jwt.md)\n",
        );
        write(
            dir.path(),
            "functions/auth/decode_jwt.md",
            "---\ntype: Rust Function\ntitle: decode_jwt\nresource: src/auth.rs#L10\nvisibility: private\nrelationships:\n  called_by:\n    - functions/auth/verify_token\n---\n\nbody\n",
        );

        let concepts = read_bundle(dir.path()).unwrap();
        assert_eq!(concepts.len(), 2);

        let verify = &concepts[1];
        assert_eq!(verify.id, "functions/auth/verify_token");
        assert_eq!(verify.kind, ConceptKind::Function);
        assert_eq!(verify.language, Language::Rust);
        assert_eq!(verify.name, "verify_token");
        assert!(verify.is_public);
        assert_eq!(verify.tags, vec!["auth".to_string()]);
        assert_eq!(
            verify.signature.as_deref(),
            Some("fn verify_token(&self, token: &str) -> bool")
        );
        assert_eq!(verify.location.file, "src/auth.rs");
        assert_eq!(verify.location.start_line, 4);
        assert_eq!(verify.location.end_line, 6);
        assert_eq!(verify.relationships.len(), 1);
        assert_eq!(verify.relationships[0].kind, RelationKind::Calls);
        assert_eq!(verify.relationships[0].target, "functions/auth/decode_jwt");
        assert_eq!(verify.relationships[0].target_display, "decode_jwt");
        // A bare target-id string (the format every bundle used before
        // provenance/confidence existed) falls back to the same
        // "produced by Tree-sitter, exact" default a fresh `okf-rs
        // generate` gives an ordinary edge.
        assert_eq!(verify.relationships[0].resolved_by, "tree-sitter");
        assert_eq!(verify.relationships[0].confidence, Confidence::Exact);

        let decode = &concepts[0];
        assert!(!decode.is_public);
        assert_eq!(decode.relationships[0].kind, RelationKind::CalledBy);
        assert_eq!(decode.relationships[0].target_display, "verify_token");
    }

    #[test]
    fn reads_the_object_shaped_relationship_entry_with_real_provenance() {
        let dir = tempfile::tempdir().unwrap();
        write(
            dir.path(),
            "functions/a.md",
            "---\ntype: Rust Function\ntitle: a\nresource: src/lib.rs#L1\nrelationships:\n  calls:\n    - target: functions/b\n      resolved_by: rust-analyzer\n      confidence: semantic\n---\n\nbody\n",
        );

        let concepts = read_bundle(dir.path()).unwrap();
        assert_eq!(concepts.len(), 1);
        assert_eq!(concepts[0].relationships[0].target, "functions/b");
        assert_eq!(concepts[0].relationships[0].resolved_by, "rust-analyzer");
        assert_eq!(
            concepts[0].relationships[0].confidence,
            Confidence::Semantic
        );
    }

    #[test]
    fn an_object_shaped_entry_missing_or_invalid_provenance_falls_back_to_the_default() {
        let dir = tempfile::tempdir().unwrap();
        write(
            dir.path(),
            "functions/a.md",
            "---\ntype: Rust Function\ntitle: a\nresource: src/lib.rs#L1\nrelationships:\n  calls:\n    - target: functions/b\n      confidence: not-a-real-value\n---\n\nbody\n",
        );

        let concepts = read_bundle(dir.path()).unwrap();
        // Neither `resolved_by` (absent) nor `confidence` (present but
        // unrecognized) should fail parsing the entry entirely -- both
        // fall back to the same default a bare string gets.
        assert_eq!(concepts[0].relationships[0].target, "functions/b");
        assert_eq!(concepts[0].relationships[0].resolved_by, "tree-sitter");
        assert_eq!(concepts[0].relationships[0].confidence, Confidence::Exact);
    }

    #[test]
    fn an_object_shaped_entry_with_no_target_is_skipped() {
        let dir = tempfile::tempdir().unwrap();
        write(
            dir.path(),
            "functions/a.md",
            "---\ntype: Rust Function\ntitle: a\nresource: src/lib.rs#L1\nrelationships:\n  calls:\n    - resolved_by: tree-sitter\n      confidence: exact\n---\n\nbody\n",
        );

        let concepts = read_bundle(dir.path()).unwrap();
        assert!(concepts[0].relationships.is_empty());
    }

    #[test]
    fn skips_index_files_and_unparsable_frontmatter() {
        let dir = tempfile::tempdir().unwrap();
        write(dir.path(), "index.md", "nav\n");
        write(dir.path(), "functions/index.md", "nav\n");
        write(
            dir.path(),
            "functions/broken.md",
            "not a valid concept file\n",
        );

        let concepts = read_bundle(dir.path()).unwrap();
        assert!(concepts.is_empty());
    }

    #[test]
    fn falls_back_to_raw_target_id_for_external_relationships() {
        let dir = tempfile::tempdir().unwrap();
        write(
            dir.path(),
            "modules/auth.md",
            "---\ntype: Rust Module\ntitle: auth\nresource: src/auth.rs#L1\nrelationships:\n  imports:\n    - external/std-collections-hashmap\n---\n\nbody\n",
        );

        let concepts = read_bundle(dir.path()).unwrap();
        assert_eq!(concepts.len(), 1);
        assert_eq!(
            concepts[0].relationships[0].target_display,
            "external/std-collections-hashmap"
        );
    }
}
