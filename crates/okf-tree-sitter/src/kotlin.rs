//! Kotlin extraction. One `Module` concept per file, as usual; classes,
//! interfaces, enums, and singleton `object`s are all one `class_declaration`
//! (or `object_declaration`) node kind in this grammar, distinguished only
//! by a literal keyword child (`interface`) or body kind
//! (`enum_class_body` vs `class_body`) — see [`concept_kind_of`]. Top-level
//! functions and methods are likewise both `function_declaration`,
//! distinguished by whether [`container_name`] finds an enclosing type —
//! the same shape Rust's `function_item` has.
//!
//! Kotlin's actual default visibility is public (like PHP, unlike Java/
//! C#): an unmarked declaration is public unless an explicit `private`/
//! `protected`/`internal` modifier says otherwise (see [`is_public`]).
//!
//! `call_expression`'s callee has no named field in this grammar — it's
//! either a bare `identifier` (positional first child) or a
//! `navigation_expression` whose last child is the accessed member, so
//! [`call_target_name`] inspects the node's shape directly rather than
//! using field-based query captures, covering bare calls, `this.foo()`,
//! `obj.foo()`, and `Companion.foo()` uniformly.
//!
//! Not handled (kept out of scope for this pass): primary/secondary
//! constructors, which have no `name` field to key a `Method` concept on
//! in this grammar and would need special-casing distinct from every
//! other extractor's "capture the def, capture the name" shape.

use crate::common::{
    import_relationship, location, make_concept, module_path, node_text, smallest_containing,
};
use crate::{CallCandidate, FileExtraction};
use anyhow::{Context, Result};
use okf_parser::{Concept, ConceptKind, Language};
use std::ops::Range;
use tree_sitter::{Node, Parser, Query, QueryCursor, StreamingIterator};

const QUERY_SRC: &str = r#"
(import) @import
(class_declaration name: (identifier) @type.name) @type.def
(object_declaration name: (identifier) @type.name) @type.def
(function_declaration name: (identifier) @fn.name) @fn.def
(call_expression) @call.def
"#;

/// Classes, interfaces, enums, and objects all reach here; distinguishes
/// them by shape rather than node kind, since Kotlin's grammar reuses
/// `class_declaration` for the first three.
fn concept_kind_of(node: Node) -> ConceptKind {
    if node.kind() == "object_declaration" {
        return ConceptKind::Class;
    }
    let mut cursor = node.walk();
    let is_interface = node
        .children(&mut cursor)
        .any(|child| child.kind() == "interface");
    if is_interface {
        return ConceptKind::Interface;
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

/// The enclosing class/interface/enum/object name of a function
/// declaration, if any — `None` means it's a top-level function.
fn container_name<'a>(src: &'a str, def_node: Node) -> Option<&'a str> {
    let body = def_node.parent()?;
    if !matches!(body.kind(), "class_body" | "enum_class_body") {
        return None;
    }
    let decl = body.parent()?;
    let name = decl.child_by_field_name("name")?;
    Some(node_text(src, name))
}

/// Kotlin's actual default: a declaration with no visibility modifier is
/// public. Only an explicit `private`, `protected`, or `internal` narrows
/// that — the same "public unless marked otherwise" polarity as PHP,
/// opposite Rust/Java/C#'s "explicit `pub`/`public` required".
fn is_public(src: &str, def_node: Node) -> bool {
    let mut cursor = def_node.walk();
    let modifiers = def_node
        .children(&mut cursor)
        .find(|child| child.kind() == "modifiers");
    let Some(modifiers) = modifiers else {
        return true;
    };
    let mut mods_cursor = modifiers.walk();
    let restrictive = modifiers.children(&mut mods_cursor).any(|child| {
        child.kind() == "visibility_modifier"
            && matches!(node_text(src, child), "private" | "protected" | "internal")
    });
    !restrictive
}

/// The identifier a `call_expression` actually invokes: either its bare
/// `identifier` callee, or the last (member) identifier of a
/// `navigation_expression` callee (`recv.member`), covering `this.foo()`,
/// `obj.foo()`, and `Companion.foo()` uniformly since Kotlin uses `.` for
/// all of them.
fn call_target_name<'a>(src: &'a str, call_node: Node<'a>) -> Option<&'a str> {
    let callee = call_node.named_child(0)?;
    match callee.kind() {
        "identifier" => Some(node_text(src, callee)),
        "navigation_expression" => {
            let count = callee.named_child_count();
            let last = callee.named_child(count.checked_sub(1)? as u32)?;
            (last.kind() == "identifier").then(|| node_text(src, last))
        }
        _ => None,
    }
}

fn signature_before_body(src: &str, def_node: Node) -> String {
    let mut cursor = def_node.walk();
    let end = def_node
        .children(&mut cursor)
        .find(|child| child.kind() == "function_body")
        .map(|b| b.start_byte())
        .unwrap_or(def_node.end_byte());
    src[def_node.start_byte()..end]
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

/// `class_declaration`/`object_declaration` have no field usable across
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
    let ts_lang: tree_sitter::Language = tree_sitter_kotlin_ng::LANGUAGE.into();
    let mut parser = Parser::new();
    parser
        .set_language(&ts_lang)
        .context("failed to load Kotlin grammar")?;
    let tree = parser
        .parse(source, None)
        .context("failed to parse Kotlin source")?;

    let module = module_path(relative_path);
    let mut module_concept = make_concept(
        ConceptKind::Module,
        Language::Kotlin,
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

    let query = Query::new(&ts_lang, QUERY_SRC).context("invalid Kotlin query")?;
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
            let path = text
                .trim_start_matches("import")
                .trim()
                .trim_end_matches(';')
                .trim();
            module_concept.relationships.push(import_relationship(path));
        }

        if let (Some(def), Some(name)) = (type_def, type_name) {
            let name_text = node_text(source, name);
            let qualified = format!("{}.{}", module, name_text);
            concepts.push(make_concept(
                concept_kind_of(def),
                Language::Kotlin,
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
                Language::Kotlin,
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
    fn extracts_classes_interfaces_enums_objects_functions_and_calls() {
        let src = r#"
package com.example

import kotlin.collections.List

interface Greeter {
    fun greet(name: String): String
}

enum class Status { Active, Inactive }

class Auth : Greeter {
    fun verifyToken(token: String): Boolean {
        return decodeJwt(token)
    }

    private fun decodeJwt(token: String): Boolean {
        return true
    }

    override fun greet(name: String): String {
        return "hi " + name
    }
}

object Util {
    fun log(s: String) {}
}

fun topLevel() {
    Util.log("hi")
}
"#;
        let extraction = extract(src, "src/Auth.kt").unwrap();
        let names: Vec<_> = extraction
            .concepts
            .iter()
            .map(|c| c.name.as_str())
            .collect();
        assert!(names.contains(&"Greeter"));
        assert!(names.contains(&"Status"));
        assert!(names.contains(&"Auth"));
        assert!(names.contains(&"verifyToken"));
        assert!(names.contains(&"decodeJwt"));
        assert!(names.contains(&"Util"));
        assert!(names.contains(&"topLevel"));

        let module = extraction
            .concepts
            .iter()
            .find(|c| c.kind == ConceptKind::Module)
            .unwrap();
        assert_eq!(module.relationships.len(), 1);
        assert_eq!(
            module.relationships[0].target_display,
            "kotlin.collections.List"
        );

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
                .find(|c| c.name == "Util")
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

        let callee_names: Vec<_> = extraction
            .calls
            .iter()
            .map(|c| c.callee_name.as_str())
            .collect();
        assert!(callee_names.contains(&"decodeJwt"));
        assert!(callee_names.contains(&"log"));
    }

    #[test]
    fn a_declaration_is_public_by_default_unless_marked_otherwise() {
        let src = r#"
class Holder {
    fun publicFn() {}
    private fun privateFn() {}
    protected fun protectedFn() {}
    internal fun internalFn() {}
    public fun explicitPublicFn() {}
}
"#;
        let extraction = extract(src, "src/Holder.kt").unwrap();
        let find = |name: &str| extraction.concepts.iter().find(|c| c.name == name).unwrap();

        assert!(
            find("publicFn").is_public,
            "Kotlin's actual default: no modifier means public"
        );
        assert!(!find("privateFn").is_public);
        assert!(!find("protectedFn").is_public);
        assert!(!find("internalFn").is_public);
        assert!(find("explicitPublicFn").is_public);
    }

    #[test]
    fn captures_this_object_and_bare_calls() {
        let src = r#"
class Util {
    fun log(s: String) {}
}

class Service {
    fun run() {
        helper()
        this.helper()
        Util().log("hi")
    }
    fun helper() {}
}
"#;
        let extraction = extract(src, "src/Service.kt").unwrap();
        let callee_names: Vec<_> = extraction
            .calls
            .iter()
            .map(|c| c.callee_name.as_str())
            .collect();
        assert!(callee_names.contains(&"helper"));
        assert!(
            callee_names.iter().filter(|&&n| n == "helper").count() >= 2,
            "expected both helper() and this.helper() to be captured, got {callee_names:?}"
        );
        assert!(callee_names.contains(&"log"), "expected Util().log(...)");
    }
}
