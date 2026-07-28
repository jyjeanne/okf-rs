use crate::common::{
    import_relationship, location, make_concept, module_path, node_text, smallest_containing,
};
use crate::{CallCandidate, FileExtraction};
use anyhow::{Context, Result};
use okf_parser::{Concept, ConceptKind, Language};
use std::ops::Range;
use tree_sitter::{Node, Parser, Query, QueryCursor, StreamingIterator};

const QUERY_SRC: &str = r#"
(use_declaration) @import
(struct_item name: (type_identifier) @struct.name) @struct.def
(enum_item name: (type_identifier) @enum.name) @enum.def
(trait_item name: (type_identifier) @trait.name) @trait.def
(function_item name: (identifier) @fn.name) @fn.def
(call_expression function: (identifier) @call.name) @call.def
"#;

/// Finds the containing `impl`/`trait` block of a `function_item`, if any,
/// treating the function as a method rather than a top-level function.
fn container_name<'a>(src: &'a str, function_node: Node) -> Option<&'a str> {
    let parent = function_node.parent()?;
    if parent.kind() != "declaration_list" {
        return None;
    }
    let grand = parent.parent()?;
    let field = match grand.kind() {
        "impl_item" => grand.child_by_field_name("type")?,
        "trait_item" => grand.child_by_field_name("name")?,
        _ => return None,
    };
    let text = node_text(src, field);
    Some(text.split('<').next().unwrap_or(text).trim())
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

fn type_signature(src: &str, def_node: Node) -> String {
    let text = node_text(src, def_node);
    text.split(['{', ';'])
        .next()
        .unwrap_or(text)
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

pub fn extract(source: &str, relative_path: &str) -> Result<FileExtraction> {
    let ts_lang: tree_sitter::Language = tree_sitter_rust::LANGUAGE.into();
    let mut parser = Parser::new();
    parser
        .set_language(&ts_lang)
        .context("failed to load Rust grammar")?;
    let tree = parser
        .parse(source, None)
        .context("failed to parse Rust source")?;

    let module = module_path(relative_path);
    let mut module_concept = make_concept(
        ConceptKind::Module,
        Language::Rust,
        module.rsplit('.').next().unwrap_or(&module),
        &module,
        okf_parser::Location {
            file: relative_path.to_string(),
            start_line: 1,
            end_line: source.lines().count().max(1),
        },
        None,
    );

    let query = Query::new(&ts_lang, QUERY_SRC).context("invalid Rust query")?;
    let mut cursor = QueryCursor::new();
    let mut matches = cursor.matches(&query, tree.root_node(), source.as_bytes());

    let mut concepts: Vec<Concept> = Vec::new();
    let mut function_spans: Vec<(String, Range<usize>)> = Vec::new();
    let mut raw_calls: Vec<(Range<usize>, String)> = Vec::new();

    while let Some(m) = matches.next() {
        let mut import_node = None;
        let mut struct_def = None;
        let mut struct_name = None;
        let mut enum_def = None;
        let mut enum_name = None;
        let mut trait_def = None;
        let mut trait_name = None;
        let mut fn_def = None;
        let mut fn_name = None;
        let mut call_name = None;

        for cap in m.captures {
            let cap_name = query.capture_names()[cap.index as usize];
            match cap_name {
                "import" => import_node = Some(cap.node),
                "struct.def" => struct_def = Some(cap.node),
                "struct.name" => struct_name = Some(cap.node),
                "enum.def" => enum_def = Some(cap.node),
                "enum.name" => enum_name = Some(cap.node),
                "trait.def" => trait_def = Some(cap.node),
                "trait.name" => trait_name = Some(cap.node),
                "fn.def" => fn_def = Some(cap.node),
                "fn.name" => fn_name = Some(cap.node),
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

        if let (Some(def), Some(name)) = (struct_def, struct_name) {
            let qualified = format!("{}.{}", module, node_text(source, name));
            concepts.push(make_concept(
                ConceptKind::Struct,
                Language::Rust,
                node_text(source, name),
                &qualified,
                location(relative_path, def),
                Some(type_signature(source, def)),
            ));
        }

        if let (Some(def), Some(name)) = (enum_def, enum_name) {
            let qualified = format!("{}.{}", module, node_text(source, name));
            concepts.push(make_concept(
                ConceptKind::Enum,
                Language::Rust,
                node_text(source, name),
                &qualified,
                location(relative_path, def),
                Some(type_signature(source, def)),
            ));
        }

        if let (Some(def), Some(name)) = (trait_def, trait_name) {
            let qualified = format!("{}.{}", module, node_text(source, name));
            concepts.push(make_concept(
                ConceptKind::Trait,
                Language::Rust,
                node_text(source, name),
                &qualified,
                location(relative_path, def),
                Some(type_signature(source, def)),
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
                Language::Rust,
                fn_name_text,
                &qualified,
                location(relative_path, def),
                Some(signature_before_body(source, def)),
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
    fn extracts_functions_struct_and_calls() {
        let src = r#"
use std::collections::HashMap;

pub struct Foo { x: i32 }

pub fn verify_token(token: &str) -> bool {
    decode_jwt(token);
    true
}

fn decode_jwt(token: &str) -> bool { true }

impl Foo {
    pub fn new() -> Self { Foo { x: 0 } }
}
"#;
        let extraction = extract(src, "src/auth.rs").unwrap();
        let names: Vec<_> = extraction
            .concepts
            .iter()
            .map(|c| c.name.as_str())
            .collect();
        assert!(names.contains(&"auth"));
        assert!(names.contains(&"Foo"));
        assert!(names.contains(&"verify_token"));
        assert!(names.contains(&"decode_jwt"));
        assert!(names.contains(&"new"));

        let new_method = extraction
            .concepts
            .iter()
            .find(|c| c.name == "new")
            .unwrap();
        assert_eq!(new_method.kind, ConceptKind::Method);

        let module = extraction
            .concepts
            .iter()
            .find(|c| c.name == "auth")
            .unwrap();
        assert_eq!(module.relationships.len(), 1);
        assert_eq!(
            module.relationships[0].target_display,
            "std::collections::HashMap"
        );

        assert_eq!(extraction.calls.len(), 1);
        assert_eq!(extraction.calls[0].callee_name, "decode_jwt");
    }
}
