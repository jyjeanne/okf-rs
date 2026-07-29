//! Small, pure traversal helpers shared by okf-rs's two bundle renderers:
//! `okf-generator` (the Markdown OKF bundle) and `okf-docs` (the static
//! HTML site and consolidated Markdown doc). Both group the same concepts
//! into the same per-kind directory layout, link between pages the same
//! "walk up to the root, back down" way, and resolve a `Module`/`Package`
//! concept's "Contains" members the same way — only the actual per-concept
//! templating (frontmatter+Markdown vs. styled HTML) differs, and that
//! stays in each renderer's own crate.

use okf_parser::{Concept, ConceptKind, RelationKind};
use std::collections::BTreeMap;

/// Groups `concepts` by their kind's bundle directory (`functions`,
/// `modules`, ...), with each group's entries sorted by id — the layout
/// every bundle/doc renderer builds its per-directory and root indexes
/// from.
pub fn group_by_kind_dir(concepts: &[Concept]) -> BTreeMap<&str, Vec<&Concept>> {
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
    by_dir
}

/// Relative link from the page/file for `from_pseudo_id` (a concept id, or
/// a directory/root index's own pseudo-id, e.g. `"<dir>/index"`) to
/// `to_pseudo_id`, with `ext` appended (`"md"` for the Markdown bundle,
/// `"html"` for the HTML site). Always correct (walks up to the bundle
/// root and back down), though not always the shortest possible path.
pub fn relative_link(from_pseudo_id: &str, to_pseudo_id: &str, ext: &str) -> String {
    let depth = from_pseudo_id.matches('/').count();
    let up = "../".repeat(depth);
    format!("{up}{to_pseudo_id}.{ext}")
}

/// A concept's own "Contains" members: a `Module`'s are every other
/// concept declared in the same file; a `Package`'s are every `Module`
/// that named it in a `MemberOf` relationship (see `okf-analyzer`'s
/// workspace/monorepo aggregation) — two different ways of finding "what's
/// inside this container," since only one of them (file identity) needs an
/// explicit relationship to express. Every other kind has none.
pub fn contains_members<'a>(concept: &Concept, all: &'a [Concept]) -> Vec<&'a Concept> {
    match concept.kind {
        ConceptKind::Module => all
            .iter()
            .filter(|c| c.id != concept.id && c.location.file == concept.location.file)
            .collect(),
        ConceptKind::Package => all
            .iter()
            .filter(|c| {
                c.relationships
                    .iter()
                    .any(|r| r.kind == RelationKind::MemberOf && r.target == concept.id)
            })
            .collect(),
        _ => Vec::new(),
    }
}

/// Uppercases a directory name's first character for display (`"functions"`
/// -> `"Functions"`).
pub fn capitalize(s: &str) -> String {
    let mut chars = s.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
        None => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use okf_parser::{Language, Location, Relationship};

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
            signature: None,
            tags: Vec::new(),
            is_public: true,
            timestamp: None,
            relationships: Vec::new(),
        }
    }

    #[test]
    fn groups_by_kind_dir_sorted_by_id() {
        let a = concept(ConceptKind::Function, "b_fn", "b_fn", "src/a.rs");
        let b = concept(ConceptKind::Function, "a_fn", "a_fn", "src/a.rs");
        let module = concept(ConceptKind::Module, "a", "a", "src/a.rs");

        let concepts = [a, b, module];
        let by_dir = group_by_kind_dir(&concepts);
        let functions = &by_dir["functions"];
        assert_eq!(functions.len(), 2);
        assert!(functions[0].id < functions[1].id, "should be sorted by id");
        assert_eq!(by_dir["modules"].len(), 1);
    }

    #[test]
    fn relative_link_walks_up_by_depth_and_appends_extension() {
        assert_eq!(
            relative_link("functions/a", "functions/b", "md"),
            "../functions/b.md"
        );
        assert_eq!(
            relative_link("functions/auth/verify", "functions/auth/decode", "html"),
            "../../functions/auth/decode.html"
        );
    }

    #[test]
    fn module_contains_is_every_other_concept_in_the_same_file() {
        let module = concept(ConceptKind::Module, "auth", "auth", "src/auth.rs");
        let f1 = concept(ConceptKind::Function, "a", "auth.a", "src/auth.rs");
        let f2 = concept(ConceptKind::Function, "b", "auth.b", "src/other.rs");
        let all = vec![module.clone(), f1.clone(), f2];

        let members = contains_members(&module, &all);
        assert_eq!(members.len(), 1);
        assert_eq!(members[0].id, f1.id);
    }

    #[test]
    fn package_contains_is_every_module_with_a_matching_member_of() {
        let package = concept(ConceptKind::Package, "demo", "demo", "Cargo.toml");
        let mut module = concept(ConceptKind::Module, "lib", "lib", "src/lib.rs");
        module.relationships.push(Relationship {
            kind: RelationKind::MemberOf,
            target: package.id.clone(),
            target_display: "demo".to_string(),
        });
        let all = vec![package.clone(), module.clone()];

        let members = contains_members(&package, &all);
        assert_eq!(members.len(), 1);
        assert_eq!(members[0].id, module.id);
    }

    #[test]
    fn non_container_kinds_have_no_contains_members() {
        let function = concept(ConceptKind::Function, "f", "f", "src/a.rs");
        assert!(contains_members(&function, std::slice::from_ref(&function)).is_empty());
    }

    #[test]
    fn capitalize_uppercases_first_char_only() {
        assert_eq!(capitalize("functions"), "Functions");
        assert_eq!(capitalize(""), "");
    }
}
