//! C# extraction. Shaped like Java: one `Module` concept per file, classes/
//! structs/interfaces/enums, and methods/constructors unified as `Method`,
//! scoped to their enclosing type via [`container_name`].
//!
//! Unlike Java, the grammar doesn't wrap access modifiers in a `modifiers`
//! node — `public`/`private`/`protected`/`internal`/`static`/... each
//! appear as a `(modifier)` node directly among a declaration's children —
//! so visibility is checked by walking the definition node's own children
//! rather than via a query capture (see [`has_public_modifier`]).
//!
//! `invocation_expression`'s `function` field is either a bare `identifier`
//! or a `member_access_expression` whose own `name` field is the method
//! identifier regardless of receiver, so two query patterns cover bare
//! calls, `this.Foo()`, `obj.Foo()`, and `ClassName.StaticFoo()`.

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
(using_directive) @import
(class_declaration name: (identifier) @class.name) @class.def
(struct_declaration name: (identifier) @struct.name) @struct.def
(interface_declaration name: (identifier) @interface.name) @interface.def
(enum_declaration name: (identifier) @enum.name) @enum.def
(method_declaration name: (identifier) @method.name) @method.def
(constructor_declaration name: (identifier) @method.name) @method.def
(invocation_expression function: (identifier) @call.name) @call.def
(invocation_expression function: (member_access_expression name: (identifier) @call.name)) @call.def
"#;

/// The enclosing class/struct/interface name of a method or constructor
/// declaration, if any. C# enums can't have methods, so unlike Java there
/// is no extra body-nesting case to handle.
fn container_name<'a>(src: &'a str, def_node: Node) -> Option<&'a str> {
    let body = def_node.parent()?;
    if body.kind() != "declaration_list" {
        return None;
    }
    let decl = body.parent()?;
    let name = decl.child_by_field_name("name")?;
    Some(node_text(src, name))
}

/// `public` is the only modifier that makes a member part of the API
/// surface — package-... er, assembly-private (no modifier), `private`,
/// `protected`, and `internal` are all treated as private, matching the
/// "explicit modifier required" precedent set by Rust's `pub` and Java's
/// `public`. Modifiers aren't wrapped in their own node in this grammar,
/// so this walks `def_node`'s direct children rather than a captured
/// modifiers node.
fn has_public_modifier(src: &str, def_node: Node) -> bool {
    let mut cursor = def_node.walk();
    let is_public = def_node
        .children(&mut cursor)
        .any(|child| child.kind() == "modifier" && node_text(src, child) == "public");
    is_public
}

pub fn extract(source: &str, relative_path: &str) -> Result<FileExtraction> {
    let ts_lang: tree_sitter::Language = tree_sitter_c_sharp::LANGUAGE.into();
    let mut parser = Parser::new();
    parser
        .set_language(&ts_lang)
        .context("failed to load C# grammar")?;
    let tree = parser
        .parse(source, None)
        .context("failed to parse C# source")?;

    let module = module_path(relative_path);
    let mut module_concept = module_concept(Language::CSharp, relative_path, source);

    let query = Query::new(&ts_lang, QUERY_SRC).context("invalid C# query")?;
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
        let mut interface_def = None;
        let mut interface_name = None;
        let mut enum_def = None;
        let mut enum_name = None;
        let mut method_def = None;
        let mut method_name = None;
        let mut call_name = None;

        for cap in m.captures {
            match query.capture_names()[cap.index as usize] {
                "import" => import_node = Some(cap.node),
                "class.def" => class_def = Some(cap.node),
                "class.name" => class_name = Some(cap.node),
                "struct.def" => struct_def = Some(cap.node),
                "struct.name" => struct_name = Some(cap.node),
                "interface.def" => interface_def = Some(cap.node),
                "interface.name" => interface_name = Some(cap.node),
                "enum.def" => enum_def = Some(cap.node),
                "enum.name" => enum_name = Some(cap.node),
                "method.def" => method_def = Some(cap.node),
                "method.name" => method_name = Some(cap.node),
                "call.name" => call_name = Some(cap.node),
                _ => {}
            }
        }

        if let Some(node) = import_node {
            let text = node_text(source, node);
            let path = text
                .trim_start_matches("using")
                .trim()
                .trim_end_matches(';')
                .trim();
            module_concept.relationships.push(import_relationship(path));
        }

        if let (Some(def), Some(name)) = (class_def, class_name) {
            let name_text = node_text(source, name);
            let qualified = format!("{}.{}", module, name_text);
            concepts.push(make_concept(
                ConceptKind::Class,
                Language::CSharp,
                name_text,
                &qualified,
                location(relative_path, def),
                Some(type_signature(source, def)),
                has_public_modifier(source, def),
            ));
        }

        if let (Some(def), Some(name)) = (struct_def, struct_name) {
            let name_text = node_text(source, name);
            let qualified = format!("{}.{}", module, name_text);
            concepts.push(make_concept(
                ConceptKind::Struct,
                Language::CSharp,
                name_text,
                &qualified,
                location(relative_path, def),
                Some(type_signature(source, def)),
                has_public_modifier(source, def),
            ));
        }

        if let (Some(def), Some(name)) = (interface_def, interface_name) {
            let name_text = node_text(source, name);
            let qualified = format!("{}.{}", module, name_text);
            concepts.push(make_concept(
                ConceptKind::Interface,
                Language::CSharp,
                name_text,
                &qualified,
                location(relative_path, def),
                Some(type_signature(source, def)),
                has_public_modifier(source, def),
            ));
        }

        if let (Some(def), Some(name)) = (enum_def, enum_name) {
            let name_text = node_text(source, name);
            let qualified = format!("{}.{}", module, name_text);
            concepts.push(make_concept(
                ConceptKind::Enum,
                Language::CSharp,
                name_text,
                &qualified,
                location(relative_path, def),
                Some(type_signature(source, def)),
                has_public_modifier(source, def),
            ));
        }

        if let (Some(def), Some(name)) = (method_def, method_name) {
            if let Some(container) = container_name(source, def) {
                let method_name_text = node_text(source, name);
                let qualified = format!("{}.{}.{}", module, container, method_name_text);
                let concept = make_concept(
                    ConceptKind::Method,
                    Language::CSharp,
                    method_name_text,
                    &qualified,
                    location(relative_path, def),
                    Some(signature_before_body(source, def)),
                    has_public_modifier(source, def),
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
    fn extracts_classes_interfaces_enums_structs_methods_and_calls() {
        let src = r#"
using System;
using System.Collections.Generic;

namespace Demo {
    public interface IGreeter {
        string Greet(string name);
    }

    public enum Status { Active, Inactive }

    public struct Point {
        public int X;
    }

    public class Auth : IGreeter {
        public bool VerifyToken(string token) {
            return DecodeJwt(token);
        }

        private bool DecodeJwt(string token) {
            return true;
        }

        public string Greet(string name) {
            return "hi " + name;
        }
    }
}
"#;
        let extraction = extract(src, "src/Auth.cs").unwrap();
        let names: Vec<_> = extraction
            .concepts
            .iter()
            .map(|c| c.name.as_str())
            .collect();
        assert!(names.contains(&"IGreeter"));
        assert!(names.contains(&"Status"));
        assert!(names.contains(&"Point"));
        assert!(names.contains(&"Auth"));
        assert!(names.contains(&"VerifyToken"));
        assert!(names.contains(&"DecodeJwt"));

        let module = extraction
            .concepts
            .iter()
            .find(|c| c.kind == ConceptKind::Module)
            .unwrap();
        assert_eq!(module.relationships.len(), 2);
        assert_eq!(module.relationships[0].target_display, "System");
        assert_eq!(
            module.relationships[1].target_display,
            "System.Collections.Generic"
        );

        let point = extraction
            .concepts
            .iter()
            .find(|c| c.name == "Point")
            .unwrap();
        assert_eq!(point.kind, ConceptKind::Struct);

        let verify = extraction
            .concepts
            .iter()
            .find(|c| c.name == "VerifyToken")
            .unwrap();
        assert_eq!(verify.kind, ConceptKind::Method);

        assert_eq!(extraction.calls.len(), 1);
        assert_eq!(extraction.calls[0].callee_name, "DecodeJwt");
    }

    #[test]
    fn detects_public_modifier_only_as_public() {
        let src = r#"
public class Public {}
class AssemblyPrivate {}

public class Holder {
    public void PublicMethod() {}
    private void PrivateMethod() {}
    protected void ProtectedMethod() {}
    internal void InternalMethod() {}
    void DefaultMethod() {}
}
"#;
        let extraction = extract(src, "src/Holder.cs").unwrap();
        let find = |name: &str| extraction.concepts.iter().find(|c| c.name == name).unwrap();

        assert!(find("Public").is_public);
        assert!(!find("AssemblyPrivate").is_public);
        assert!(find("PublicMethod").is_public);
        assert!(!find("PrivateMethod").is_public);
        assert!(!find("ProtectedMethod").is_public);
        assert!(!find("InternalMethod").is_public);
        assert!(!find("DefaultMethod").is_public);
    }

    #[test]
    fn captures_this_object_static_and_bare_calls() {
        let src = r#"
class Util {
    public static void Log(string s) {}
}

class Service {
    void Run() {
        Helper();
        this.Helper();
        Util.Log("hi");
    }
    void Helper() {}
}
"#;
        let extraction = extract(src, "src/Service.cs").unwrap();
        let callee_names: Vec<_> = extraction
            .calls
            .iter()
            .map(|c| c.callee_name.as_str())
            .collect();
        assert!(callee_names.contains(&"Helper"));
        assert!(
            callee_names.iter().filter(|&&n| n == "Helper").count() >= 2,
            "expected both Helper() and this.Helper() to be captured, got {callee_names:?}"
        );
        assert!(callee_names.contains(&"Log"), "expected Util.Log(...)");
    }

    #[test]
    fn constructors_are_extracted_as_methods_scoped_to_their_class() {
        let src = r#"
public class Widget {
    public Widget(string name) {
        this.name = name;
    }
    private string name;
}
"#;
        let extraction = extract(src, "src/Widget.cs").unwrap();
        let ctor = extraction
            .concepts
            .iter()
            .find(|c| c.name == "Widget" && c.kind == ConceptKind::Method)
            .expect("constructor should be extracted as a Method");
        assert!(ctor.id.contains("Widget/Widget"), "got id {}", ctor.id);
        assert!(ctor.is_public);
    }
}
