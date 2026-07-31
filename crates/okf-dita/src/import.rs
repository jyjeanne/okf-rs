//! Import: reads an existing DITA XML corpus (a single topic file, or a
//! directory of them) and produces `Document` concepts — so a technical-
//! writing team's existing DITA corpus can be brought into the same
//! bundle/CLI/MCP/graph queries every code-derived concept already works
//! with, the way the roadmap itself frames this half of the converter.
//!
//! Deliberately tolerant, not a validating DITA parser: a topic missing
//! a `<title>` still imports (falling back to a title derived from its
//! filename), and a file that isn't even well-formed XML is skipped
//! rather than failing the whole import — reported back to the caller
//! (see [`import_dita`]'s return type) instead of silently dropped,
//! since a real DITA corpus can be large and one malformed topic
//! shouldn't block importing the rest of it.

use anyhow::{Context, Result};
use okf_parser::{Concept, ConceptKind, Language, Location};
use roxmltree::{Document, Node, ParsingOptions};
use std::path::Path;

/// A real DITA topic (and `okf-dita`'s own export) declares a `<!DOCTYPE
/// topic PUBLIC "-//OASIS//DTD DITA Topic//EN" "topic.dtd">` prolog —
/// `roxmltree` rejects any `DOCTYPE` by default (`allow_dtd: false`), so
/// this must be enabled to import real-world DITA content at all. Safe
/// to allow here: `roxmltree` is a purely local, non-validating parser
/// that never fetches an external DTD subset over the network or
/// filesystem to resolve it, so this doesn't introduce an XXE-style
/// risk — it only tolerates the doctype *declaration's syntax*.
fn parsing_options() -> ParsingOptions {
    ParsingOptions {
        allow_dtd: true,
        ..ParsingOptions::default()
    }
}

/// Reads every `.dita`/`.xml` file under `input_path` — recursively, if
/// it's a directory; just that one file, if it's a file — and converts
/// each into a `Document` concept. Returns the successfully imported
/// concepts alongside a human-readable line per file that couldn't be
/// imported (malformed XML, or an empty/unreadable file), sorted by
/// relative path for deterministic output either way.
pub fn import_dita(input_path: &Path) -> Result<(Vec<Concept>, Vec<String>)> {
    let (base, mut files) = if input_path.is_file() {
        (
            input_path.parent().unwrap_or(input_path).to_path_buf(),
            vec![input_path.to_path_buf()],
        )
    } else {
        let files = walkdir::WalkDir::new(input_path)
            .into_iter()
            .filter_map(|entry| entry.ok())
            .filter(|entry| entry.file_type().is_file())
            .map(|entry| entry.into_path())
            .filter(|path| {
                matches!(
                    path.extension().and_then(|e| e.to_str()),
                    Some("dita") | Some("xml")
                )
            })
            .collect();
        (input_path.to_path_buf(), files)
    };
    files.sort();

    let mut concepts = Vec::new();
    let mut skipped = Vec::new();
    for path in files {
        let relative = path
            .strip_prefix(&base)
            .unwrap_or(&path)
            .to_string_lossy()
            .replace('\\', "/");
        match import_file(&path, &relative) {
            Ok(concept) => concepts.push(concept),
            Err(e) => skipped.push(format!("{relative}: {e:#}")),
        }
    }

    Ok((concepts, skipped))
}

fn import_file(path: &Path, relative: &str) -> Result<Concept> {
    let content = std::fs::read_to_string(path)
        .with_context(|| format!("failed to read {}", path.display()))?;
    let doc = Document::parse_with_options(&content, parsing_options())
        .context("not well-formed XML")?;
    let root = doc.root_element();

    let title = find_child_text(root, "title")
        .filter(|t| !t.trim().is_empty())
        .unwrap_or_else(|| fallback_title(relative));
    let description = find_child_text(root, "shortdesc").filter(|d| !d.trim().is_empty());

    // A topic's own DITA `id` is the more meaningful identity when
    // present (stable even if the file gets moved/renamed within the
    // corpus); the relative path is the honest fallback otherwise.
    let qualified_name = root
        .attribute("id")
        .filter(|id| !id.trim().is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| path_to_qualified_name(relative));

    Ok(Concept {
        id: Concept::make_id(ConceptKind::Document, &qualified_name),
        kind: ConceptKind::Document,
        language: Language::Dita,
        name: title,
        qualified_name,
        description,
        location: Location {
            file: relative.to_string(),
            start_line: 1,
            end_line: content.lines().count().max(1),
        },
        signature: None,
        tags: Vec::new(),
        is_public: true,
        generated_at: None,
        relationships: Vec::new(),
    })
}

/// The text of `parent`'s first direct child element named `tag`,
/// concatenating every descendant text node (not just a lone one) so
/// inline markup (`<b>...</b>` mixed with plain text, common in a real
/// `<shortdesc>`) doesn't silently disappear.
fn find_child_text(parent: Node, tag: &str) -> Option<String> {
    let child = parent.children().find(|n| n.has_tag_name(tag))?;
    let text: String = child
        .descendants()
        .filter(|n| n.is_text())
        .filter_map(|n| n.text())
        .collect();
    Some(text)
}

fn fallback_title(relative: &str) -> String {
    Path::new(relative)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or(relative)
        .to_string()
}

fn path_to_qualified_name(relative: &str) -> String {
    let no_ext = relative
        .rsplit_once('.')
        .map(|(base, _)| base)
        .unwrap_or(relative);
    no_ext.replace('/', ".")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn write(dir: &Path, relative: &str, content: &str) {
        let path = dir.join(relative);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, content).unwrap();
    }

    #[test]
    fn imports_a_single_topic_with_title_and_shortdesc() {
        let dir = tempfile::tempdir().unwrap();
        write(
            dir.path(),
            "auth/verify-token.dita",
            r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE topic PUBLIC "-//OASIS//DTD DITA Topic//EN" "topic.dtd">
<topic id="verify-token">
  <title>Verify Token</title>
  <shortdesc>Explains how token verification works.</shortdesc>
  <body><p>Details here.</p></body>
</topic>
"#,
        );

        let (concepts, skipped) = import_dita(dir.path()).unwrap();
        assert!(skipped.is_empty(), "unexpected skips: {skipped:?}");
        assert_eq!(concepts.len(), 1);

        let concept = &concepts[0];
        assert_eq!(concept.kind, ConceptKind::Document);
        assert_eq!(concept.language, Language::Dita);
        assert_eq!(concept.name, "Verify Token");
        assert_eq!(
            concept.description.as_deref(),
            Some("Explains how token verification works.")
        );
        assert_eq!(concept.qualified_name, "verify-token");
        assert_eq!(concept.id, "documents/verify-token");
        assert_eq!(concept.location.file, "auth/verify-token.dita");
    }

    #[test]
    fn a_topic_with_no_id_falls_back_to_a_path_derived_qualified_name() {
        let dir = tempfile::tempdir().unwrap();
        write(
            dir.path(),
            "guides/getting-started.dita",
            "<topic><title>Getting Started</title><body/></topic>",
        );

        let (concepts, _) = import_dita(dir.path()).unwrap();
        assert_eq!(concepts.len(), 1);
        assert_eq!(concepts[0].qualified_name, "guides.getting-started");
    }

    #[test]
    fn a_topic_with_no_title_falls_back_to_its_filename() {
        let dir = tempfile::tempdir().unwrap();
        write(dir.path(), "untitled.dita", "<topic><body/></topic>");

        let (concepts, _) = import_dita(dir.path()).unwrap();
        assert_eq!(concepts.len(), 1);
        assert_eq!(concepts[0].name, "untitled");
    }

    #[test]
    fn malformed_xml_is_skipped_not_fatal_to_the_rest_of_the_import() {
        let dir = tempfile::tempdir().unwrap();
        write(dir.path(), "broken.dita", "<topic><title>Oops</topic>"); // mismatched tag
        write(
            dir.path(),
            "fine.dita",
            "<topic><title>Fine</title><body/></topic>",
        );

        let (concepts, skipped) = import_dita(dir.path()).unwrap();
        assert_eq!(concepts.len(), 1);
        assert_eq!(concepts[0].name, "Fine");
        assert_eq!(skipped.len(), 1);
        assert!(skipped[0].starts_with("broken.dita:"));
    }

    #[test]
    fn non_dita_xml_files_are_ignored_by_extension() {
        let dir = tempfile::tempdir().unwrap();
        write(dir.path(), "notes.txt", "not xml at all");
        write(
            dir.path(),
            "topic.dita",
            "<topic><title>Real Topic</title><body/></topic>",
        );

        let (concepts, skipped) = import_dita(dir.path()).unwrap();
        assert_eq!(concepts.len(), 1);
        assert!(skipped.is_empty());
    }

    #[test]
    fn importing_a_single_file_path_works_the_same_as_a_directory() {
        let dir = tempfile::tempdir().unwrap();
        write(
            dir.path(),
            "solo.dita",
            "<topic id=\"solo\"><title>Solo</title><body/></topic>",
        );

        let (concepts, _) = import_dita(&dir.path().join("solo.dita")).unwrap();
        assert_eq!(concepts.len(), 1);
        assert_eq!(concepts[0].name, "Solo");
        assert_eq!(concepts[0].location.file, "solo.dita");
    }

    #[test]
    fn concatenates_mixed_inline_markup_in_shortdesc() {
        let dir = tempfile::tempdir().unwrap();
        write(
            dir.path(),
            "mixed.dita",
            "<topic><title>Mixed</title><shortdesc>Uses <b>bold</b> text.</shortdesc><body/></topic>",
        );

        let (concepts, _) = import_dita(dir.path()).unwrap();
        assert_eq!(concepts[0].description.as_deref(), Some("Uses bold text."));
    }
}
