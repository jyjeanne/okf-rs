//! Shared extraction logic for TypeScript and JavaScript, whose grammars
//! use identical node shapes for classes, functions, methods, and calls
//! (TypeScript additionally has `interface_declaration`, which JavaScript
//! lacks).

use crate::common::{
    import_relationship, is_public_by_underscore_convention, location, make_concept, module_path,
    node_text, smallest_containing, strip_quotes,
};
use crate::{CallCandidate, FileExtraction};
use anyhow::{Context, Result};
use okf_parser::{Concept, ConceptKind, Language};
use std::ops::Range;
use tree_sitter::{Node, Parser, Query, QueryCursor, StreamingIterator};

const BASE_QUERY_SRC: &str = r#"
(import_statement source: (string) @import.source)
(function_declaration name: (identifier) @fn.name) @fn.def
(method_definition name: (property_identifier) @method.name) @method.def
(call_expression function: (identifier) @call.name) @call.def
(call_expression function: (member_expression property: (property_identifier) @call.name)) @call.def
"#;

/// TypeScript-only: JavaScript's grammar has no `interface_declaration`
/// node, so this pattern is only appended when parsing TypeScript.
const INTERFACE_QUERY_SRC: &str = r#"
(interface_declaration name: (type_identifier) @interface.name) @interface.def
"#;

fn class_container_name<'a>(src: &'a str, method_node: Node) -> Option<&'a str> {
    let class_body = method_node.parent()?;
    if class_body.kind() != "class_body" {
        return None;
    }
    let class_decl = class_body.parent()?;
    let name = class_decl.child_by_field_name("name")?;
    Some(node_text(src, name))
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

pub fn extract(
    source: &str,
    relative_path: &str,
    language: Language,
    ts_lang: tree_sitter::Language,
    include_interfaces: bool,
) -> Result<FileExtraction> {
    // TypeScript's `class_declaration` names are `type_identifier`;
    // JavaScript (no type system) uses plain `identifier`.
    let class_name_node = if language == Language::TypeScript {
        "type_identifier"
    } else {
        "identifier"
    };
    let mut parser = Parser::new();
    parser
        .set_language(&ts_lang)
        .with_context(|| format!("failed to load {} grammar", language))?;
    let tree = parser
        .parse(source, None)
        .with_context(|| format!("failed to parse {} source", language))?;

    let module = module_path(relative_path);
    let mut module_concept = make_concept(
        ConceptKind::Module,
        language,
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

    let mut query_src = format!(
        "{}\n(class_declaration name: ({}) @class.name) @class.def\n",
        BASE_QUERY_SRC, class_name_node
    );
    if include_interfaces {
        query_src.push_str(INTERFACE_QUERY_SRC);
    }
    let query =
        Query::new(&ts_lang, &query_src).with_context(|| format!("invalid {} query", language))?;
    let mut cursor = QueryCursor::new();
    let mut matches = cursor.matches(&query, tree.root_node(), source.as_bytes());

    let mut concepts: Vec<Concept> = Vec::new();
    let mut function_spans: Vec<(String, Range<usize>)> = Vec::new();
    let mut raw_calls: Vec<(Range<usize>, String)> = Vec::new();

    while let Some(m) = matches.next() {
        let mut import_source = None;
        let mut interface_def = None;
        let mut interface_name = None;
        let mut class_def = None;
        let mut class_name = None;
        let mut fn_def = None;
        let mut fn_name = None;
        let mut method_def = None;
        let mut method_name = None;
        let mut call_name = None;

        for cap in m.captures {
            match query.capture_names()[cap.index as usize] {
                "import.source" => import_source = Some(cap.node),
                "interface.def" => interface_def = Some(cap.node),
                "interface.name" => interface_name = Some(cap.node),
                "class.def" => class_def = Some(cap.node),
                "class.name" => class_name = Some(cap.node),
                "fn.def" => fn_def = Some(cap.node),
                "fn.name" => fn_name = Some(cap.node),
                "method.def" => method_def = Some(cap.node),
                "method.name" => method_name = Some(cap.node),
                "call.name" => call_name = Some(cap.node),
                _ => {}
            }
        }

        if let Some(node) = import_source {
            let raw = strip_quotes(node_text(source, node));
            module_concept.relationships.push(import_relationship(&raw));
        }

        if let (Some(def), Some(name)) = (interface_def, interface_name) {
            let name_text = node_text(source, name);
            let qualified = format!("{}.{}", module, name_text);
            concepts.push(make_concept(
                ConceptKind::Interface,
                language,
                name_text,
                &qualified,
                location(relative_path, def),
                Some(signature_before_body(source, def)),
                is_public_by_underscore_convention(name_text),
            ));
        }

        if let (Some(def), Some(name)) = (class_def, class_name) {
            let name_text = node_text(source, name);
            let qualified = format!("{}.{}", module, name_text);
            concepts.push(make_concept(
                ConceptKind::Class,
                language,
                name_text,
                &qualified,
                location(relative_path, def),
                Some(signature_before_body(source, def)),
                is_public_by_underscore_convention(name_text),
            ));
        }

        if let (Some(def), Some(name)) = (fn_def, fn_name) {
            let fn_name_text = node_text(source, name);
            let qualified = format!("{}.{}", module, fn_name_text);
            let concept = make_concept(
                ConceptKind::Function,
                language,
                fn_name_text,
                &qualified,
                location(relative_path, def),
                Some(signature_before_body(source, def)),
                is_public_by_underscore_convention(fn_name_text),
            );
            function_spans.push((concept.id.clone(), def.byte_range()));
            concepts.push(concept);
        }

        if let (Some(def), Some(name)) = (method_def, method_name) {
            // Only a `method_definition` inside a `class_body` is a real
            // class method; the same node shape also matches ES6 shorthand
            // methods in plain object literals (`{ greet() {...} }`), which
            // have no stable, collision-free container to qualify them
            // with, so they're skipped rather than misfiled as a top-level
            // `Function` sharing an id with any same-named real function.
            if let Some(container) = class_container_name(source, def) {
                let method_name_text = node_text(source, name);
                let qualified = format!("{}.{}.{}", module, container, method_name_text);
                let concept = make_concept(
                    ConceptKind::Method,
                    language,
                    method_name_text,
                    &qualified,
                    location(relative_path, def),
                    Some(signature_before_body(source, def)),
                    is_public_by_underscore_convention(method_name_text),
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
