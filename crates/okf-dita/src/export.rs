//! Export: writes an OKF bundle's concepts as DITA topics plus a root
//! ditamap — the "DITA (planned)" documentation-generation format the
//! spec named alongside Markdown/HTML/PDF, `okf-docs`'s own sibling
//! formats. It lives in its own crate rather than `okf-docs` only
//! because it also owns the reverse direction ([`crate::import_dita`]),
//! not because export alone couldn't have lived there.
//!
//! Uses the generic `<topic>` DITA doctype (the base every specialized
//! topic type — `<concept>`, `<task>`, `<reference>` — inherits from)
//! rather than a specialization, since a specialization's own validity
//! rules (required child-element ordering, etc.) would need per-concept-
//! kind logic to satisfy correctly for no benefit a generic topic
//! doesn't already give a DITA toolkit that opens this output.

use anyhow::{Context, Result};
use okf_parser::Concept;
use okf_render::{capitalize, escape_markup, group_by_kind_dir, RELATIONSHIP_HEADINGS};
use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

/// Writes `concepts` as one DITA topic per concept (`<id>.dita`, mirroring
/// the bundle's own per-kind directory layout) plus a root `root.ditamap`
/// linking every topic, grouped by kind — the same content
/// `okf_docs::generate_markdown`/`generate_html` render, just as DITA XML.
pub fn export_dita(concepts: &[Concept], output_dir: &Path) -> Result<()> {
    fs::create_dir_all(output_dir)
        .with_context(|| format!("failed to create {}", output_dir.display()))?;

    for concept in concepts {
        let file_path = output_dir.join(format!("{}.dita", concept.id));
        if let Some(parent) = file_path.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(&file_path, render_topic(concept))
            .with_context(|| format!("failed to write {}", file_path.display()))?;
    }

    let by_dir = group_by_kind_dir(concepts);
    let map_path = output_dir.join("root.ditamap");
    fs::write(&map_path, render_map(&by_dir))
        .with_context(|| format!("failed to write {}", map_path.display()))?;

    Ok(())
}

/// A DITA topic `id` attribute must be a valid XML `NAME` token (no
/// `/`), unlike a concept id — `/`-separated path segments collapse to
/// `-` for the attribute, while the *filename* still uses the real `/`
/// path (matching every other bundle-derived output format).
fn topic_id(concept_id: &str) -> String {
    concept_id.replace('/', "-")
}

fn render_topic(concept: &Concept) -> String {
    let mut body = String::new();

    if let Some(signature) = &concept.signature {
        body.push_str(&format!(
            "<section><title>Signature</title><codeblock>{}</codeblock></section>\n",
            escape_markup(signature)
        ));
    }
    body.push_str(&format!(
        "<section><title>Type</title><p>{}</p></section>\n",
        escape_markup(&concept.frontmatter_type())
    ));
    body.push_str(&format!(
        "<section><title>Location</title><p>{}</p></section>\n",
        escape_markup(&concept.location.to_string())
    ));
    if !concept.tags.is_empty() {
        body.push_str(&format!(
            "<section><title>Tags</title><p>{}</p></section>\n",
            escape_markup(&concept.tags.join(", "))
        ));
    }
    for (kind, heading) in RELATIONSHIP_HEADINGS {
        let targets: Vec<&str> = concept
            .relationships
            .iter()
            .filter(|r| r.kind == kind)
            .map(|r| r.target_display.as_str())
            .collect();
        if targets.is_empty() {
            continue;
        }
        body.push_str(&format!("<section><title>{heading}</title><ul>\n"));
        for target in targets {
            body.push_str(&format!("<li>{}</li>\n", escape_markup(target)));
        }
        body.push_str("</ul></section>\n");
    }

    let shortdesc = concept
        .description
        .as_deref()
        .map(|d| format!("<shortdesc>{}</shortdesc>\n", escape_markup(d)))
        .unwrap_or_default();

    format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<!DOCTYPE topic PUBLIC \"-//OASIS//DTD DITA Topic//EN\" \"topic.dtd\">\n<topic id=\"{}\">\n<title>{}</title>\n{shortdesc}<body>\n{body}</body>\n</topic>\n",
        topic_id(&concept.id),
        escape_markup(&concept.name),
    )
}

fn render_map(by_dir: &BTreeMap<&str, Vec<&Concept>>) -> String {
    let mut out = String::from(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<!DOCTYPE map PUBLIC \"-//OASIS//DTD DITA Map//EN\" \"map.dtd\">\n<map>\n<title>Documentation</title>\n",
    );
    for (dir, entries) in by_dir {
        out.push_str(&format!(
            "<topichead navtitle=\"{}\">\n",
            escape_markup(&capitalize(dir))
        ));
        for concept in entries {
            out.push_str(&format!(
                "<topicref href=\"{}.dita\"/>\n",
                escape_markup(&concept.id)
            ));
        }
        out.push_str("</topichead>\n");
    }
    out.push_str("</map>\n");
    out
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
    fn writes_a_topic_per_concept_and_a_root_map() {
        let dir = tempfile::tempdir().unwrap();
        let module = concept(ConceptKind::Module, "auth", "auth", "src/auth.rs");
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
        caller.description = Some("Verifies a signed token.".to_string());
        caller.tags = vec!["auth".to_string()];

        let concepts = vec![module, caller, callee];
        export_dita(&concepts, dir.path()).unwrap();

        assert!(dir.path().join("root.ditamap").exists());
        assert!(dir
            .path()
            .join("functions/auth/verify_token.dita")
            .exists());

        let content =
            fs::read_to_string(dir.path().join("functions/auth/verify_token.dita")).unwrap();
        assert!(content.starts_with("<?xml version=\"1.0\" encoding=\"UTF-8\"?>"));
        assert!(content.contains("<topic id=\"functions-auth-verify_token\">"));
        assert!(content.contains("<title>verify_token</title>"));
        assert!(content.contains("<shortdesc>Verifies a signed token.</shortdesc>"));
        assert!(content.contains("<codeblock>fn verify_token()</codeblock>"));
        assert!(content.contains("<li>decode_jwt</li>"));
        assert!(content.contains("<p>auth</p>"));

        let map = fs::read_to_string(dir.path().join("root.ditamap")).unwrap();
        assert!(map.contains("<topicref href=\"functions/auth/verify_token.dita\"/>"));
        assert!(map.contains("<topicref href=\"modules/auth.dita\"/>"));
    }

    #[test]
    fn escapes_signatures_and_descriptions_containing_generics() {
        let dir = tempfile::tempdir().unwrap();
        let mut fn_concept = concept(ConceptKind::Function, "wrap", "wrap", "src/lib.rs");
        fn_concept.signature = Some("fn wrap<T: Clone>(x: &T) -> Vec<T>".to_string());
        fn_concept.description = Some("Uses <angle brackets> & ampersands".to_string());

        export_dita(&[fn_concept], dir.path()).unwrap();
        let content = fs::read_to_string(dir.path().join("functions/wrap.dita")).unwrap();

        assert!(content.contains("fn wrap&lt;T: Clone&gt;(x: &amp;T) -&gt; Vec&lt;T&gt;"));
        assert!(content.contains("Uses &lt;angle brackets&gt; &amp; ampersands"));
        assert!(
            !content.contains("Vec<T>"),
            "raw unescaped generics would break the XML"
        );
    }
}
