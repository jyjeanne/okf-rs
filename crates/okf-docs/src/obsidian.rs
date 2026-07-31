//! Obsidian vault export: one Markdown note per concept, cross-linked
//! with Obsidian's native `[[wikilink]]` syntax instead of the OKF
//! bundle's relative markdown links — for opening the knowledge base
//! directly as an Obsidian vault and using its built-in graph view,
//! backlinks pane, and tag search.
//!
//! Deliberately not just a re-export of the OKF bundle under a new name:
//! `okf-generator`'s own bundle already opens reasonably well in Obsidian
//! (it follows ordinary relative Markdown links too), but wikilinks are
//! the idiomatic, move-resilient way Obsidian expects notes to reference
//! each other, and are what its graph view and backlinks pane are built
//! around.

use anyhow::Result;
use okf_parser::Concept;
use okf_render::{capitalize, contains_members, group_by_kind_dir, RELATIONSHIP_HEADINGS};
use std::collections::HashSet;
use std::fs;
use std::path::Path;

/// Writes one Markdown note per concept (at `<id>.md`, mirroring the OKF
/// bundle's own per-kind directory layout) plus a root index note, into
/// `output_dir` — open `output_dir` directly as an Obsidian vault.
pub fn generate_obsidian(concepts: &[Concept], output_dir: &Path) -> Result<()> {
    let bundle_ids: HashSet<&str> = concepts.iter().map(|c| c.id.as_str()).collect();

    crate::write_one_file_per_concept(concepts, output_dir, "md", |concept| {
        render_note(concept, concepts, &bundle_ids)
    })?;

    write_root_index(concepts, output_dir)?;
    Ok(())
}

/// An Obsidian wikilink to `id`, aliased to `display` — `[[<id>|<display>]]`
/// — so the visible text stays a short, readable name while the link
/// itself always resolves unambiguously by full vault-relative path
/// (concept ids are unique, unlike a bare display name).
fn wikilink(id: &str, display: &str) -> String {
    format!("[[{id}|{display}]]")
}

fn render_note(concept: &Concept, all: &[Concept], bundle_ids: &HashSet<&str>) -> String {
    let mut out = String::from("---\n");
    out.push_str(&format!("type: {}\n", concept.frontmatter_type()));
    if !concept.tags.is_empty() {
        out.push_str(&format!("tags: [{}]\n", concept.tags.join(", ")));
    }
    out.push_str("---\n\n");

    out.push_str(&format!("# {}\n\n", concept.name));
    if !concept.is_public {
        out.push_str("_private_\n\n");
    }
    if let Some(signature) = &concept.signature {
        out.push_str(&format!("```\n{signature}\n```\n\n"));
    }
    if let Some(description) = &concept.description {
        out.push_str(&format!("{description}\n\n"));
    }

    let contains = contains_members(concept, all);
    if !contains.is_empty() {
        out.push_str("## Contains\n\n");
        for member in &contains {
            out.push_str(&format!("- {}\n", wikilink(&member.id, &member.name)));
        }
        out.push('\n');
    }

    for (kind, heading) in RELATIONSHIP_HEADINGS {
        let rels: Vec<_> = concept
            .relationships
            .iter()
            .filter(|r| r.kind == kind)
            .collect();
        if rels.is_empty() {
            continue;
        }
        out.push_str(&format!("## {heading}\n\n"));
        for rel in rels {
            if bundle_ids.contains(rel.target.as_str()) {
                out.push_str(&format!(
                    "- {}\n",
                    wikilink(&rel.target, &rel.target_display)
                ));
            } else {
                out.push_str(&format!("- `{}`\n", rel.target_display));
            }
        }
        out.push('\n');
    }

    out.push_str(&format!("Source: `{}`\n", concept.location));
    out
}

fn write_root_index(concepts: &[Concept], output_dir: &Path) -> Result<()> {
    let by_dir = group_by_kind_dir(concepts);
    let mut out = String::from("# Knowledge Base\n\n");
    for (dir, entries) in &by_dir {
        out.push_str(&format!("## {}\n\n", capitalize(dir)));
        for concept in entries {
            out.push_str(&format!("- {}\n", wikilink(&concept.id, &concept.name)));
        }
        out.push('\n');
    }
    fs::write(output_dir.join("index.md"), out)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use okf_parser::{ConceptKind, Language, Location, RelationKind, Relationship};

    fn concept(kind: ConceptKind, name: &str, qualified: &str, file: &str) -> Concept {
        Concept {
            id: Concept::make_id(kind, qualified),
            kind,
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
            is_public: true,
            generated_at: None,
            relationships: Vec::new(),
        }
    }

    #[test]
    fn writes_notes_with_wikilinks_and_a_root_index() {
        let dir = tempfile::tempdir().unwrap();
        let mut caller = concept(
            ConceptKind::Function,
            "verify_token",
            "verify_token",
            "src/auth.rs",
        );
        let callee = concept(
            ConceptKind::Function,
            "decode_jwt",
            "decode_jwt",
            "src/auth.rs",
        );
        caller.relationships.push(Relationship {
            kind: RelationKind::Calls,
            target: callee.id.clone(),
            target_display: "decode_jwt".to_string(),
        });

        let concepts = vec![caller.clone(), callee.clone()];
        generate_obsidian(&concepts, dir.path()).unwrap();

        assert!(dir.path().join("index.md").exists());
        let note = fs::read_to_string(dir.path().join(format!("{}.md", caller.id))).unwrap();
        assert!(note.starts_with("---\ntype: Rust Function\n---\n"));
        assert!(note.contains("# verify_token"));
        assert!(note.contains("## Calls"));
        assert!(note.contains(&format!("[[{}|decode_jwt]]", callee.id)));

        let index = fs::read_to_string(dir.path().join("index.md")).unwrap();
        assert!(index.contains(&format!("[[{}|verify_token]]", caller.id)));
    }

    #[test]
    fn a_relationship_target_outside_the_bundle_is_plain_text_not_a_dangling_link() {
        let dir = tempfile::tempdir().unwrap();
        let mut concept = concept(ConceptKind::Function, "f", "f", "src/lib.rs");
        concept.relationships.push(Relationship {
            kind: RelationKind::Calls,
            target: "external/std::io::Read::read".to_string(),
            target_display: "Read::read".to_string(),
        });

        generate_obsidian(&[concept.clone()], dir.path()).unwrap();
        let note = fs::read_to_string(dir.path().join(format!("{}.md", concept.id))).unwrap();
        assert!(note.contains("`Read::read`"));
        assert!(!note.contains("[[Read::read"));
    }

    #[test]
    fn tags_become_frontmatter_tags() {
        let dir = tempfile::tempdir().unwrap();
        let mut concept = concept(ConceptKind::Function, "f", "f", "src/lib.rs");
        concept.tags = vec!["auth".to_string(), "security".to_string()];

        generate_obsidian(&[concept.clone()], dir.path()).unwrap();
        let note = fs::read_to_string(dir.path().join(format!("{}.md", concept.id))).unwrap();
        assert!(note.contains("tags: [auth, security]"));
    }
}
