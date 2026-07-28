use crate::common::{
    import_relationship, is_public_by_underscore_convention, location, make_concept,
    module_concept, module_path, node_text, smallest_containing,
};
use crate::{CallCandidate, FileExtraction};
use anyhow::{Context, Result};
use okf_parser::{Concept, ConceptKind, Language};
use std::ops::Range;
use tree_sitter::{Node, Parser, Query, QueryCursor, StreamingIterator};

const QUERY_SRC: &str = r#"
(import_statement name: (_) @import.name)
(import_from_statement module_name: (_) @import.name)
(class_definition name: (identifier) @class.name) @class.def
(function_definition name: (identifier) @fn.name) @fn.def
(call function: (identifier) @call.name) @call.def
(call function: (attribute attribute: (identifier) @call.name)) @call.def
"#;

fn container_name<'a>(src: &'a str, function_node: Node) -> Option<&'a str> {
    // A decorated method's real parent is `decorated_definition` (the
    // decorator wraps it), which itself sits in the class's `block` — skip
    // over that wrapper so `@staticmethod`/`@classmethod`/`@property`/any
    // decorator doesn't break class-membership detection.
    let mut parent = function_node.parent()?;
    if parent.kind() == "decorated_definition" {
        parent = parent.parent()?;
    }
    if parent.kind() != "block" {
        return None;
    }
    let grand = parent.parent()?;
    if grand.kind() != "class_definition" {
        return None;
    }
    let name = grand.child_by_field_name("name")?;
    Some(node_text(src, name))
}

fn signature_before_body(src: &str, def_node: Node) -> String {
    let end = def_node
        .child_by_field_name("body")
        .map(|b| b.start_byte())
        .unwrap_or(def_node.end_byte());
    src[def_node.start_byte()..end]
        .trim_end_matches(':')
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

pub fn extract(source: &str, relative_path: &str) -> Result<FileExtraction> {
    let ts_lang: tree_sitter::Language = tree_sitter_python::LANGUAGE.into();
    let mut parser = Parser::new();
    parser
        .set_language(&ts_lang)
        .context("failed to load Python grammar")?;
    let tree = parser
        .parse(source, None)
        .context("failed to parse Python source")?;

    let module = module_path(relative_path);
    let mut module_concept = module_concept(Language::Python, relative_path, source);

    let query = Query::new(&ts_lang, QUERY_SRC).context("invalid Python query")?;
    let mut cursor = QueryCursor::new();
    let mut matches = cursor.matches(&query, tree.root_node(), source.as_bytes());

    let mut concepts: Vec<Concept> = Vec::new();
    let mut function_spans: Vec<(String, Range<usize>)> = Vec::new();
    let mut raw_calls: Vec<(Range<usize>, String)> = Vec::new();

    while let Some(m) = matches.next() {
        let mut import_name = None;
        let mut class_def = None;
        let mut class_name = None;
        let mut fn_def = None;
        let mut fn_name = None;
        let mut call_name = None;

        for cap in m.captures {
            match query.capture_names()[cap.index as usize] {
                "import.name" => import_name = Some(cap.node),
                "class.def" => class_def = Some(cap.node),
                "class.name" => class_name = Some(cap.node),
                "fn.def" => fn_def = Some(cap.node),
                "fn.name" => fn_name = Some(cap.node),
                "call.name" => call_name = Some(cap.node),
                _ => {}
            }
        }

        if let Some(node) = import_name {
            module_concept
                .relationships
                .push(import_relationship(node_text(source, node)));
        }

        if let (Some(def), Some(name)) = (class_def, class_name) {
            let name_text = node_text(source, name);
            let qualified = format!("{}.{}", module, name_text);
            concepts.push(make_concept(
                ConceptKind::Class,
                Language::Python,
                name_text,
                &qualified,
                location(relative_path, def),
                Some(signature_before_body(source, def)),
                is_public_by_underscore_convention(name_text),
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
                Language::Python,
                fn_name_text,
                &qualified,
                location(relative_path, def),
                Some(signature_before_body(source, def)),
                is_public_by_underscore_convention(fn_name_text),
            );
            function_spans.push((concept.id.clone(), def.byte_range()));
            concepts.push(concept);
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
    fn extracts_class_methods_and_calls() {
        let src = r#"
import os
from typing import List

class Foo:
    def bar(self):
        baz()

def baz():
    pass
"#;
        let extraction = extract(src, "pkg/foo.py").unwrap();
        let names: Vec<_> = extraction
            .concepts
            .iter()
            .map(|c| c.name.as_str())
            .collect();
        assert!(names.contains(&"foo"));
        assert!(names.contains(&"Foo"));
        assert!(names.contains(&"bar"));
        assert!(names.contains(&"baz"));

        let bar = extraction
            .concepts
            .iter()
            .find(|c| c.name == "bar")
            .unwrap();
        assert_eq!(bar.kind, ConceptKind::Method);
        assert_eq!(bar.qualified_name, "pkg.foo.Foo.bar");

        let module = extraction
            .concepts
            .iter()
            .find(|c| c.name == "foo")
            .unwrap();
        assert_eq!(module.relationships.len(), 2);

        assert_eq!(extraction.calls.len(), 1);
        assert_eq!(extraction.calls[0].callee_name, "baz");
    }

    #[test]
    fn decorated_methods_stay_class_members() {
        let src = r#"
class Foo:
    @staticmethod
    def bar():
        pass

    def baz(self):
        pass
"#;
        let extraction = extract(src, "foo.py").unwrap();

        let bar = extraction
            .concepts
            .iter()
            .find(|c| c.name == "bar")
            .unwrap();
        assert_eq!(bar.kind, ConceptKind::Method);
        assert_eq!(bar.qualified_name, "foo.Foo.bar");

        let baz = extraction
            .concepts
            .iter()
            .find(|c| c.name == "baz")
            .unwrap();
        assert_eq!(baz.kind, ConceptKind::Method);
    }

    #[test]
    fn captures_self_and_attribute_calls() {
        let src = r#"
class Foo:
    def bar(self):
        self.helper()

    def helper(self):
        pass
"#;
        let extraction = extract(src, "foo.py").unwrap();
        assert!(extraction.calls.iter().any(|c| c.callee_name == "helper"));
    }

    #[test]
    fn detects_underscore_convention_visibility() {
        let src = r#"
def public_fn():
    pass

def _private_fn():
    pass

class Foo:
    def public_method(self):
        pass

    def _private_method(self):
        pass
"#;
        let extraction = extract(src, "foo.py").unwrap();
        let find = |name: &str| extraction.concepts.iter().find(|c| c.name == name).unwrap();

        assert!(find("public_fn").is_public);
        assert!(!find("_private_fn").is_public);
        assert!(find("public_method").is_public);
        assert!(!find("_private_method").is_public);
    }
}
