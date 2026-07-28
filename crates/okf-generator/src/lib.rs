//! Writes a set of [`okf_parser::Concept`]s out as a conformant OKF bundle:
//! one markdown file with YAML frontmatter per concept, grouped into
//! per-kind directories, with an `index.md` at the bundle root and at each
//! directory.

use anyhow::{anyhow, Result};
use chrono::{DateTime, Utc};
use okf_parser::{Concept, ConceptKind, RelationKind};
use serde::Serialize;
use std::collections::{BTreeMap, HashSet};
use std::fs;
use std::path::Path;

#[derive(Serialize)]
struct Frontmatter {
    #[serde(rename = "type")]
    type_: String,
    title: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    description: Option<String>,
    resource: String,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    tags: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    timestamp: Option<DateTime<Utc>>,
}

/// Writes `concepts` to `output_dir` as an OKF bundle. Fails fast (before
/// writing anything) if two concepts would collide on the same id, since
/// that would otherwise silently overwrite one file with another.
///
/// The collision check is case-insensitive, even though ids are compared
/// exactly everywhere else: ids become file paths, and the most common
/// target filesystems (default macOS APFS, Windows NTFS) are
/// case-insensitive, so two ids differing only by case (e.g. `Run` vs.
/// `run`) would still collide on disk even though this checker's own
/// case-sensitive `HashSet` wouldn't catch it.
pub fn write_bundle(concepts: &[Concept], output_dir: &Path) -> Result<()> {
    let mut seen = HashSet::new();
    for concept in concepts {
        if !seen.insert(concept.id.to_ascii_lowercase()) {
            return Err(anyhow!(
                "duplicate concept id `{}` (from {}); refusing to write a bundle that would silently overwrite it (ids are compared case-insensitively, since they become file paths on filesystems that may not distinguish case)",
                concept.id,
                concept.location
            ));
        }
    }

    let bundle_ids: HashSet<&str> = concepts.iter().map(|c| c.id.as_str()).collect();

    let mut by_dir: BTreeMap<&str, Vec<&Concept>> = BTreeMap::new();
    for concept in concepts {
        by_dir
            .entry(concept.kind.bundle_dir())
            .or_default()
            .push(concept);
    }
    for entries in by_dir.values_mut() {
        entries.sort_by(|a, b| a.id.cmp(&b.id));
    }

    fs::create_dir_all(output_dir)?;

    for concept in concepts {
        let file_path = output_dir.join(format!("{}.md", concept.id));
        if let Some(parent) = file_path.parent() {
            fs::create_dir_all(parent)?;
        }
        let content = render_concept(concept, concepts, &bundle_ids)?;
        fs::write(file_path, content)?;
    }

    for (dir, entries) in &by_dir {
        write_dir_index(output_dir, dir, entries)?;
    }

    write_root_index(output_dir, &by_dir)?;

    Ok(())
}

/// Relative markdown link from the file at `from_pseudo_id` (a concept id,
/// or `"<dir>/index"` / `"index"` for an index file) to `to_id`. Always
/// correct (walks up to the bundle root and back down), though not always
/// the shortest possible path.
fn relative_link(from_pseudo_id: &str, to_id: &str) -> String {
    let depth = from_pseudo_id.matches('/').count();
    let up = "../".repeat(depth);
    format!("{up}{to_id}.md")
}

fn render_concept(
    concept: &Concept,
    all: &[Concept],
    bundle_ids: &HashSet<&str>,
) -> Result<String> {
    let frontmatter = Frontmatter {
        type_: concept.frontmatter_type(),
        title: concept.name.clone(),
        description: concept.description.clone(),
        resource: concept.location.to_string(),
        tags: concept.tags.clone(),
        timestamp: concept.timestamp,
    };
    let yaml = serde_yaml::to_string(&frontmatter)?;
    let yaml = yaml.strip_prefix("---\n").unwrap_or(&yaml);

    let mut body = String::new();

    if let Some(signature) = &concept.signature {
        body.push_str("# Signature\n\n`");
        body.push_str(signature);
        body.push_str("`\n\n");
    }

    if concept.kind == ConceptKind::Module {
        let members: Vec<&Concept> = all
            .iter()
            .filter(|c| c.id != concept.id && c.location.file == concept.location.file)
            .collect();
        if !members.is_empty() {
            body.push_str("# Contains\n\n");
            for member in members {
                body.push_str(&format!(
                    "- [{}]({})\n",
                    member.name,
                    relative_link(&concept.id, &member.id)
                ));
            }
            body.push('\n');
        }
    }

    for (kind, heading) in [
        (RelationKind::Imports, "# Imports"),
        (RelationKind::Calls, "# Calls"),
        (RelationKind::CalledBy, "# Called by"),
        (RelationKind::Implements, "# Implements"),
        (RelationKind::Inherits, "# Inherits"),
        (RelationKind::DependsOn, "# Depends on"),
        (RelationKind::MemberOf, "# Member of"),
    ] {
        let rels: Vec<_> = concept
            .relationships
            .iter()
            .filter(|r| r.kind == kind)
            .collect();
        if rels.is_empty() {
            continue;
        }
        body.push_str(heading);
        body.push_str("\n\n");
        for rel in rels {
            if bundle_ids.contains(rel.target.as_str()) {
                body.push_str(&format!(
                    "- [{}]({})\n",
                    rel.target_display,
                    relative_link(&concept.id, &rel.target)
                ));
            } else {
                body.push_str(&format!("- `{}`\n", rel.target_display));
            }
        }
        body.push('\n');
    }

    Ok(format!("---\n{yaml}---\n\n{}", body.trim_end()))
}

fn write_dir_index(output_dir: &Path, dir: &str, entries: &[&Concept]) -> Result<()> {
    let pseudo_id = format!("{dir}/index");
    let mut content = format!("# {}\n\n", capitalize(dir));
    for concept in entries {
        content.push_str(&format!(
            "- [{}]({}) — {}\n",
            concept.name,
            relative_link(&pseudo_id, &concept.id),
            concept.frontmatter_type()
        ));
    }
    fs::write(output_dir.join(dir).join("index.md"), content)?;
    Ok(())
}

fn write_root_index(output_dir: &Path, by_dir: &BTreeMap<&str, Vec<&Concept>>) -> Result<()> {
    let mut content = String::from("# Knowledge Base\n\n");
    for (dir, entries) in by_dir {
        content.push_str(&format!(
            "- [{}]({}/index.md) ({})\n",
            capitalize(dir),
            dir,
            entries.len()
        ));
    }
    fs::write(output_dir.join("index.md"), content)?;
    Ok(())
}

fn capitalize(s: &str) -> String {
    let mut chars = s.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
        None => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use okf_parser::{ConceptKind, Language, Location, Relationship};

    fn concept(id_kind: ConceptKind, name: &str, qualified: &str, file: &str) -> Concept {
        Concept {
            id: Concept::make_id(id_kind, qualified),
            kind: id_kind,
            language: Language::Rust,
            name: name.to_string(),
            qualified_name: qualified.to_string(),
            description: None,
            location: Location {
                file: file.to_string(),
                start_line: 1,
                end_line: 1,
            },
            signature: Some(format!("fn {name}()")),
            tags: Vec::new(),
            timestamp: None,
            relationships: Vec::new(),
        }
    }

    #[test]
    fn writes_bundle_with_index_and_links() {
        let dir = tempfile::tempdir().unwrap();
        let mut module = concept(ConceptKind::Module, "auth", "auth", "src/auth.rs");
        let mut caller = concept(
            ConceptKind::Function,
            "verify_token",
            "auth.verify_token",
            "src/auth.rs",
        );
        let callee = concept(
            ConceptKind::Function,
            "decode_jwt",
            "auth.decode_jwt",
            "src/auth.rs",
        );

        caller.relationships.push(Relationship {
            kind: RelationKind::Calls,
            target: callee.id.clone(),
            target_display: "decode_jwt".to_string(),
        });
        module.relationships.push(Relationship {
            kind: RelationKind::Imports,
            target: "external/std-collections-hashmap".to_string(),
            target_display: "std::collections::HashMap".to_string(),
        });

        let concepts = vec![module, caller, callee];
        write_bundle(&concepts, dir.path()).unwrap();

        assert!(dir.path().join("index.md").exists());
        assert!(dir.path().join("modules/index.md").exists());
        assert!(dir.path().join("functions/index.md").exists());
        assert!(dir.path().join("modules/auth.md").exists());
        // qualified_name "auth.verify_token" -> id "functions/auth/verify_token"
        assert!(dir.path().join("functions/auth/verify_token.md").exists());

        let caller_content =
            fs::read_to_string(dir.path().join("functions/auth/verify_token.md")).unwrap();
        assert!(caller_content.starts_with("---\ntype: Rust Function\n"));
        // Cross-links always resolve correctly (root-relative round trip),
        // even though they aren't the shortest possible relative path.
        assert!(caller_content.contains("[decode_jwt](../../functions/auth/decode_jwt.md)"));

        let module_content = fs::read_to_string(dir.path().join("modules/auth.md")).unwrap();
        assert!(module_content.contains("- `std::collections::HashMap`"));
        assert!(module_content.contains("[verify_token]"));
        assert!(module_content.contains("[decode_jwt]"));
    }

    #[test]
    fn rejects_duplicate_ids() {
        let dir = tempfile::tempdir().unwrap();
        let a = concept(ConceptKind::Function, "run", "run", "src/a.rs");
        let b = concept(ConceptKind::Function, "run", "run", "src/b.rs");
        let err = write_bundle(&[a, b], dir.path()).unwrap_err();
        assert!(err.to_string().contains("duplicate concept id"));
    }

    #[test]
    fn rejects_ids_differing_only_by_case() {
        // On case-insensitive filesystems (default macOS, Windows), `Run`
        // and `run` are the same path, so this must be caught even though
        // the ids aren't byte-for-byte equal.
        let dir = tempfile::tempdir().unwrap();
        let a = concept(ConceptKind::Function, "Run", "Run", "src/a.rs");
        let b = concept(ConceptKind::Function, "run", "run", "src/b.rs");
        let err = write_bundle(&[a, b], dir.path()).unwrap_err();
        assert!(err.to_string().contains("duplicate concept id"));
    }
}
