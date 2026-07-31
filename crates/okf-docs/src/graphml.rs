//! GraphML export: a directed graph over the bundle's concepts and
//! relationships, for opening in a general-purpose graph tool (Gephi,
//! yEd, or any other GraphML-reading application) — a deterministic,
//! offline visualization path that doesn't require `okf-server`'s
//! eventual interactive explorer.
//!
//! Hand-templated XML, the same "no rendering dependency for a
//! write-only export" choice `okf-docs`'s other renderers already make
//! for HTML/Markdown/PDF.

use okf_parser::{Concept, RelationKind};
use okf_render::{escape_markup, RELATIONSHIP_HEADINGS};
use std::collections::HashSet;

/// Whether `kind` gets rendered as a GraphML edge — every relationship
/// kind [`okf_render::RELATIONSHIP_HEADINGS`] knows about (the same
/// complete kind list the HTML/Markdown/Obsidian renderers already
/// consume) except `CalledBy`: `okf-analyzer` always emits `Calls` and
/// `CalledBy` as two sides of the same resolved edge (see
/// `okf-validator`'s symmetry check), so including both would
/// double-count every call edge in the exported graph. Deriving this
/// from `RELATIONSHIP_HEADINGS` instead of a second, independently
/// maintained kind list means a future `RelationKind` variant shows up
/// here automatically, the same way it would for every other renderer.
fn is_edge_kind(kind: RelationKind) -> bool {
    kind != RelationKind::CalledBy && RELATIONSHIP_HEADINGS.iter().any(|(k, _)| *k == kind)
}

/// Renders `concepts` as a single GraphML document: one `<node>` per
/// concept (`label`/`kind`/`language`/`public` attributes) and one
/// `<edge>` per relationship whose kind passes [`is_edge_kind`] and
/// whose target is another concept in the same bundle (an external or
/// otherwise unresolved target has no node to draw an edge to).
pub fn generate_graphml(concepts: &[Concept]) -> String {
    let mut out = String::from(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<graphml xmlns=\"http://graphml.graphdrawing.org/xmlns\">\n",
    );
    out.push_str("  <key id=\"label\" for=\"node\" attr.name=\"label\" attr.type=\"string\"/>\n");
    out.push_str("  <key id=\"kind\" for=\"node\" attr.name=\"kind\" attr.type=\"string\"/>\n");
    out.push_str(
        "  <key id=\"language\" for=\"node\" attr.name=\"language\" attr.type=\"string\"/>\n",
    );
    out.push_str(
        "  <key id=\"public\" for=\"node\" attr.name=\"public\" attr.type=\"boolean\"/>\n",
    );
    out.push_str(
        "  <key id=\"relation\" for=\"edge\" attr.name=\"relation\" attr.type=\"string\"/>\n",
    );
    out.push_str("  <graph id=\"okf\" edgedefault=\"directed\">\n");

    for concept in concepts {
        out.push_str(&format!(
            "    <node id=\"{}\">\n      <data key=\"label\">{}</data>\n      <data key=\"kind\">{}</data>\n      <data key=\"language\">{}</data>\n      <data key=\"public\">{}</data>\n    </node>\n",
            escape_markup(&concept.id),
            escape_markup(&concept.name),
            escape_markup(&concept.frontmatter_type()),
            escape_markup(concept.language.display_name()),
            concept.is_public,
        ));
    }

    let ids: HashSet<&str> = concepts.iter().map(|c| c.id.as_str()).collect();
    for concept in concepts {
        for rel in &concept.relationships {
            if !is_edge_kind(rel.kind) || !ids.contains(rel.target.as_str()) {
                continue;
            }
            out.push_str(&format!(
                "    <edge source=\"{}\" target=\"{}\">\n      <data key=\"relation\">{}</data>\n    </edge>\n",
                escape_markup(&concept.id),
                escape_markup(&rel.target),
                escape_markup(rel.kind.label()),
            ));
        }
    }

    out.push_str("  </graph>\n</graphml>\n");
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use okf_parser::{ConceptKind, Language, Location, Relationship};

    fn concept(id: &str, kind: ConceptKind, is_public: bool) -> Concept {
        Concept {
            id: id.to_string(),
            kind,
            language: Language::Rust,
            name: id.rsplit('/').next().unwrap().to_string(),
            qualified_name: id.to_string(),
            description: None,
            location: Location {
                file: "src/lib.rs".to_string(),
                start_line: 1,
                end_line: 1,
            },
            signature: None,
            tags: Vec::new(),
            is_public,
            generated_at: None,
            relationships: Vec::new(),
        }
    }

    #[test]
    fn renders_a_well_formed_document_with_nodes_and_edges() {
        let mut caller = concept("functions/a", ConceptKind::Function, true);
        let callee = concept("functions/b", ConceptKind::Function, false);
        caller.relationships.push(Relationship {
            kind: RelationKind::Calls,
            target: "functions/b".to_string(),
            target_display: "b".to_string(),
        });

        let xml = generate_graphml(&[caller, callee]);

        assert!(xml.starts_with("<?xml version=\"1.0\" encoding=\"UTF-8\"?>"));
        assert!(xml.contains("<node id=\"functions/a\">"));
        assert!(xml.contains("<node id=\"functions/b\">"));
        assert!(xml.contains("<data key=\"public\">true</data>"));
        assert!(xml.contains("<data key=\"public\">false</data>"));
        assert!(xml.contains("<edge source=\"functions/a\" target=\"functions/b\">"));
        assert!(xml.contains("<data key=\"relation\">Calls</data>"));
    }

    #[test]
    fn called_by_edges_are_omitted_to_avoid_doubling_call_edges() {
        let mut caller = concept("functions/a", ConceptKind::Function, true);
        let mut callee = concept("functions/b", ConceptKind::Function, true);
        caller.relationships.push(Relationship {
            kind: RelationKind::Calls,
            target: "functions/b".to_string(),
            target_display: "b".to_string(),
        });
        callee.relationships.push(Relationship {
            kind: RelationKind::CalledBy,
            target: "functions/a".to_string(),
            target_display: "a".to_string(),
        });

        let xml = generate_graphml(&[caller, callee]);
        assert_eq!(xml.matches("<edge ").count(), 1, "{xml}");
    }

    #[test]
    fn a_relationship_target_outside_the_bundle_produces_no_dangling_edge() {
        let mut caller = concept("functions/a", ConceptKind::Function, true);
        caller.relationships.push(Relationship {
            kind: RelationKind::Calls,
            target: "external/std::io::Read::read".to_string(),
            target_display: "Read::read".to_string(),
        });

        let xml = generate_graphml(&[caller]);
        assert_eq!(xml.matches("<edge ").count(), 0, "{xml}");
    }

    #[test]
    fn escapes_special_characters_in_signatures_and_names() {
        let mut c = concept("functions/wrap", ConceptKind::Function, true);
        c.name = "wrap<T>".to_string();
        let xml = generate_graphml(&[c]);
        assert!(xml.contains("wrap&lt;T&gt;"));
        assert!(!xml.contains("wrap<T>"));
    }
}
