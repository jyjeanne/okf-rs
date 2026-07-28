//! PHP extraction. Shaped like Java/C#: one `Module` concept per file,
//! classes/interfaces/traits, and methods/top-level functions.
//!
//! Unlike Java/C#, an unmarked PHP method is genuinely public — PHP's
//! actual default, not just an extraction convenience — so visibility is
//! `true` unless an explicit `private`/`protected` modifier says
//! otherwise (the opposite default from Rust/Java/C#'s "explicit `pub`/
//! `public` required"). Classes/interfaces/traits themselves have no
//! visibility modifier at all in PHP, so they're always public, the same
//! way `Module`/`Package` concepts are.
//!
//! Three query patterns cover PHP's three call shapes: bare
//! `function_call_expression` (`strtoupper($s)`), `member_call_expression`
//! (`$this->foo()`/`$obj->foo()`), and `scoped_call_expression`
//! (`ClassName::staticFoo()`) — each has its own `name` field for the
//! called identifier regardless of receiver.

use crate::common::{
    import_relationship, location, make_concept, module_path, node_text, smallest_containing,
};
use crate::{CallCandidate, FileExtraction};
use anyhow::{Context, Result};
use okf_parser::{Concept, ConceptKind, Language};
use std::ops::Range;
use tree_sitter::{Node, Parser, Query, QueryCursor, StreamingIterator};

const QUERY_SRC: &str = r#"
(namespace_use_declaration) @import
(class_declaration name: (name) @class.name) @class.def
(interface_declaration name: (name) @interface.name) @interface.def
(trait_declaration name: (name) @trait.name) @trait.def
(function_definition name: (name) @fn.name) @fn.def
(method_declaration name: (name) @method.name) @method.def
(function_call_expression function: (name) @call.name) @call.def
(member_call_expression name: (name) @call.name) @call.def
(scoped_call_expression name: (name) @call.name) @call.def
"#;

/// The enclosing class/interface/trait name of a method declaration, if
/// any.
fn container_name<'a>(src: &'a str, def_node: Node) -> Option<&'a str> {
    let body = def_node.parent()?;
    if body.kind() != "declaration_list" {
        return None;
    }
    let decl = body.parent()?;
    let name = decl.child_by_field_name("name")?;
    Some(node_text(src, name))
}

/// PHP's actual default: a method with no visibility modifier is public.
/// Only an explicit `private` or `protected` narrows that — the opposite
/// polarity from Rust/Java/C#, where an *absent* modifier means private.
fn is_public_method(src: &str, def_node: Node) -> bool {
    let mut cursor = def_node.walk();
    let has_restrictive_modifier = def_node.children(&mut cursor).any(|child| {
        child.kind() == "visibility_modifier"
            && matches!(node_text(src, child), "private" | "protected")
    });
    !has_restrictive_modifier
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

/// `class_declaration`/`interface_declaration`/`trait_declaration` have no
/// field usable across all three — truncating at the first `{` gives just
/// the declaration line either way.
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
    let ts_lang: tree_sitter::Language = tree_sitter_php::LANGUAGE_PHP.into();
    let mut parser = Parser::new();
    parser
        .set_language(&ts_lang)
        .context("failed to load PHP grammar")?;
    let tree = parser
        .parse(source, None)
        .context("failed to parse PHP source")?;

    let module = module_path(relative_path);
    let mut module_concept = make_concept(
        ConceptKind::Module,
        Language::Php,
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

    let query = Query::new(&ts_lang, QUERY_SRC).context("invalid PHP query")?;
    let mut cursor = QueryCursor::new();
    let mut matches = cursor.matches(&query, tree.root_node(), source.as_bytes());

    let mut concepts: Vec<Concept> = Vec::new();
    let mut function_spans: Vec<(String, Range<usize>)> = Vec::new();
    let mut raw_calls: Vec<(Range<usize>, String)> = Vec::new();

    while let Some(m) = matches.next() {
        let mut import_node = None;
        let mut class_def = None;
        let mut class_name = None;
        let mut interface_def = None;
        let mut interface_name = None;
        let mut trait_def = None;
        let mut trait_name = None;
        let mut fn_def = None;
        let mut fn_name = None;
        let mut method_def = None;
        let mut method_name = None;
        let mut call_name = None;

        for cap in m.captures {
            match query.capture_names()[cap.index as usize] {
                "import" => import_node = Some(cap.node),
                "class.def" => class_def = Some(cap.node),
                "class.name" => class_name = Some(cap.node),
                "interface.def" => interface_def = Some(cap.node),
                "interface.name" => interface_name = Some(cap.node),
                "trait.def" => trait_def = Some(cap.node),
                "trait.name" => trait_name = Some(cap.node),
                "fn.def" => fn_def = Some(cap.node),
                "fn.name" => fn_name = Some(cap.node),
                "method.def" => method_def = Some(cap.node),
                "method.name" => method_name = Some(cap.node),
                "call.name" => call_name = Some(cap.node),
                _ => {}
            }
        }

        if let Some(node) = import_node {
            let text = node_text(source, node);
            let path = text
                .trim_start_matches("use")
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
                Language::Php,
                name_text,
                &qualified,
                location(relative_path, def),
                Some(type_signature(source, def)),
                true,
            ));
        }

        if let (Some(def), Some(name)) = (interface_def, interface_name) {
            let name_text = node_text(source, name);
            let qualified = format!("{}.{}", module, name_text);
            concepts.push(make_concept(
                ConceptKind::Interface,
                Language::Php,
                name_text,
                &qualified,
                location(relative_path, def),
                Some(type_signature(source, def)),
                true,
            ));
        }

        if let (Some(def), Some(name)) = (trait_def, trait_name) {
            let name_text = node_text(source, name);
            let qualified = format!("{}.{}", module, name_text);
            concepts.push(make_concept(
                ConceptKind::Trait,
                Language::Php,
                name_text,
                &qualified,
                location(relative_path, def),
                Some(type_signature(source, def)),
                true,
            ));
        }

        if let (Some(def), Some(name)) = (fn_def, fn_name) {
            let fn_name_text = node_text(source, name);
            let qualified = format!("{}.{}", module, fn_name_text);
            let concept = make_concept(
                ConceptKind::Function,
                Language::Php,
                fn_name_text,
                &qualified,
                location(relative_path, def),
                Some(signature_before_body(source, def)),
                true,
            );
            function_spans.push((concept.id.clone(), def.byte_range()));
            concepts.push(concept);
        }

        if let (Some(def), Some(name)) = (method_def, method_name) {
            if let Some(container) = container_name(source, def) {
                let method_name_text = node_text(source, name);
                let qualified = format!("{}.{}.{}", module, container, method_name_text);
                let concept = make_concept(
                    ConceptKind::Method,
                    Language::Php,
                    method_name_text,
                    &qualified,
                    location(relative_path, def),
                    Some(signature_before_body(source, def)),
                    is_public_method(source, def),
                );
                function_spans.push((concept.id.clone(), def.byte_range()));
                concepts.push(concept);
            }
        }

        if let Some(node) = call_name {
            raw_calls.push((node.byte_range(), node_text(source, node).to_string()));
        }
    }

    let mut calls = Vec::new();
    for (range, callee_name) in raw_calls {
        if let Some(caller_id) = smallest_containing(&function_spans, range.start) {
            calls.push(CallCandidate {
                caller_id: caller_id.to_string(),
                callee_name,
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
    fn extracts_classes_interfaces_traits_functions_methods_and_calls() {
        let src = r#"<?php
namespace Demo;

use Foo\Bar;

interface Greeter {
    public function greet(string $name): string;
}

trait Loud {
    public function shout(string $s): string {
        return strtoupper($s);
    }
}

class Auth implements Greeter {
    use Loud;

    public function verifyToken(string $token): bool {
        return $this->decodeJwt($token);
    }

    private function decodeJwt(string $token): bool {
        return true;
    }

    public function greet(string $name): string {
        return "hi " . $name;
    }
}

function topLevel() {
    return Auth::class;
}
"#;
        let extraction = extract(src, "src/Auth.php").unwrap();
        let names: Vec<_> = extraction
            .concepts
            .iter()
            .map(|c| c.name.as_str())
            .collect();
        assert!(names.contains(&"Greeter"));
        assert!(names.contains(&"Loud"));
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
        assert_eq!(module.relationships[0].target_display, "Foo\\Bar");

        let loud = extraction
            .concepts
            .iter()
            .find(|c| c.name == "Loud")
            .unwrap();
        assert_eq!(loud.kind, ConceptKind::Trait);

        let top_level = extraction
            .concepts
            .iter()
            .find(|c| c.name == "topLevel")
            .unwrap();
        assert_eq!(top_level.kind, ConceptKind::Function);

        assert_eq!(extraction.calls.len(), 2);
        let callee_names: Vec<_> = extraction
            .calls
            .iter()
            .map(|c| c.callee_name.as_str())
            .collect();
        assert!(callee_names.contains(&"decodeJwt"));
        assert!(callee_names.contains(&"strtoupper"));
    }

    #[test]
    fn a_method_is_public_by_default_unless_marked_private_or_protected() {
        let src = r#"<?php
class Holder {
    public function publicMethod() {}
    private function privateMethod() {}
    protected function protectedMethod() {}
    function unmarkedMethod() {}
}
"#;
        let extraction = extract(src, "src/Holder.php").unwrap();
        let find = |name: &str| extraction.concepts.iter().find(|c| c.name == name).unwrap();

        assert!(find("publicMethod").is_public);
        assert!(!find("privateMethod").is_public);
        assert!(!find("protectedMethod").is_public);
        assert!(
            find("unmarkedMethod").is_public,
            "PHP's actual default: no modifier means public"
        );
    }

    #[test]
    fn captures_this_static_and_bare_calls() {
        let src = r#"<?php
class Util {
    public static function log(string $s): void {}
}

class Service {
    public function run(): void {
        $this->helper();
        Util::log("hi");
        strtoupper("hi");
    }
    public function helper(): void {}
}
"#;
        let extraction = extract(src, "src/Service.php").unwrap();
        let callee_names: Vec<_> = extraction
            .calls
            .iter()
            .map(|c| c.callee_name.as_str())
            .collect();
        assert!(callee_names.contains(&"helper"), "expected $this->helper()");
        assert!(callee_names.contains(&"log"), "expected Util::log(...)");
        assert!(
            callee_names.contains(&"strtoupper"),
            "expected bare strtoupper(...)"
        );
    }
}
