//! Basic documentation generation, templated directly from a bundle's
//! concepts — no LLM required, and no markdown-parsing or HTML-templating
//! dependency: both renderers here build their output directly from the
//! structured [`Concept`] model (the same data `okf-generator` writes the
//! bundle from, or `okf_parser::read_bundle` reads back), the same way
//! `okf-generator` renders markdown from it.
//!
//! Two genuinely different outputs, not two views of the same thing:
//! - [`generate_html`] writes a browsable static site — one page per
//!   concept plus directory/root indexes — for when a plain OKF bundle
//!   (a directory of separate `.md` files, which is already exactly what
//!   `okf-generator` produces) isn't what's wanted; a real doc site a
//!   browser can open directly, no markdown renderer required.
//! - [`generate_markdown`] produces one consolidated document with a
//!   table of contents, which a bundle's one-file-per-concept shape
//!   doesn't give you — useful for pasting into a wiki/README, or
//!   feeding to a single-file-oriented tool.

mod pdf;

pub use pdf::generate_pdf;

use anyhow::Result;
use okf_parser::Concept;
use okf_render::{
    capitalize, contains_members, escape_markup, group_by_kind_dir, RELATIONSHIP_HEADINGS,
};
use std::collections::{BTreeMap, HashSet};
use std::fs;
use std::path::Path;

const STYLE: &str = r#"<style>
:root { color-scheme: light dark; }
body { font-family: -apple-system, BlinkMacSystemFont, "Segoe UI", Helvetica, Arial, sans-serif; max-width: 860px; margin: 2rem auto; padding: 0 1rem; line-height: 1.5; }
nav { font-size: 0.9rem; margin-bottom: 1.5rem; opacity: 0.8; }
nav a { color: inherit; }
h1 { margin-bottom: 0.25rem; }
.type { opacity: 0.7; margin-top: 0; font-size: 0.9rem; text-transform: uppercase; letter-spacing: 0.03em; }
.visibility { display: inline-block; background: #8882; padding: 0.1rem 0.5rem; border-radius: 0.3rem; font-size: 0.8rem; }
pre { background: #8881; padding: 0.75rem 1rem; border-radius: 0.4rem; overflow-x: auto; }
.resource, .tags { font-size: 0.85rem; opacity: 0.7; }
ul { padding-left: 1.25rem; }
a { color: #2563eb; text-decoration: none; }
a:hover { text-decoration: underline; }
</style>
"#;

/// Basename (no extension) used for the per-kind directory listing page,
/// e.g. `modules/@index.html`. Deliberately not `index`: a concept's own
/// slug can legitimately be the literal text `index` (e.g. a root-level
/// `index.js`, whose module id becomes `modules/index` -- see
/// `okf_tree_sitter::common::module_path`), which would otherwise collide
/// with this page at the exact same path and get silently overwritten.
/// `@` can't appear in an extracted identifier in any supported language,
/// so this can never collide with a real concept id.
const DIR_INDEX_NAME: &str = "@index";

/// Writes a static HTML documentation site for `concepts` to `output_dir`:
/// one page per concept (`<id>.html`), a per-kind-directory index, and a
/// root index — the same layout `okf-generator::write_bundle` uses for
/// the markdown bundle, just rendered to HTML with an embedded stylesheet
/// instead.
pub fn generate_html(concepts: &[Concept], output_dir: &Path) -> Result<()> {
    let bundle_ids: HashSet<&str> = concepts.iter().map(|c| c.id.as_str()).collect();
    let by_dir = group_by_kind_dir(concepts);

    fs::create_dir_all(output_dir)?;

    for concept in concepts {
        let file_path = output_dir.join(format!("{}.html", concept.id));
        if let Some(parent) = file_path.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(
            file_path,
            render_concept_html(concept, concepts, &bundle_ids),
        )?;
    }

    for (dir, entries) in &by_dir {
        write_dir_index_html(output_dir, dir, entries)?;
    }
    write_root_index_html(output_dir, &by_dir)?;

    Ok(())
}

/// Relative link from the page for `from_pseudo_id` (a concept id, or
/// `"<dir>/{DIR_INDEX_NAME}"` / `"index"` for an index page) to
/// `to_pseudo_id`, always with an `.html` extension — see
/// `okf_render::relative_link` for the shared depth-walking logic
/// `okf-generator` uses the same way for the `.md` bundle.
fn relative_link(from_pseudo_id: &str, to_pseudo_id: &str) -> String {
    okf_render::relative_link(from_pseudo_id, to_pseudo_id, "html")
}

fn html_page(title: &str, body: &str) -> String {
    format!(
        "<!doctype html>\n<html lang=\"en\">\n<head>\n<meta charset=\"utf-8\">\n<title>{title}</title>\n{STYLE}</head>\n<body>\n{body}</body>\n</html>\n"
    )
}

fn render_concept_html(concept: &Concept, all: &[Concept], bundle_ids: &HashSet<&str>) -> String {
    let name = escape_markup(&concept.name);
    let type_label = escape_markup(&concept.frontmatter_type());
    let home = relative_link(&concept.id, "index");
    let kind_dir = concept.kind.bundle_dir();
    let kind_index = relative_link(&concept.id, &format!("{kind_dir}/{DIR_INDEX_NAME}"));

    let mut body = String::new();
    body.push_str(&format!(
        "<nav><a href=\"{home}\">Home</a> / <a href=\"{kind_index}\">{}</a></nav>\n",
        escape_markup(kind_dir)
    ));
    body.push_str(&format!(
        "<h1>{name}</h1>\n<p class=\"type\">{type_label}</p>\n"
    ));

    if !concept.is_public {
        body.push_str("<p><span class=\"visibility\">private</span></p>\n");
    }

    if let Some(signature) = &concept.signature {
        body.push_str(&format!(
            "<pre><code>{}</code></pre>\n",
            escape_markup(signature)
        ));
    }

    if let Some(description) = &concept.description {
        body.push_str(&format!("<p>{}</p>\n", escape_markup(description)));
    }

    if !concept.tags.is_empty() {
        let tags: Vec<String> = concept.tags.iter().map(|t| escape_markup(t)).collect();
        body.push_str(&format!(
            "<p class=\"tags\">Tags: {}</p>\n",
            tags.join(", ")
        ));
    }

    let contains = contains_members(concept, all);
    render_link_section(&mut body, "Contains", &contains, &concept.id);

    for (kind, heading) in RELATIONSHIP_HEADINGS {
        let rels: Vec<_> = concept
            .relationships
            .iter()
            .filter(|r| r.kind == kind)
            .collect();
        if rels.is_empty() {
            continue;
        }
        body.push_str(&format!("<h2>{heading}</h2>\n<ul>\n"));
        for rel in rels {
            if bundle_ids.contains(rel.target.as_str()) {
                body.push_str(&format!(
                    "<li><a href=\"{}\">{}</a></li>\n",
                    relative_link(&concept.id, &rel.target),
                    escape_markup(&rel.target_display)
                ));
            } else {
                body.push_str(&format!(
                    "<li><code>{}</code></li>\n",
                    escape_markup(&rel.target_display)
                ));
            }
        }
        body.push_str("</ul>\n");
    }

    body.push_str(&format!(
        "<p class=\"resource\">Source: {}</p>\n",
        escape_markup(&concept.location.to_string())
    ));

    html_page(&format!("{name} — {type_label}"), &body)
}

fn render_link_section(body: &mut String, heading: &str, members: &[&Concept], from_id: &str) {
    if members.is_empty() {
        return;
    }
    body.push_str(&format!("<h2>{heading}</h2>\n<ul>\n"));
    for member in members {
        body.push_str(&format!(
            "<li><a href=\"{}\">{}</a> — {}</li>\n",
            relative_link(from_id, &member.id),
            escape_markup(&member.name),
            escape_markup(&member.frontmatter_type())
        ));
    }
    body.push_str("</ul>\n");
}

fn write_dir_index_html(output_dir: &Path, dir: &str, entries: &[&Concept]) -> Result<()> {
    let pseudo_id = format!("{dir}/{DIR_INDEX_NAME}");
    let home = relative_link(&pseudo_id, "index");
    let title = capitalize(dir);

    let mut body = format!(
        "<nav><a href=\"{home}\">Home</a></nav>\n<h1>{}</h1>\n<ul>\n",
        escape_markup(&title)
    );
    for concept in entries {
        body.push_str(&format!(
            "<li><a href=\"{}\">{}</a> — {}</li>\n",
            relative_link(&pseudo_id, &concept.id),
            escape_markup(&concept.name),
            escape_markup(&concept.frontmatter_type())
        ));
    }
    body.push_str("</ul>\n");

    fs::write(
        output_dir.join(dir).join(format!("{DIR_INDEX_NAME}.html")),
        html_page(&title, &body),
    )?;
    Ok(())
}

fn write_root_index_html(output_dir: &Path, by_dir: &BTreeMap<&str, Vec<&Concept>>) -> Result<()> {
    let mut body = String::from("<h1>Knowledge Base</h1>\n<ul>\n");
    for (dir, entries) in by_dir {
        body.push_str(&format!(
            "<li><a href=\"{dir}/{DIR_INDEX_NAME}.html\">{}</a> ({})</li>\n",
            escape_markup(&capitalize(dir)),
            entries.len()
        ));
    }
    body.push_str("</ul>\n");

    fs::write(
        output_dir.join("index.html"),
        html_page("Knowledge Base", &body),
    )?;
    Ok(())
}

/// Renders `concepts` as one consolidated Markdown document with a table
/// of contents and one section per concept, grouped by kind — a
/// genuinely different shape from the bundle's own one-file-per-concept
/// layout, for consumers that want a single file (a wiki page, a README
/// section, a search-in-one-document reference) rather than a directory.
pub fn generate_markdown(concepts: &[Concept]) -> String {
    let by_dir = group_by_kind_dir(concepts);

    let mut out = String::from("# Documentation\n\n## Contents\n\n");
    for (dir, entries) in &by_dir {
        out.push_str(&format!(
            "- [{}](#{}) ({})\n",
            capitalize(dir),
            slug(dir),
            entries.len()
        ));
    }
    out.push('\n');

    for (dir, entries) in &by_dir {
        out.push_str(&format!("## {}\n\n", capitalize(dir)));
        for concept in entries {
            out.push_str(&format!("### {}\n\n", concept.name));
            out.push_str(&format!(
                "*{}* — `{}`\n\n",
                concept.frontmatter_type(),
                concept.location
            ));
            if !concept.is_public {
                out.push_str("_private_\n\n");
            }
            if let Some(signature) = &concept.signature {
                out.push_str(&format!("```\n{signature}\n```\n\n"));
            }
            if let Some(description) = &concept.description {
                out.push_str(&format!("{description}\n\n"));
            }
            if !concept.tags.is_empty() {
                out.push_str(&format!("Tags: {}\n\n", concept.tags.join(", ")));
            }
            for (kind, heading) in RELATIONSHIP_HEADINGS {
                let rels: Vec<&str> = concept
                    .relationships
                    .iter()
                    .filter(|r| r.kind == kind)
                    .map(|r| r.target_display.as_str())
                    .collect();
                if rels.is_empty() {
                    continue;
                }
                out.push_str(&format!("**{heading}:** {}\n\n", rels.join(", ")));
            }
        }
    }
    out
}

/// A GitHub-flavored-markdown-style heading anchor: lowercase,
/// non-alphanumerics collapsed to `-`. `dir` values here (`functions`,
/// `classes`, ...) are already simple lowercase words, so this is really
/// just future-proofing rather than doing real slugification work.
fn slug(s: &str) -> String {
    s.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() {
                c.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect()
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
    fn writes_html_site_with_index_and_links() {
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

        let concepts = vec![module, caller, callee];
        generate_html(&concepts, dir.path()).unwrap();

        assert!(dir.path().join("index.html").exists());
        assert!(dir.path().join("modules/@index.html").exists());
        assert!(dir.path().join("functions/@index.html").exists());
        assert!(dir.path().join("modules/auth.html").exists());
        assert!(dir.path().join("functions/auth/verify_token.html").exists());

        let content =
            fs::read_to_string(dir.path().join("functions/auth/verify_token.html")).unwrap();
        assert!(content.contains("<h1>verify_token</h1>"));
        assert!(content.contains("Rust Function"));
        assert!(content.contains("<pre><code>fn verify_token()</code></pre>"));
        assert!(!content.contains("[decode_jwt]")); // no markdown syntax leaks into HTML
        assert!(content.contains("../../functions/auth/decode_jwt.html"));
    }

    #[test]
    fn escapes_signatures_containing_generics() {
        let dir = tempfile::tempdir().unwrap();
        let mut fn_concept = concept(ConceptKind::Function, "wrap", "wrap", "src/lib.rs");
        fn_concept.signature = Some("fn wrap<T: Clone>(x: &T) -> Vec<T>".to_string());
        fn_concept.description = Some("Uses <angle brackets> & ampersands".to_string());

        generate_html(&[fn_concept], dir.path()).unwrap();
        let content = fs::read_to_string(dir.path().join("functions/wrap.html")).unwrap();

        assert!(content.contains("fn wrap&lt;T: Clone&gt;(x: &amp;T) -&gt; Vec&lt;T&gt;"));
        assert!(content.contains("Uses &lt;angle brackets&gt; &amp; ampersands"));
        assert!(
            !content.contains("Vec<T>"),
            "raw unescaped generics would break the HTML"
        );
    }

    #[test]
    fn package_html_page_lists_member_modules() {
        let dir = tempfile::tempdir().unwrap();
        let package = concept(ConceptKind::Package, "demo", "demo", "Cargo.toml");
        let mut module = concept(ConceptKind::Module, "lib", "lib", "src/lib.rs");
        module.relationships.push(Relationship {
            kind: RelationKind::MemberOf,
            target: package.id.clone(),
            target_display: "demo".to_string(),
        });

        generate_html(&[package.clone(), module.clone()], dir.path()).unwrap();

        let package_content = fs::read_to_string(dir.path().join("packages/demo.html")).unwrap();
        assert!(package_content.contains("<h2>Contains</h2>"));
        assert!(package_content.contains("../modules/lib.html"));

        let module_content = fs::read_to_string(dir.path().join("modules/lib.html")).unwrap();
        assert!(module_content.contains("<h2>Member of</h2>"));
    }

    #[test]
    fn markdown_has_table_of_contents_and_per_concept_sections() {
        let module = concept(ConceptKind::Module, "auth", "auth", "src/auth.rs");
        let mut private_fn = concept(
            ConceptKind::Function,
            "helper",
            "auth.helper",
            "src/auth.rs",
        );
        private_fn.is_public = false;
        private_fn.tags = vec!["internal".to_string()];

        let markdown = generate_markdown(&[module, private_fn]);

        assert!(markdown.starts_with("# Documentation"));
        assert!(markdown.contains("## Contents"));
        assert!(markdown.contains("[Modules](#modules)"));
        assert!(markdown.contains("[Functions](#functions)"));
        assert!(markdown.contains("### helper"));
        assert!(markdown.contains("_private_"));
        assert!(markdown.contains("Tags: internal"));
    }

    #[test]
    fn markdown_lists_relationships_by_display_name() {
        let mut caller = concept(ConceptKind::Function, "a", "a", "src/lib.rs");
        caller.relationships.push(Relationship {
            kind: RelationKind::Calls,
            target: "functions/b".to_string(),
            target_display: "b".to_string(),
        });

        let markdown = generate_markdown(&[caller]);
        assert!(markdown.contains("**Calls:** b"));
    }

    #[test]
    fn a_module_whose_id_is_literally_index_does_not_collide_with_the_directory_index() {
        // A root-level `index.js`-style file collapses to the module id
        // `modules/index` (see `okf_tree_sitter::common::module_path`),
        // which used to be exactly the same output path as the
        // `modules/` directory's own listing page.
        let module = concept(ConceptKind::Module, "index", "index", "index.js");
        let dir = tempfile::tempdir().unwrap();
        generate_html(&[module], dir.path()).unwrap();

        let module_page = fs::read_to_string(dir.path().join("modules/index.html")).unwrap();
        assert!(
            module_page.contains("<h1>index</h1>"),
            "the module's own page must not be overwritten by the directory index: {module_page}"
        );

        let dir_index = fs::read_to_string(dir.path().join("modules/@index.html")).unwrap();
        assert!(dir_index.contains("Modules"));
    }
}
