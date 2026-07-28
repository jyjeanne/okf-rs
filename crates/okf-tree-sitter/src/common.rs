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
    Relationship {
        kind: RelationKind::Imports,
        target: format!("external/{}", slugify(raw)),
        target_display: raw.to_string(),
    }
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
        timestamp: None,
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
