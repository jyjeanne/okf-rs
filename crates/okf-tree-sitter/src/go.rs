use crate::common::{
    import_relationship, is_public_by_go_convention, location, make_concept, module_path,
    node_text, smallest_containing, strip_quotes,
};
use crate::{CallCandidate, FileExtraction};
use anyhow::Context;
use anyhow::Result;
use okf_parser::{Concept, ConceptKind, Language};
use std::ops::Range;
use tree_sitter::{Node, Parser, Query, QueryCursor, StreamingIterator};

const QUERY_SRC: &str = r#"
(import_spec path: (interpreted_string_literal) @import.path)
(type_spec name: (type_identifier) @struct.name type: (struct_type)) @struct.def
(type_spec name: (type_identifier) @interface.name type: (interface_type)) @interface.def
(method_declaration receiver: (parameter_list (parameter_declaration type: (_) @method.receiver)) name: (field_identifier) @method.name) @method.def
(function_declaration name: (identifier) @fn.name) @fn.def
(call_expression function: (identifier) @call.name) @call.def
(call_expression function: (selector_expression field: (field_identifier) @call.name)) @call.def
"#;

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

/// `type_spec` nodes (struct/interface definitions) have no `body` field,
/// so `signature_before_body` can't be used for them — this truncates at
/// the first `{` instead, giving just the declaration line.
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
    let ts_lang: tree_sitter::Language = tree_sitter_go::LANGUAGE.into();
    let mut parser = Parser::new();
    parser
        .set_language(&ts_lang)
        .context("failed to load Go grammar")?;
    let tree = parser
        .parse(source, None)
        .context("failed to parse Go source")?;

    let module = module_path(relative_path);
    let mut module_concept = make_concept(
        ConceptKind::Module,
        Language::Go,
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

    let query = Query::new(&ts_lang, QUERY_SRC).context("invalid Go query")?;
    let mut cursor = QueryCursor::new();
    let mut matches = cursor.matches(&query, tree.root_node(), source.as_bytes());

    let mut concepts: Vec<Concept> = Vec::new();
    let mut function_spans: Vec<(String, Range<usize>)> = Vec::new();
    let mut raw_calls: Vec<(Range<usize>, String)> = Vec::new();

    while let Some(m) = matches.next() {
        let mut import_path = None;
        let mut struct_def = None;
        let mut struct_name = None;
        let mut interface_def = None;
        let mut interface_name = None;
        let mut method_def = None;
        let mut method_name = None;
        let mut method_receiver = None;
        let mut fn_def = None;
        let mut fn_name = None;
        let mut call_name = None;

        for cap in m.captures {
            match query.capture_names()[cap.index as usize] {
                "import.path" => import_path = Some(cap.node),
                "struct.def" => struct_def = Some(cap.node),
                "struct.name" => struct_name = Some(cap.node),
                "interface.def" => interface_def = Some(cap.node),
                "interface.name" => interface_name = Some(cap.node),
                "method.def" => method_def = Some(cap.node),
                "method.name" => method_name = Some(cap.node),
                "method.receiver" => method_receiver = Some(cap.node),
                "fn.def" => fn_def = Some(cap.node),
                "fn.name" => fn_name = Some(cap.node),
                "call.name" => call_name = Some(cap.node),
                _ => {}
            }
        }

        if let Some(node) = import_path {
            let raw = strip_quotes(node_text(source, node));
            module_concept.relationships.push(import_relationship(&raw));
        }

        if let (Some(def), Some(name)) = (struct_def, struct_name) {
            let name_text = node_text(source, name);
            let qualified = format!("{}.{}", module, name_text);
            concepts.push(make_concept(
                ConceptKind::Struct,
                Language::Go,
                name_text,
                &qualified,
                location(relative_path, def),
                Some(type_signature(source, def)),
                is_public_by_go_convention(name_text),
            ));
        }

        if let (Some(def), Some(name)) = (interface_def, interface_name) {
            let name_text = node_text(source, name);
            let qualified = format!("{}.{}", module, name_text);
            concepts.push(make_concept(
                ConceptKind::Interface,
                Language::Go,
                name_text,
                &qualified,
                location(relative_path, def),
                Some(type_signature(source, def)),
                is_public_by_go_convention(name_text),
            ));
        }

        if let (Some(def), Some(name)) = (fn_def, fn_name) {
            let fn_name_text = node_text(source, name);
            let qualified = format!("{}.{}", module, fn_name_text);
            let concept = make_concept(
                ConceptKind::Function,
                Language::Go,
                fn_name_text,
                &qualified,
                location(relative_path, def),
                Some(signature_before_body(source, def)),
                is_public_by_go_convention(fn_name_text),
            );
            function_spans.push((concept.id.clone(), def.byte_range()));
            concepts.push(concept);
        }

        if let (Some(def), Some(name)) = (method_def, method_name) {
            let method_name_text = node_text(source, name);
            let receiver = method_receiver
                .map(|n| node_text(source, n).trim_start_matches('*').to_string())
                .unwrap_or_default();
            let qualified = if receiver.is_empty() {
                format!("{}.{}", module, method_name_text)
            } else {
                format!("{}.{}.{}", module, receiver, method_name_text)
            };
            let concept = make_concept(
                ConceptKind::Method,
                Language::Go,
                method_name_text,
                &qualified,
                location(relative_path, def),
                Some(signature_before_body(source, def)),
                is_public_by_go_convention(method_name_text),
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
    fn extracts_struct_methods_functions_and_calls() {
        let src = r#"
package main

import (
    "fmt"
    "os"
)

type Foo struct {
    X int
}

func (f *Foo) Bar() {
    Baz()
}

func Baz() {
    fmt.Println("hi")
}
"#;
        let extraction = extract(src, "pkg/main.go").unwrap();
        let names: Vec<_> = extraction
            .concepts
            .iter()
            .map(|c| c.name.as_str())
            .collect();
        assert!(names.contains(&"Foo"));
        assert!(names.contains(&"Bar"));
        assert!(names.contains(&"Baz"));

        let bar = extraction
            .concepts
            .iter()
            .find(|c| c.name == "Bar")
            .unwrap();
        assert_eq!(bar.kind, ConceptKind::Method);
        // "main.go" is treated like an index file (its module is the
        // containing directory "pkg"), matching the same convention used
        // for mod.rs/lib.rs/__init__.py/index.ts.
        assert_eq!(bar.qualified_name, "pkg.Foo.Bar");

        let module = extraction
            .concepts
            .iter()
            .find(|c| c.kind == ConceptKind::Module)
            .unwrap();
        assert_eq!(module.relationships.len(), 2);

        // Baz() (bare identifier, from Bar) and fmt.Println (selector
        // expression, from Baz) are both now captured as call candidates.
        assert_eq!(extraction.calls.len(), 2);
        let callee_names: Vec<_> = extraction
            .calls
            .iter()
            .map(|c| c.callee_name.as_str())
            .collect();
        assert!(callee_names.contains(&"Baz"));
        assert!(callee_names.contains(&"Println"));
    }

    #[test]
    fn extracts_interface_and_selector_calls_to_local_methods() {
        let src = r#"
package pkg

type Reader interface {
	Read([]byte) (int, error)
}

type Foo struct{}

func (f *Foo) Helper() {}

func Run(f *Foo) {
	f.Helper()
}
"#;
        let extraction = extract(src, "pkg/reader.go").unwrap();

        let reader = extraction
            .concepts
            .iter()
            .find(|c| c.name == "Reader")
            .unwrap();
        assert_eq!(reader.kind, ConceptKind::Interface);
        assert_eq!(reader.signature.as_deref(), Some("Reader interface"));

        assert_eq!(extraction.calls.len(), 1);
        assert_eq!(extraction.calls[0].callee_name, "Helper");
    }

    #[test]
    fn detects_exported_identifier_convention() {
        let src = r#"
package pkg

type Public struct{}
type private struct{}

func Exported() {}
func unexported() {}
"#;
        let extraction = extract(src, "pkg/x.go").unwrap();
        let find = |name: &str| extraction.concepts.iter().find(|c| c.name == name).unwrap();

        assert!(find("Public").is_public);
        assert!(!find("private").is_public);
        assert!(find("Exported").is_public);
        assert!(!find("unexported").is_public);
    }
}
