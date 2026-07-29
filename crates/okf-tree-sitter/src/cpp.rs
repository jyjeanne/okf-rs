//! C/C++ extraction. Unlike every other language here, C++ visibility is
//! section-based, not per-member: a `public:`/`private:`/`protected:`
//! label inside a class body applies to every member that follows until
//! the next label (or the body's end) — see [`is_public_inline_member`].
//! `class` defaults its members to private, `struct` to public, matching
//! the language's own default.
//!
//! Both free functions and methods are `function_definition` nodes; a
//! method defined inline in a class body has a bare name (`identifier`/
//! `field_identifier`), while the common C++ idiom of declaring a method
//! in the class body and defining its body separately (often in a
//! different file entirely) produces a `qualified_identifier` name
//! (`ClassName::method`) with no class body in sight — see
//! [`function_name_and_scope`]. Since this extractor has no way to see
//! that other file's access-specifier section, an out-of-class definition
//! defaults to private; see the crate-level docs' known-limitations note.
//!
//! Only `function_definition` (a body present) becomes a concept — a
//! bodyless prototype (`field_declaration` with a function declarator,
//! the usual header-file shape) is not, so a header/source split where
//! only the source file has bodies still gets exactly one `Method`
//! concept per method, not zero or two.
//!
//! Covers both C and C++ (`Language::Cpp`, matching the roadmap's single
//! "C/C++" item): tree-sitter's C++ grammar parses plain C's structs and
//! functions just fine, and C simply never uses the C++-only shapes here
//! (`class_specifier`, out-of-class `Class::method` definitions).

use crate::common::{
    import_relationship, location, lsp_position, make_concept, module_concept, module_path,
    node_text, signature_before_body, smallest_containing, type_signature,
};
use crate::{CallCandidate, CallSite, FileExtraction};
use anyhow::{Context, Result};
use okf_parser::{Concept, ConceptKind, Language};
use std::ops::Range;
use tree_sitter::{Node, Parser, Query, QueryCursor, StreamingIterator};

const QUERY_SRC: &str = r#"
(preproc_include path: (_) @import.path) @import.def
(class_specifier name: (type_identifier) @class.name body: (field_declaration_list)) @class.def
(struct_specifier name: (type_identifier) @struct.name body: (field_declaration_list)) @struct.def
(function_definition) @fn.def
(call_expression function: (identifier) @call.name) @call.def
(call_expression function: (field_expression field: (field_identifier) @call.name)) @call.def
(call_expression function: (qualified_identifier name: (identifier) @call.name)) @call.def
"#;

/// Descends through wrapping declarators (pointer/reference return types)
/// to find the `function_declarator` a `function_definition`'s own
/// `declarator` field ultimately wraps.
fn innermost_function_declarator(node: Node) -> Option<Node> {
    if node.kind() == "function_declarator" {
        return Some(node);
    }
    innermost_function_declarator(node.child_by_field_name("declarator")?)
}

/// The function's own name, plus its class scope if the name is written
/// `ClassName::method` (the out-of-class definition idiom) rather than a
/// bare identifier.
fn function_name_and_scope<'a>(
    src: &'a str,
    def_node: Node<'a>,
) -> Option<(&'a str, Option<&'a str>)> {
    let declarator = def_node.child_by_field_name("declarator")?;
    let func_declarator = innermost_function_declarator(declarator)?;
    let name_node = func_declarator.child_by_field_name("declarator")?;
    match name_node.kind() {
        "identifier" | "field_identifier" => Some((node_text(src, name_node), None)),
        "qualified_identifier" => {
            let name = name_node.child_by_field_name("name")?;
            if !matches!(name.kind(), "identifier" | "field_identifier") {
                return None;
            }
            let scope = name_node.child_by_field_name("scope")?;
            Some((node_text(src, name), Some(node_text(src, scope))))
        }
        _ => None,
    }
}

/// The enclosing class/struct name of a `function_definition` defined
/// inline in its body, if any.
fn container_name<'a>(src: &'a str, def_node: Node) -> Option<&'a str> {
    let body = def_node.parent()?;
    if body.kind() != "field_declaration_list" {
        return None;
    }
    let decl = body.parent()?;
    if !matches!(decl.kind(), "class_specifier" | "struct_specifier") {
        return None;
    }
    let name = decl.child_by_field_name("name")?;
    Some(node_text(src, name))
}

/// The visibility in effect for an inline member at `def_node`'s position:
/// the nearest preceding `access_specifier` sibling's keyword, or the
/// enclosing type's own default (`private` for `class`, `public` for
/// `struct`) if no section label precedes it.
fn is_public_inline_member(src: &str, def_node: Node) -> bool {
    let Some(body) = def_node.parent() else {
        return false;
    };
    let default_public = body
        .parent()
        .is_some_and(|decl| decl.kind() == "struct_specifier");
    let mut cursor = body.walk();
    let mut visibility: Option<&str> = None;
    for child in body.children(&mut cursor) {
        if child.id() == def_node.id() {
            break;
        }
        if child.kind() == "access_specifier" {
            visibility = Some(node_text(src, child).trim_end_matches(':').trim());
        }
    }
    visibility.map(|v| v == "public").unwrap_or(default_public)
}

pub fn extract(source: &str, relative_path: &str) -> Result<FileExtraction> {
    let ts_lang: tree_sitter::Language = tree_sitter_cpp::LANGUAGE.into();
    let mut parser = Parser::new();
    parser
        .set_language(&ts_lang)
        .context("failed to load C++ grammar")?;
    let tree = parser
        .parse(source, None)
        .context("failed to parse C++ source")?;

    let module = module_path(relative_path);
    let mut module_concept = module_concept(Language::Cpp, relative_path, source);

    let query = Query::new(&ts_lang, QUERY_SRC).context("invalid C++ query")?;
    let mut cursor = QueryCursor::new();
    let mut matches = cursor.matches(&query, tree.root_node(), source.as_bytes());

    let mut concepts: Vec<Concept> = Vec::new();
    let mut function_spans: Vec<(String, Range<usize>)> = Vec::new();
    let mut raw_calls: Vec<(Range<usize>, String, CallSite)> = Vec::new();

    while let Some(m) = matches.next() {
        let mut import_node = None;
        let mut class_def = None;
        let mut class_name = None;
        let mut struct_def = None;
        let mut struct_name = None;
        let mut fn_def = None;
        let mut call_name = None;

        for cap in m.captures {
            match query.capture_names()[cap.index as usize] {
                "import.path" => import_node = Some(cap.node),
                "class.def" => class_def = Some(cap.node),
                "class.name" => class_name = Some(cap.node),
                "struct.def" => struct_def = Some(cap.node),
                "struct.name" => struct_name = Some(cap.node),
                "fn.def" => fn_def = Some(cap.node),
                "call.name" => call_name = Some(cap.node),
                _ => {}
            }
        }

        if let Some(node) = import_node {
            module_concept
                .relationships
                .push(import_relationship(node_text(source, node)));
        }

        if let (Some(def), Some(name)) = (class_def, class_name) {
            let name_text = node_text(source, name);
            let qualified = format!("{}.{}", module, name_text);
            concepts.push(make_concept(
                ConceptKind::Class,
                Language::Cpp,
                name_text,
                &qualified,
                location(relative_path, def),
                Some(type_signature(source, def)),
                false,
            ));
        }

        if let (Some(def), Some(name)) = (struct_def, struct_name) {
            let name_text = node_text(source, name);
            let qualified = format!("{}.{}", module, name_text);
            concepts.push(make_concept(
                ConceptKind::Struct,
                Language::Cpp,
                name_text,
                &qualified,
                location(relative_path, def),
                Some(type_signature(source, def)),
                true,
            ));
        }

        if let Some(def) = fn_def {
            if let Some((name_text, scope)) = function_name_and_scope(source, def) {
                let (kind, qualified, is_public) = if let Some(container) = scope {
                    (
                        ConceptKind::Method,
                        format!("{}.{}.{}", module, container, name_text),
                        false,
                    )
                } else if let Some(container) = container_name(source, def) {
                    (
                        ConceptKind::Method,
                        format!("{}.{}.{}", module, container, name_text),
                        is_public_inline_member(source, def),
                    )
                } else {
                    (
                        ConceptKind::Function,
                        format!("{}.{}", module, name_text),
                        true,
                    )
                };
                let concept = make_concept(
                    kind,
                    Language::Cpp,
                    name_text,
                    &qualified,
                    location(relative_path, def),
                    Some(signature_before_body(source, def)),
                    is_public,
                );
                function_spans.push((concept.id.clone(), def.byte_range()));
                concepts.push(concept);
            }
        }

        if let Some(node) = call_name {
            raw_calls.push((
                node.byte_range(),
                node_text(source, node).to_string(),
                lsp_position(source, node),
            ));
        }
    }

    let mut calls = Vec::new();
    for (range, callee_name, call_site) in raw_calls {
        if let Some(caller_id) = smallest_containing(&function_spans, range.start) {
            calls.push(CallCandidate {
                caller_id: caller_id.to_string(),
                callee_name,
                call_site,
            });
        }
    }

    let mut all = vec![module_concept];
    all.extend(concepts);
    Ok(FileExtraction {
        concepts: all,
        calls,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_classes_structs_top_level_functions_and_calls() {
        let src = r#"
#include <string>

class Auth {
public:
    bool verifyToken(const std::string& token) {
        return decodeJwt(token);
    }

private:
    bool decodeJwt(const std::string& token) {
        return true;
    }
};

struct Point {
    int x;
    int y;
};

int topLevel() {
    return 0;
}
"#;
        let extraction = extract(src, "src/auth.cpp").unwrap();
        let names: Vec<_> = extraction
            .concepts
            .iter()
            .map(|c| c.name.as_str())
            .collect();
        assert!(names.contains(&"Auth"));
        assert!(names.contains(&"Point"));
        assert!(names.contains(&"verifyToken"));
        assert!(names.contains(&"decodeJwt"));
        assert!(names.contains(&"topLevel"));

        let module = extraction
            .concepts
            .iter()
            .find(|c| c.kind == ConceptKind::Module)
            .unwrap();
        assert_eq!(module.relationships.len(), 1);
        assert_eq!(module.relationships[0].target_display, "<string>");

        let point = extraction
            .concepts
            .iter()
            .find(|c| c.name == "Point")
            .unwrap();
        assert_eq!(point.kind, ConceptKind::Struct);

        let top_level = extraction
            .concepts
            .iter()
            .find(|c| c.name == "topLevel")
            .unwrap();
        assert_eq!(top_level.kind, ConceptKind::Function);

        assert_eq!(extraction.calls.len(), 1);
        assert_eq!(extraction.calls[0].callee_name, "decodeJwt");
    }

    #[test]
    fn section_based_visibility_respects_access_specifiers_and_container_defaults() {
        let src = r#"
class Holder {
public:
    void publicMethod() {}
private:
    void privateMethod() {}
protected:
    void protectedMethod() {}
public:
    void secondPublicMethod() {}
};

class NoSections {
    void implicitlyPrivate() {}
};

struct PlainStruct {
    void implicitlyPublic() {}
};
"#;
        let extraction = extract(src, "src/holder.cpp").unwrap();
        let find = |name: &str| extraction.concepts.iter().find(|c| c.name == name).unwrap();

        assert!(find("publicMethod").is_public);
        assert!(!find("privateMethod").is_public);
        assert!(!find("protectedMethod").is_public);
        assert!(
            find("secondPublicMethod").is_public,
            "a later public: section should re-open public visibility"
        );
        assert!(
            !find("implicitlyPrivate").is_public,
            "class defaults members to private"
        );
        assert!(
            find("implicitlyPublic").is_public,
            "struct defaults members to public"
        );
    }

    #[test]
    fn captures_this_static_and_bare_calls() {
        let src = r#"
void helper();

void Service::run() {
    this->helper();
    Util::log("hi");
    helper();
}
"#;
        let extraction = extract(src, "src/service.cpp").unwrap();
        let callee_names: Vec<_> = extraction
            .calls
            .iter()
            .map(|c| c.callee_name.as_str())
            .collect();
        assert!(callee_names.contains(&"helper"));
        assert!(
            callee_names.iter().filter(|&&n| n == "helper").count() >= 2,
            "expected both bare helper() and this->helper() to be captured, got {callee_names:?}"
        );
        assert!(callee_names.contains(&"log"), "expected Util::log(...)");
    }

    #[test]
    fn out_of_class_definition_is_scoped_to_its_class_but_defaults_private() {
        let src = r#"
void Service::run() {}
"#;
        let extraction = extract(src, "src/service.cpp").unwrap();
        let run = extraction
            .concepts
            .iter()
            .find(|c| c.name == "run" && c.kind == ConceptKind::Method)
            .expect("out-of-class definition should be extracted as a Method");
        assert!(run.id.contains("Service/run"), "got id {}", run.id);
        assert!(
            !run.is_public,
            "visibility can't be known without the class body, so it defaults to private"
        );
    }
}
