use okf_parser::{Concept, ConceptKind, Language, Location, RelationKind, Relationship};
use std::ops::Range;
use tree_sitter::Node;

pub fn node_text<'a>(src: &'a str, node: Node) -> &'a str {
    &src[node.byte_range()]
}

pub fn location(relative_path: &str, node: Node) -> Location {
    Location {
        file: relative_path.to_string(),
        start_line: node.start_position().row + 1,
        end_line: node.end_position().row + 1,
    }
}

/// Converts `node`'s start position into the Language Server Protocol's
/// `Position` convention: 0-based line, 0-based *UTF-16 code unit* column.
/// Not just a field rename from tree-sitter's own `Point` -- tree-sitter's
/// `column` is a byte offset, so a line with any non-ASCII text before
/// `node` needs its prefix actually re-encoded to know where a real LSP
/// server would consider the node to start.
pub fn lsp_position(src: &str, node: Node) -> crate::CallSite {
    let start_byte = node.start_byte();
    let line_start = src[..start_byte].rfind('\n').map(|i| i + 1).unwrap_or(0);
    let character = src[line_start..start_byte].encode_utf16().count() as u32;
    crate::CallSite {
        line: node.start_position().row as u32,
        character,
    }
}

/// Derives a dotted module path from a file's relative path: drops the
/// extension and collapses directory-index-like filenames (`mod`, `lib`,
/// `main`, `__init__`, `index`) into their parent module, so
/// `src/auth/mod.rs` and `src/auth.rs` both become `src.auth`.
pub fn module_path(relative_path: &str) -> String {
    let no_ext = relative_path
        .rsplit_once('.')
        .map(|(base, _)| base)
        .unwrap_or(relative_path);
    let mut segments: Vec<&str> = no_ext.split('/').collect();
    if let Some(last) = segments.last() {
        let is_index = matches!(
            last.to_ascii_lowercase().as_str(),
            "mod" | "lib" | "main" | "__init__" | "index"
        );
        if is_index && segments.len() > 1 {
            segments.pop();
        }
    }
    segments.join(".")
}

pub fn slugify(text: &str) -> String {
    let mut out = String::new();
    let mut last_dash = false;
    for c in text.chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c.to_ascii_lowercase());
            last_dash = false;
        } else if !last_dash && !out.is_empty() {
            out.push('-');
            last_dash = true;
        }
    }
    out.trim_end_matches('-').to_string()
}

pub fn strip_quotes(s: &str) -> String {
    s.trim_matches(|c| c == '"' || c == '\'' || c == '`')
        .to_string()
}

/// Builds an unresolved `Imports` relationship pointing at a synthetic
/// external id. `okf-generator` renders these as plain text rather than
/// links, since the target is not (yet) a concept in the bundle.
pub fn import_relationship(raw: &str) -> Relationship {
    Relationship::new(
        RelationKind::Imports,
        format!("external/{}", slugify(raw)),
        raw,
    )
}

#[allow(clippy::too_many_arguments)]
pub fn make_concept(
    kind: ConceptKind,
    language: Language,
    name: &str,
    qualified_name: &str,
    location: Location,
    signature: Option<String>,
    is_public: bool,
) -> Concept {
    Concept {
        id: Concept::make_id(kind, qualified_name),
        kind,
        language,
        name: name.to_string(),
        qualified_name: qualified_name.to_string(),
        description: None,
        location,
        signature,
        tags: Vec::new(),
        is_public,
        generated_at: None,
        relationships: Vec::new(),
    }
}

/// Python/JavaScript/TypeScript's "leading underscore means private"
/// naming convention. Unlike Rust's `pub` keyword or Go's capitalization
/// rule, this is not enforced by the language — it's a heuristic until
/// real `export`/access-modifier tracking lands.
pub fn is_public_by_underscore_convention(name: &str) -> bool {
    !name.starts_with('_')
}

/// Go's actual visibility rule: an identifier is exported (public) if and
/// only if its first character is uppercase.
pub fn is_public_by_go_convention(name: &str) -> bool {
    name.chars().next().is_some_and(|c| c.is_uppercase())
}

/// Finds, among `candidates`, the id of the smallest byte range containing
/// `byte` — used to attribute a call expression to its nearest enclosing
/// function or method.
pub fn smallest_containing(candidates: &[(String, Range<usize>)], byte: usize) -> Option<&str> {
    candidates
        .iter()
        .filter(|(_, range)| range.contains(&byte))
        .min_by_key(|(_, range)| range.end - range.start)
        .map(|(id, _)| id.as_str())
}

/// A definition node's source text up to (not including) its body,
/// whitespace-collapsed to a single line — the common shape of a
/// function/method signature across languages whose grammar exposes a
/// `body` field on the definition node. Some grammars need their own
/// variant instead of this one: Python's signature also has a trailing
/// `:` to trim, and Kotlin's function node has no `body` field, only a
/// `function_body` child by kind.
pub fn signature_before_body(src: &str, def_node: Node) -> String {
    let end = def_node
        .child_by_field_name("body")
        .map(|b| b.start_byte())
        .unwrap_or(def_node.end_byte());
    src[def_node.start_byte()..end]
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

/// A type-declaration node's source text up to (not including) its body,
/// whitespace-collapsed to a single line. Rust needs its own variant
/// instead of this one: a trait/extern item can end in `;` with no body
/// at all, which this version doesn't stop at.
pub fn type_signature(src: &str, def_node: Node) -> String {
    let text = node_text(src, def_node);
    text.split('{')
        .next()
        .unwrap_or(text)
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

/// Builds the file-level `Module` concept every language extractor emits
/// exactly one of: its qualified name and displayed name both come from
/// [`module_path`], its location spans the whole file, and it starts out
/// with no signature and no relationships (a caller that resolves imports
/// pushes those onto the returned concept's `relationships` itself).
pub fn module_concept(language: Language, relative_path: &str, source: &str) -> Concept {
    let module = module_path(relative_path);
    make_concept(
        ConceptKind::Module,
        language,
        module.rsplit('.').next().unwrap_or(&module),
        &module,
        Location {
            file: relative_path.to_string(),
            start_line: 1,
            end_line: source.lines().count().max(1),
        },
        None,
        true,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use tree_sitter::Parser;

    /// Parses `src` as Rust and returns the byte range of the first
    /// occurrence of `needle`, used to locate a specific call/identifier
    /// to check `lsp_position` against without hand-computing offsets.
    fn find_byte_range(src: &str, needle: &str) -> Range<usize> {
        let start = src.find(needle).expect("needle not found in source");
        start..start + needle.len()
    }

    fn node_at<'a>(tree: &'a tree_sitter::Tree, byte: usize) -> Node<'a> {
        tree.root_node()
            .descendant_for_byte_range(byte, byte)
            .expect("no node at byte")
    }

    #[test]
    fn lsp_position_matches_line_and_ascii_column() {
        let src = "fn a() {\n    b();\n}\n";
        let mut parser = Parser::new();
        parser
            .set_language(&tree_sitter_rust::LANGUAGE.into())
            .unwrap();
        let tree = parser.parse(src, None).unwrap();

        let range = find_byte_range(src, "b()");
        let node = node_at(&tree, range.start);
        let pos = lsp_position(src, node);

        assert_eq!(pos.line, 1, "call is on the second (0-based) line");
        assert_eq!(pos.character, 4, "call starts after 4 leading spaces");
    }

    #[test]
    fn lsp_position_counts_utf16_not_bytes_before_non_ascii_text_on_the_same_line() {
        // "é" is 2 bytes in UTF-8 but 1 UTF-16 code unit -- a byte-offset
        // column (tree-sitter's own convention) would overcount by 1 for
        // anything after it on the same line; LSP needs the UTF-16 count.
        let src = "fn a() {\n    let é = 1; b();\n}\n";
        let mut parser = Parser::new();
        parser
            .set_language(&tree_sitter_rust::LANGUAGE.into())
            .unwrap();
        let tree = parser.parse(src, None).unwrap();

        let range = find_byte_range(src, "b()");
        let node = node_at(&tree, range.start);
        let pos = lsp_position(src, node);

        assert_eq!(pos.line, 1);
        assert_eq!(
            pos.character, 15,
            "UTF-16 code units before `b()`, not UTF-8 bytes (which would be 16)"
        );
    }
}
