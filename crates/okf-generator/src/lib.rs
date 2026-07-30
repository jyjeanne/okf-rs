//! Writes a set of [`okf_parser::Concept`]s out as a conformant OKF bundle:
//! one markdown file with YAML frontmatter per concept, grouped into
//! per-kind directories, with an `index.md` at the bundle root and at each
//! directory. Each concept's relationships are rendered both into the
//! markdown body (human-readable `# Calls` / `# Imports` / ... sections,
//! cross-linked where the target is in the bundle) and into a
//! `relationships` frontmatter field (machine-readable target ids grouped
//! by kind), so `okf_parser::read_bundle` can reconstruct the full graph
//! from a bundle on disk without re-analyzing the project from source.

mod agents;

use anyhow::{anyhow, Result};
use chrono::{DateTime, Utc};
use okf_parser::{Concept, RelationKind};
use okf_render::{capitalize, contains_members, group_by_kind_dir, relative_link};
use serde::Serialize;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::fs;
use std::path::Path;

pub use agents::write_agent_entrypoints;

/// The OKF spec version `okf-rs` targets (see spec §12), declared once in
/// the bundle root's `index.md` frontmatter (the only place an `index.md`
/// may carry one).
const OKF_SPEC_VERSION: &str = "0.2";

/// The actor identity `okf-rs` fills into every concept's `generated.by`
/// (spec §7's `<producer>/<version>` convention), since the producer of a
/// generated bundle is always `okf-rs` itself.
const PRODUCER: &str = concat!("okf-rs/", env!("CARGO_PKG_VERSION"));

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
    /// Only emitted for non-public concepts, so a bundle where everything
    /// is public (the common case) stays uncluttered; absence of the
    /// field means public, matching the natural reading of a knowledge
    /// base entry as something worth exposing.
    #[serde(skip_serializing_if = "Option::is_none")]
    visibility: Option<&'static str>,
    /// OKF v0.2 trust family (spec §5.2), superseding the v0.1 `timestamp`
    /// field (§13.1). `by` is always populated with the [`PRODUCER`] actor,
    /// since it's the only always-known half of the pair.
    generated: GeneratedFrontmatter,
    /// Target concept ids grouped by relation kind, so a bundle on disk
    /// carries the full call/import graph without needing to re-analyze
    /// the project from source (see `okf_parser::read_bundle`, the
    /// reverse of this). Only emitted when non-empty.
    #[serde(skip_serializing_if = "Option::is_none")]
    relationships: Option<RelationshipsFrontmatter>,
}

#[derive(Serialize)]
struct GeneratedFrontmatter {
    by: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    at: Option<DateTime<Utc>>,
}

#[derive(Serialize, Default)]
struct RelationshipsFrontmatter {
    #[serde(skip_serializing_if = "Vec::is_empty")]
    imports: Vec<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    calls: Vec<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    called_by: Vec<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    implements: Vec<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    inherits: Vec<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    depends_on: Vec<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    member_of: Vec<String>,
}

/// Groups `concept`'s relationship targets by kind for frontmatter,
/// preserving each kind's original relative order. `None` when the
/// concept has no relationships, so the frontmatter field is omitted
/// entirely rather than emitted as an empty mapping.
///
/// A target appearing more than once under the same kind — e.g. a
/// function calling the same callee from two different call sites in its
/// body — is collapsed to its first occurrence: `relationships` records
/// *which* concepts a relationship holds toward, not how many times or
/// where, so a repeat carries no extra information and would otherwise
/// double up as a redundant line in both this frontmatter list and the
/// body section below (see `unique_by_kind_and_target`).
fn relationships_frontmatter(concept: &Concept) -> Option<RelationshipsFrontmatter> {
    if concept.relationships.is_empty() {
        return None;
    }
    let mut grouped = RelationshipsFrontmatter::default();
    for rel in unique_by_kind_and_target(concept.relationships.iter()) {
        let bucket = match rel.kind {
            RelationKind::Imports => &mut grouped.imports,
            RelationKind::Calls => &mut grouped.calls,
            RelationKind::CalledBy => &mut grouped.called_by,
            RelationKind::Implements => &mut grouped.implements,
            RelationKind::Inherits => &mut grouped.inherits,
            RelationKind::DependsOn => &mut grouped.depends_on,
            RelationKind::MemberOf => &mut grouped.member_of,
        };
        bucket.push(rel.target.clone());
    }
    Some(grouped)
}

/// Filters `rels` down to one entry per distinct `(kind, target)` pair,
/// keeping the first occurrence and otherwise preserving relative order.
/// Shared by `relationships_frontmatter` and `render_concept`'s body
/// sections so the frontmatter's `relationships` field and the
/// human-readable `# Calls`/... sections always agree on what's listed,
/// rather than deduplicating each independently and risking drift.
fn unique_by_kind_and_target<'a>(
    rels: impl Iterator<Item = &'a okf_parser::Relationship>,
) -> Vec<&'a okf_parser::Relationship> {
    let mut seen: HashSet<(RelationKind, &str)> = HashSet::new();
    rels.filter(|rel| seen.insert((rel.kind, rel.target.as_str())))
        .collect()
}

/// Assigns a unique id to every concept after the first occurrence of a
/// given id (compared case-insensitively, matching [`write_bundle`]'s own
/// collision check below), by appending `-2`, `-3`, ... to each repeat.
///
/// This is common for conditionally-compiled definitions that share one
/// name in one file but are never actually compiled together — e.g.
/// Rust's `#[cfg(feature = "x")]` / `#[cfg(not(feature = "x"))]` stub
/// pair for the same function name — so treating them as distinct
/// concepts reflects the source faithfully, rather than refusing to
/// generate a bundle at all just because Tree-sitter (unlike `rustc`)
/// doesn't evaluate `cfg` attributes.
///
/// Renamed in a deterministic order — sorted by `(file, start_line)`
/// rather than `concepts`' incoming order — so which occurrence is
/// considered "first" (and so keeps the unsuffixed id) never depends on
/// extraction order, matching the rest of `okf-rs`'s determinism
/// guarantee.
fn disambiguate_duplicate_ids(concepts: &[Concept]) -> Vec<Concept> {
    let mut concepts = concepts.to_vec();

    let mut order: Vec<usize> = (0..concepts.len()).collect();
    order.sort_by(|&a, &b| {
        concepts[a]
            .location
            .file
            .cmp(&concepts[b].location.file)
            .then(
                concepts[a]
                    .location
                    .start_line
                    .cmp(&concepts[b].location.start_line),
            )
    });

    let mut seen: HashMap<String, usize> = HashMap::new();
    for idx in order {
        let count = seen
            .entry(concepts[idx].id.to_ascii_lowercase())
            .or_insert(0);
        *count += 1;
        if *count > 1 {
            concepts[idx].id = format!("{}-{}", concepts[idx].id, count);
        }
    }

    concepts
}

/// Writes `concepts` to `output_dir` as an OKF bundle. Duplicate-id
/// concepts (see [`disambiguate_duplicate_ids`]) are renamed rather than
/// silently overwriting one another; if a rename still can't produce a
/// fully unique set (in practice, only if a generated `-N` suffix itself
/// happens to already be a distinct concept's id), this fails fast before
/// writing anything rather than overwrite a file.
///
/// The collision check is case-insensitive, even though ids are compared
/// exactly everywhere else: ids become file paths, and the most common
/// target filesystems (default macOS APFS, Windows NTFS) are
/// case-insensitive, so two ids differing only by case (e.g. `Run` vs.
/// `run`) would still collide on disk even though a case-sensitive check
/// wouldn't catch it.
pub fn write_bundle(concepts: &[Concept], output_dir: &Path) -> Result<()> {
    let concepts = disambiguate_duplicate_ids(concepts);
    let concepts = concepts.as_slice();

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
    let by_dir = group_by_kind_dir(concepts);

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
        visibility: if concept.is_public {
            None
        } else {
            Some("private")
        },
        generated: GeneratedFrontmatter {
            by: PRODUCER,
            at: concept.generated_at,
        },
        relationships: relationships_frontmatter(concept),
    };
    let yaml = serde_yaml::to_string(&frontmatter)?;
    let yaml = yaml.strip_prefix("---\n").unwrap_or(&yaml);

    let mut body = String::new();

    if let Some(signature) = &concept.signature {
        body.push_str("# Signature\n\n`");
        body.push_str(signature);
        body.push_str("`\n\n");
    }

    let members = contains_members(concept, all);
    if !members.is_empty() {
        body.push_str("# Contains\n\n");
        for member in members {
            body.push_str(&format!(
                "- [{}]({})\n",
                member.name,
                relative_link(&concept.id, &member.id, "md")
            ));
        }
        body.push('\n');
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
        let rels =
            unique_by_kind_and_target(concept.relationships.iter().filter(|r| r.kind == kind));
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
                    relative_link(&concept.id, &rel.target, "md")
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
            relative_link(&pseudo_id, &concept.id, "md"),
            concept.frontmatter_type()
        ));
    }
    fs::write(output_dir.join(dir).join("index.md"), content)?;
    Ok(())
}

fn write_root_index(output_dir: &Path, by_dir: &BTreeMap<&str, Vec<&Concept>>) -> Result<()> {
    // A bundle-root `index.md` is the one place OKF permits frontmatter on
    // an index file (spec §8/§12), used here to declare the spec version
    // the bundle targets.
    let mut content =
        format!("---\nokf_version: \"{OKF_SPEC_VERSION}\"\n---\n\n# Knowledge Base\n\n");
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
            is_public: true,
            generated_at: None,
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
    fn disambiguates_duplicate_ids_instead_of_overwriting() {
        // Two same-named, same-file concepts — e.g. Rust's
        // `#[cfg(feature = "x")]` / `#[cfg(not(feature = "x"))]` stub
        // pair — must both survive as distinct files rather than one
        // silently overwriting the other.
        let dir = tempfile::tempdir().unwrap();
        let a = concept(ConceptKind::Function, "run", "run", "src/a.rs");
        let b = concept(ConceptKind::Function, "run", "run", "src/a.rs");
        write_bundle(&[a, b], dir.path()).unwrap();

        assert!(dir.path().join("functions/run.md").exists());
        assert!(dir.path().join("functions/run-2.md").exists());
    }

    #[test]
    fn disambiguates_ids_differing_only_by_case() {
        // On case-insensitive filesystems (default macOS, Windows), `Run`
        // and `run` are the same path, so this must be disambiguated even
        // though the ids aren't byte-for-byte equal.
        let dir = tempfile::tempdir().unwrap();
        let a = concept(ConceptKind::Function, "Run", "Run", "src/a.rs");
        let b = concept(ConceptKind::Function, "run", "run", "src/b.rs");
        write_bundle(&[a, b], dir.path()).unwrap();

        assert!(dir.path().join("functions/Run.md").exists());
        assert!(dir.path().join("functions/run-2.md").exists());
    }

    #[test]
    fn disambiguation_order_is_deterministic_by_location_not_input_order() {
        // The concept at the earlier source location keeps the unsuffixed
        // id regardless of which order they're passed in.
        let dir = tempfile::tempdir().unwrap();
        let mut later = concept(ConceptKind::Function, "run", "run", "src/a.rs");
        later.location.start_line = 20;
        let mut earlier = concept(ConceptKind::Function, "run", "run", "src/a.rs");
        earlier.location.start_line = 10;

        // Passed in reverse of source order.
        write_bundle(&[later, earlier], dir.path()).unwrap();

        let first_content = fs::read_to_string(dir.path().join("functions/run.md")).unwrap();
        assert!(first_content.contains("#L10"));
        let second_content = fs::read_to_string(dir.path().join("functions/run-2.md")).unwrap();
        assert!(second_content.contains("#L20"));
    }

    #[test]
    fn omits_visibility_for_public_and_marks_private() {
        let dir = tempfile::tempdir().unwrap();
        let public = concept(ConceptKind::Function, "run", "run", "src/a.rs");
        let mut private = concept(ConceptKind::Function, "helper", "helper", "src/a.rs");
        private.is_public = false;

        write_bundle(&[public, private], dir.path()).unwrap();

        let public_content = fs::read_to_string(dir.path().join("functions/run.md")).unwrap();
        assert!(!public_content.contains("visibility"));

        let private_content = fs::read_to_string(dir.path().join("functions/helper.md")).unwrap();
        assert!(private_content.contains("visibility: private"));
    }

    #[test]
    fn serializes_relationships_into_frontmatter() {
        let dir = tempfile::tempdir().unwrap();
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

        write_bundle(&[caller, callee], dir.path()).unwrap();

        let content =
            fs::read_to_string(dir.path().join("functions/auth/verify_token.md")).unwrap();
        assert!(content.contains("relationships:\n  calls:\n  - functions/auth/decode_jwt\n"));
    }

    #[test]
    fn calling_the_same_target_twice_renders_only_one_line() {
        // A function calling the same callee from two different call
        // sites in its body produces two `Relationship` entries with the
        // same target — both the frontmatter `relationships` list and
        // the `# Calls` body section should collapse that to one entry
        // rather than rendering it twice.
        let dir = tempfile::tempdir().unwrap();
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
        for _ in 0..2 {
            caller.relationships.push(Relationship {
                kind: RelationKind::Calls,
                target: callee.id.clone(),
                target_display: "decode_jwt".to_string(),
            });
        }

        write_bundle(&[caller, callee], dir.path()).unwrap();

        let content =
            fs::read_to_string(dir.path().join("functions/auth/verify_token.md")).unwrap();
        assert!(content.contains("relationships:\n  calls:\n  - functions/auth/decode_jwt\n"));
        assert_eq!(
            content
                .matches("[decode_jwt](../../functions/auth/decode_jwt.md)")
                .count(),
            1,
            "the `# Calls` section should list the repeated callee once: {content}"
        );
    }

    #[test]
    fn frontmatter_relationships_round_trip_through_read_bundle() {
        let dir = tempfile::tempdir().unwrap();
        let mut caller = concept(
            ConceptKind::Function,
            "verify_token",
            "auth.verify_token",
            "src/auth.rs",
        );
        let mut callee = concept(
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
        callee.relationships.push(Relationship {
            kind: RelationKind::CalledBy,
            target: caller.id.clone(),
            target_display: "verify_token".to_string(),
        });

        write_bundle(&[caller.clone(), callee.clone()], dir.path()).unwrap();
        let read_back = okf_parser::read_bundle(dir.path()).unwrap();

        assert_eq!(read_back.len(), 2);
        let read_caller = read_back.iter().find(|c| c.id == caller.id).unwrap();
        assert_eq!(read_caller.relationships.len(), 1);
        assert_eq!(read_caller.relationships[0].kind, RelationKind::Calls);
        assert_eq!(read_caller.relationships[0].target, callee.id);
        assert_eq!(read_caller.relationships[0].target_display, "decode_jwt");

        let read_callee = read_back.iter().find(|c| c.id == callee.id).unwrap();
        assert_eq!(read_callee.relationships[0].kind, RelationKind::CalledBy);
        assert_eq!(read_callee.relationships[0].target_display, "verify_token");
    }

    #[test]
    fn omits_relationships_field_when_concept_has_none() {
        let dir = tempfile::tempdir().unwrap();
        let solo = concept(ConceptKind::Function, "run", "run", "src/a.rs");
        write_bundle(&[solo], dir.path()).unwrap();

        let content = fs::read_to_string(dir.path().join("functions/run.md")).unwrap();
        assert!(!content.contains("relationships"));
    }

    #[test]
    fn package_contains_lists_its_member_modules_via_reverse_member_of() {
        let dir = tempfile::tempdir().unwrap();
        let package = concept(ConceptKind::Package, "demo", "demo", "Cargo.toml");
        let mut module = concept(ConceptKind::Module, "lib", "lib", "src/lib.rs");
        module.relationships.push(Relationship {
            kind: RelationKind::MemberOf,
            target: package.id.clone(),
            target_display: "demo".to_string(),
        });

        write_bundle(&[package.clone(), module.clone()], dir.path()).unwrap();

        let package_content = fs::read_to_string(dir.path().join("packages/demo.md")).unwrap();
        assert!(package_content.contains("# Contains"));
        assert!(package_content.contains(&format!(
            "[lib]({})",
            relative_link(&package.id, &module.id, "md")
        )));

        let module_content = fs::read_to_string(dir.path().join("modules/lib.md")).unwrap();
        assert!(
            module_content.contains("# Member of"),
            "the generic relationship rendering should show the module's own Member of section: {module_content}"
        );
    }
}
