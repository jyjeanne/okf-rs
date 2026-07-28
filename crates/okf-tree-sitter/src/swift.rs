//! Swift extraction. `class`, `struct`, and `enum` are all one
//! `class_declaration` node kind in this grammar, distinguished only by a
//! literal keyword child (`struct`) or body kind (`enum_class_body` vs
//! `class_body`) — see [`concept_kind_of`], the same shape Kotlin's
//! grammar has. `protocol` is its own node kind, mapped to `Interface`.
//! Top-level functions and methods are both `function_declaration`,
//! distinguished by whether an enclosing type body is found — the same
//! shape Rust's `function_item` and Kotlin's `function_declaration` have.
//!
//! Swift's actual default access level is `internal` (visible within the
//! module, not merely the file) — genuinely not public, unlike Kotlin/PHP
//! where the unmarked default really is public. So visibility here uses
//! the opposite (Rust/Java/C#-style) polarity: `true` only when an
//! explicit `public` or `open` modifier is present (see [`is_public`]).
//! Protocol method *requirements* (`protocol_function_declaration`, no
//! body) aren't extracted, the same call as C++'s bodyless prototypes —
//! only a `function_declaration` with an actual body becomes a concept.
//!
//! `call_expression`'s callee has no named field in this grammar, mirroring
//! Kotlin: it's either a bare `simple_identifier` or a
//! `navigation_expression` whose `suffix` field (a `navigation_suffix`)
//! has its own `suffix` field holding the accessed member — two levels of
//! `suffix` nesting — covering bare calls, `self.foo()`, `obj.foo()`, and
//! `Type.foo()` uniformly since Swift uses `.` for all of them.

use crate::common::{
    import_relationship, location, make_concept, module_path, node_text, smallest_containing,
};
use crate::{CallCandidate, FileExtraction};
use anyhow::{Context, Result};
use okf_parser::{Concept, ConceptKind, Language};
use std::ops::Range;
use tree_sitter::{Node, Parser, Query, QueryCursor, StreamingIterator};

const QUERY_SRC: &str = r#"
(import_declaration) @import
(class_declaration name: (type_identifier) @type.name) @type.def
(protocol_declaration name: (type_identifier) @type.name) @type.def
(function_declaration name: (simple_identifier) @fn.name) @fn.def
(call_expression) @call.def
"#;

/// Classes, structs, and enums all reach here as `class_declaration`
/// (protocols are handled separately, via their own node kind).
fn concept_kind_of(node: Node) -> ConceptKind {
    if node.kind() == "protocol_declaration" {
        return ConceptKind::Interface;
    }
    let mut cursor = node.walk();
    let is_struct = node
        .children(&mut cursor)
        .any(|child| child.kind() == "struct");
    if is_struct {
        return ConceptKind::Struct;
    }
    let mut cursor = node.walk();
    let is_enum = node
        .children(&mut cursor)
        .any(|child| child.kind() == "enum_class_body");
    if is_enum {
        return ConceptKind::Enum;
    }
    ConceptKind::Class
}

/// The enclosing type's name of a function declaration, if any — `None`
/// means it's a top-level function.
fn container_name<'a>(src: &'a str, def_node: Node) -> Option<&'a str> {
    let body = def_node.parent()?;
    if !matches!(body.kind(), "class_body" | "enum_class_body") {
        return None;
    }
    let decl = body.parent()?;
    let name = decl.child_by_field_name("name")?;
    Some(node_text(src, name))
}

/// Swift's actual default access level (`internal`) is genuinely not
/// public, unlike Kotlin/PHP — so an explicit `public` or `open` modifier
/// is required, the same "opt-in" polarity as Rust's `pub`.
fn is_public(src: &str, def_node: Node) -> bool {
    let mut cursor = def_node.walk();
    let modifiers = def_node
        .children(&mut cursor)
        .find(|child| child.kind() == "modifiers");
    let Some(modifiers) = modifiers else {
        return false;
    };
    let mut mods_cursor = modifiers.walk();
    let has_public_or_open = modifiers.children(&mut mods_cursor).any(|child| {
        child.kind() == "visibility_modifier" && matches!(node_text(src, child), "public" | "open")
    });
    has_public_or_open
}

/// The identifier a `call_expression` actually invokes: either its bare
/// `simple_identifier` callee, or the member identifier nested two
/// `suffix` fields deep inside a `navigation_expression` callee
/// (`recv.suffix` is a `navigation_suffix`, whose own `suffix` field is
/// the accessed identifier) — covering `self.foo()`, `obj.foo()`, and
/// `Type.foo()` uniformly.
fn call_target_name<'a>(src: &'a str, call_node: Node<'a>) -> Option<&'a str> {
    let callee = call_node.named_child(0)?;
    match callee.kind() {
        "simple_identifier" => Some(node_text(src, callee)),
        "navigation_expression" => {
            let suffix = callee.child_by_field_name("suffix")?;
            let member = suffix.child_by_field_name("suffix")?;
            (member.kind() == "simple_identifier").then(|| node_text(src, member))
        }
        _ => None,
    }
}

fn signature_before_body(src: &str, def_node: Node) -> String {
    let end = def_node
        .child_by_field_name("body")
        .map(|b| b.start_byte())
        .unwrap_or(def_node.end_byte());
    src[def_node.start_byte()..end]
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

/// `class_declaration`/`protocol_declaration` have no field usable across
/// both — truncating at the first `{` gives just the declaration line.
fn type_signature(src: &str, def_node: Node) -> String {
    let text = node_text(src, def_node);
    text.split('{')
        .next()
        .unwrap_or(text)
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

pub fn extract(source: &str, relative_path: &str) -> Result<FileExtraction> {
    let ts_lang: tree_sitter::Language = tree_sitter_swift::LANGUAGE.into();
    let mut parser = Parser::new();
    parser
        .set_language(&ts_lang)
        .context("failed to load Swift grammar")?;
    let tree = parser
        .parse(source, None)
        .context("failed to parse Swift source")?;

    let module = module_path(relative_path);
    let mut module_concept = make_concept(
        ConceptKind::Module,
        Language::Swift,
        module.rsplit('.').next().unwrap_or(&module),
        &module,
        okf_parser::Location {
            file: relative_path.to_string(),
            start_line: 1,
            end_line: source.lines().count().max(1),
        },
        None,
        true,
    );

    let query = Query::new(&ts_lang, QUERY_SRC).context("invalid Swift query")?;
    let mut cursor = QueryCursor::new();
    let mut matches = cursor.matches(&query, tree.root_node(), source.as_bytes());

    let mut concepts: Vec<Concept> = Vec::new();
    let mut function_spans: Vec<(String, Range<usize>)> = Vec::new();
    let mut raw_calls: Vec<(Range<usize>, Node)> = Vec::new();

    while let Some(m) = matches.next() {
        let mut import_node = None;
        let mut type_def = None;
        let mut type_name = None;
        let mut fn_def = None;
        let mut fn_name = None;
        let mut call_def = None;

        for cap in m.captures {
            match query.capture_names()[cap.index as usize] {
                "import" => import_node = Some(cap.node),
                "type.def" => type_def = Some(cap.node),
                "type.name" => type_name = Some(cap.node),
                "fn.def" => fn_def = Some(cap.node),
                "fn.name" => fn_name = Some(cap.node),
                "call.def" => call_def = Some(cap.node),
                _ => {}
            }
        }

        if let Some(node) = import_node {
            let text = node_text(source, node);
            let path = text.trim_start_matches("import").trim();
            module_concept.relationships.push(import_relationship(path));
        }

        if let (Some(def), Some(name)) = (type_def, type_name) {
            let name_text = node_text(source, name);
            let qualified = format!("{}.{}", module, name_text);
            concepts.push(make_concept(
                concept_kind_of(def),
                Language::Swift,
                name_text,
                &qualified,
                location(relative_path, def),
                Some(type_signature(source, def)),
                is_public(source, def),
            ));
        }

        if let (Some(def), Some(name)) = (fn_def, fn_name) {
            let fn_name_text = node_text(source, name);
            let (kind, qualified) = match container_name(source, def) {
                Some(container) => (
                    ConceptKind::Method,
                    format!("{}.{}.{}", module, container, fn_name_text),
                ),
                None => (
                    ConceptKind::Function,
                    format!("{}.{}", module, fn_name_text),
                ),
            };
            let concept = make_concept(
                kind,
                Language::Swift,
                fn_name_text,
                &qualified,
                location(relative_path, def),
                Some(signature_before_body(source, def)),
                is_public(source, def),
            );
            function_spans.push((concept.id.clone(), def.byte_range()));
            concepts.push(concept);
        }

        if let Some(node) = call_def {
            raw_calls.push((node.byte_range(), node));
        }
    }

    let mut calls = Vec::new();
    for (range, call_node) in raw_calls {
        let Some(callee_name) = call_target_name(source, call_node) else {
            continue;
        };
        if let Some(caller_id) = smallest_containing(&function_spans, range.start) {
            calls.push(CallCandidate {
                caller_id: caller_id.to_string(),
                callee_name: callee_name.to_string(),
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
    fn extracts_classes_structs_enums_protocols_functions_and_calls() {
        let src = r#"
import Foundation

protocol Greeter {
    func greet(name: String) -> String
}

enum Status {
    case active
    case inactive
}

struct Point {
    var x: Int
    var y: Int
}

class Auth: Greeter {
    func verifyToken(token: String) -> Bool {
        return decodeJwt(token: token)
    }

    private func decodeJwt(token: String) -> Bool {
        return true
    }

    func greet(name: String) -> String {
        return "hi " + name
    }
}

func topLevel() {
    Util.log(s: "hi")
}
"#;
        let extraction = extract(src, "src/Auth.swift").unwrap();
        let names: Vec<_> = extraction
            .concepts
            .iter()
            .map(|c| c.name.as_str())
            .collect();
        assert!(names.contains(&"Greeter"));
        assert!(names.contains(&"Status"));
        assert!(names.contains(&"Point"));
        assert!(names.contains(&"Auth"));
        assert!(names.contains(&"verifyToken"));
        assert!(names.contains(&"decodeJwt"));
        assert!(names.contains(&"topLevel"));

        let module = extraction
            .concepts
            .iter()
            .find(|c| c.kind == ConceptKind::Module)
            .unwrap();
        assert_eq!(module.relationships.len(), 1);
        assert_eq!(module.relationships[0].target_display, "Foundation");

        assert_eq!(
            extraction
                .concepts
                .iter()
                .find(|c| c.name == "Greeter")
                .unwrap()
                .kind,
            ConceptKind::Interface
        );
        assert_eq!(
            extraction
                .concepts
                .iter()
                .find(|c| c.name == "Status")
                .unwrap()
                .kind,
            ConceptKind::Enum
        );
        assert_eq!(
            extraction
                .concepts
                .iter()
                .find(|c| c.name == "Point")
                .unwrap()
                .kind,
            ConceptKind::Struct
        );
        assert_eq!(
            extraction
                .concepts
                .iter()
                .find(|c| c.name == "Auth" && c.kind != ConceptKind::Module)
                .unwrap()
                .kind,
            ConceptKind::Class
        );

        let top_level = extraction
            .concepts
            .iter()
            .find(|c| c.name == "topLevel")
            .unwrap();
        assert_eq!(top_level.kind, ConceptKind::Function);

        let verify = extraction
            .concepts
            .iter()
            .find(|c| c.name == "verifyToken")
            .unwrap();
        assert_eq!(verify.kind, ConceptKind::Method);

        assert_eq!(extraction.calls.len(), 2);
        let callee_names: Vec<_> = extraction
            .calls
            .iter()
            .map(|c| c.callee_name.as_str())
            .collect();
        assert!(callee_names.contains(&"decodeJwt"));
        assert!(callee_names.contains(&"log"));
    }

    #[test]
    fn only_explicit_public_or_open_counts_as_public() {
        let src = r#"
public class PublicClass {}
class InternalClass {}

public class Holder {
    public func publicFn() {}
    open func openFn() {}
    internal func internalFn() {}
    private func privateFn() {}
    fileprivate func fileprivateFn() {}
    func unmarkedFn() {}
}
"#;
        let extraction = extract(src, "src/Holder.swift").unwrap();
        let find = |name: &str| extraction.concepts.iter().find(|c| c.name == name).unwrap();

        assert!(find("PublicClass").is_public);
        assert!(
            !find("InternalClass").is_public,
            "Swift's actual default is internal, not public"
        );
        assert!(find("publicFn").is_public);
        assert!(find("openFn").is_public);
        assert!(!find("internalFn").is_public);
        assert!(!find("privateFn").is_public);
        assert!(!find("fileprivateFn").is_public);
        assert!(!find("unmarkedFn").is_public);
    }

    #[test]
    fn captures_self_type_and_bare_calls() {
        let src = r#"
class Util {
    static func log(s: String) {}
}

class Service {
    func run() {
        helper()
        self.helper()
        Util.log(s: "hi")
    }
    func helper() {}
}
"#;
        let extraction = extract(src, "src/Service.swift").unwrap();
        let callee_names: Vec<_> = extraction
            .calls
            .iter()
            .map(|c| c.callee_name.as_str())
            .collect();
        assert!(callee_names.contains(&"helper"));
        assert!(
            callee_names.iter().filter(|&&n| n == "helper").count() >= 2,
            "expected both helper() and self.helper() to be captured, got {callee_names:?}"
        );
        assert!(callee_names.contains(&"log"), "expected Util.log(...)");
    }
}
