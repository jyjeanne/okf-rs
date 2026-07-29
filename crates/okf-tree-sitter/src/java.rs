//! Java extraction. Like the other class-based languages, one `Module`
//! concept per file (see `common::module_path`); unlike JavaScript/
//! TypeScript, Java has no free-standing functions — every method or
//! constructor is inside a class, interface, or enum body, so containment
//! is resolved the same way for all three via [`container_name`].
//!
//! `method_invocation`'s `name` field is just the method identifier
//! regardless of receiver, so one query pattern captures bare calls
//! (`foo()`), `this.foo()`, `obj.foo()`, and `ClassName.staticFoo()`
//! uniformly — unlike Rust, which needs a separate pattern per call shape.

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
(import_declaration) @import
(class_declaration (modifiers)? @class.mods name: (identifier) @class.name) @class.def
(interface_declaration (modifiers)? @interface.mods name: (identifier) @interface.name) @interface.def
(enum_declaration (modifiers)? @enum.mods name: (identifier) @enum.name) @enum.def
(method_declaration (modifiers)? @method.mods name: (identifier) @method.name) @method.def
(constructor_declaration (modifiers)? @method.mods name: (identifier) @method.name) @method.def
(method_invocation name: (identifier) @call.name) @call.def
"#;

/// The enclosing class/interface/enum name of a method or constructor
/// declaration, if any. Enum bodies wrap their member declarations one
/// level deeper, in an `enum_body_declarations` node, so that case walks
/// up one extra level before reaching the `enum_body`.
fn container_name<'a>(src: &'a str, def_node: Node) -> Option<&'a str> {
    let mut body = def_node.parent()?;
    if body.kind() == "enum_body_declarations" {
        body = body.parent()?;
    }
    if !matches!(body.kind(), "class_body" | "interface_body" | "enum_body") {
        return None;
    }
    let decl = body.parent()?;
    let name = decl.child_by_field_name("name")?;
    Some(node_text(src, name))
}

/// Java's actual visibility rule for this purpose: `public` is the only
/// modifier that makes a member part of the API surface. Package-private
/// (no modifier), `private`, and `protected` are all treated as private,
/// the same way Rust only counts an explicit `pub`.
fn has_public_modifier(src: &str, modifiers_node: Option<Node>) -> bool {
    let Some(modifiers) = modifiers_node else {
        return false;
    };
    let mut cursor = modifiers.walk();
    let is_public = modifiers
        .children(&mut cursor)
        .any(|child| node_text(src, child) == "public");
    is_public
}

pub fn extract(source: &str, relative_path: &str) -> Result<FileExtraction> {
    let ts_lang: tree_sitter::Language = tree_sitter_java::LANGUAGE.into();
    let mut parser = Parser::new();
    parser
        .set_language(&ts_lang)
        .context("failed to load Java grammar")?;
    let tree = parser
        .parse(source, None)
        .context("failed to parse Java source")?;

    let module = module_path(relative_path);
    let mut module_concept = module_concept(Language::Java, relative_path, source);

    let query = Query::new(&ts_lang, QUERY_SRC).context("invalid Java query")?;
    let mut cursor = QueryCursor::new();
    let mut matches = cursor.matches(&query, tree.root_node(), source.as_bytes());

    let mut concepts: Vec<Concept> = Vec::new();
    let mut function_spans: Vec<(String, Range<usize>)> = Vec::new();
    let mut raw_calls: Vec<(Range<usize>, String, CallSite)> = Vec::new();

    while let Some(m) = matches.next() {
        let mut import_node = None;
        let mut class_def = None;
        let mut class_name = None;
        let mut class_mods = None;
        let mut interface_def = None;
        let mut interface_name = None;
        let mut interface_mods = None;
        let mut enum_def = None;
        let mut enum_name = None;
        let mut enum_mods = None;
        let mut method_def = None;
        let mut method_name = None;
        let mut method_mods = None;
        let mut call_name = None;

        for cap in m.captures {
            match query.capture_names()[cap.index as usize] {
                "import" => import_node = Some(cap.node),
                "class.def" => class_def = Some(cap.node),
                "class.name" => class_name = Some(cap.node),
                "class.mods" => class_mods = Some(cap.node),
                "interface.def" => interface_def = Some(cap.node),
                "interface.name" => interface_name = Some(cap.node),
                "interface.mods" => interface_mods = Some(cap.node),
                "enum.def" => enum_def = Some(cap.node),
                "enum.name" => enum_name = Some(cap.node),
                "enum.mods" => enum_mods = Some(cap.node),
                "method.def" => method_def = Some(cap.node),
                "method.name" => method_name = Some(cap.node),
                "method.mods" => method_mods = Some(cap.node),
                "call.name" => call_name = Some(cap.node),
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

        if let (Some(def), Some(name)) = (class_def, class_name) {
            let name_text = node_text(source, name);
            let qualified = format!("{}.{}", module, name_text);
            concepts.push(make_concept(
                ConceptKind::Class,
                Language::Java,
                name_text,
                &qualified,
                location(relative_path, def),
                Some(type_signature(source, def)),
                has_public_modifier(source, class_mods),
            ));
        }

        if let (Some(def), Some(name)) = (interface_def, interface_name) {
            let name_text = node_text(source, name);
            let qualified = format!("{}.{}", module, name_text);
            concepts.push(make_concept(
                ConceptKind::Interface,
                Language::Java,
                name_text,
                &qualified,
                location(relative_path, def),
                Some(type_signature(source, def)),
                has_public_modifier(source, interface_mods),
            ));
        }

        if let (Some(def), Some(name)) = (enum_def, enum_name) {
            let name_text = node_text(source, name);
            let qualified = format!("{}.{}", module, name_text);
            concepts.push(make_concept(
                ConceptKind::Enum,
                Language::Java,
                name_text,
                &qualified,
                location(relative_path, def),
                Some(type_signature(source, def)),
                has_public_modifier(source, enum_mods),
            ));
        }

        if let (Some(def), Some(name)) = (method_def, method_name) {
            if let Some(container) = container_name(source, def) {
                let method_name_text = node_text(source, name);
                let qualified = format!("{}.{}.{}", module, container, method_name_text);
                let concept = make_concept(
                    ConceptKind::Method,
                    Language::Java,
                    method_name_text,
                    &qualified,
                    location(relative_path, def),
                    Some(signature_before_body(source, def)),
                    has_public_modifier(source, method_mods),
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
    fn extracts_classes_interfaces_enums_methods_and_calls() {
        let src = r#"
import java.util.List;

public interface Greeter {
    String greet(String name);
}

enum Status {
    ACTIVE, INACTIVE
}

public class Auth implements Greeter {
    public boolean verifyToken(String token) {
        return decodeJwt(token);
    }

    private boolean decodeJwt(String token) {
        return true;
    }

    public String greet(String name) {
        return "hi " + name;
    }
}
"#;
        let extraction = extract(src, "src/Auth.java").unwrap();
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

        let module = extraction
            .concepts
            .iter()
            .find(|c| c.kind == ConceptKind::Module)
            .unwrap();
        assert_eq!(module.relationships.len(), 1);
        assert_eq!(module.relationships[0].target_display, "java.util.List");

        let verify = extraction
            .concepts
            .iter()
            .find(|c| c.name == "verifyToken")
            .unwrap();
        assert_eq!(verify.kind, ConceptKind::Method);

        assert_eq!(extraction.calls.len(), 1);
        assert_eq!(extraction.calls[0].callee_name, "decodeJwt");
    }

    #[test]
    fn detects_public_modifier_only_as_public() {
        let src = r#"
public class Public {}
class PackagePrivate {}

public class Holder {
    public void publicMethod() {}
    private void privateMethod() {}
    protected void protectedMethod() {}
    void packagePrivateMethod() {}
}
"#;
        let extraction = extract(src, "src/Holder.java").unwrap();
        let find = |name: &str| extraction.concepts.iter().find(|c| c.name == name).unwrap();

        assert!(find("Public").is_public);
        assert!(!find("PackagePrivate").is_public);
        assert!(find("publicMethod").is_public);
        assert!(!find("privateMethod").is_public);
        assert!(!find("protectedMethod").is_public);
        assert!(!find("packagePrivateMethod").is_public);
    }

    #[test]
    fn captures_this_object_static_and_bare_calls() {
        let src = r#"
class Util {
    static void log(String s) {}
}

class Service {
    void run() {
        helper();
        this.helper();
        Util.log("hi");
        System.out.println("hi");
    }
    void helper() {}
}
"#;
        let extraction = extract(src, "src/Service.java").unwrap();
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
        assert!(callee_names.contains(&"log"), "expected Util.log(...)");
        assert!(
            callee_names.contains(&"println"),
            "expected System.out.println(...)"
        );
    }

    #[test]
    fn constructors_are_extracted_as_methods_scoped_to_their_class() {
        let src = r#"
public class Widget {
    public Widget(String name) {
        this.name = name;
    }
    private String name;
}
"#;
        let extraction = extract(src, "src/Widget.java").unwrap();
        let ctor = extraction
            .concepts
            .iter()
            .find(|c| c.name == "Widget" && c.kind == ConceptKind::Method)
            .expect("constructor should be extracted as a Method");
        assert!(ctor.id.contains("Widget/Widget"), "got id {}", ctor.id);
        assert!(ctor.is_public);
    }
}
