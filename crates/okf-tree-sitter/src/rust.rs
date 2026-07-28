use crate::common::{
    import_relationship, location, make_concept, module_path, node_text, slugify,
    smallest_containing,
};
use crate::{CallCandidate, FileExtraction};
use anyhow::{Context, Result};
use okf_parser::{Concept, ConceptKind, Language};
use std::ops::Range;
use tree_sitter::{Node, Parser, Query, QueryCursor, StreamingIterator};

const QUERY_SRC: &str = r#"
(use_declaration) @import
(struct_item (visibility_modifier)? @struct.vis name: (type_identifier) @struct.name) @struct.def
(enum_item (visibility_modifier)? @enum.vis name: (type_identifier) @enum.name) @enum.def
(trait_item (visibility_modifier)? @trait.vis name: (type_identifier) @trait.name) @trait.def
(function_item (visibility_modifier)? @fn.vis name: (identifier) @fn.name) @fn.def
(call_expression function: (identifier) @call.name) @call.def
(call_expression function: (field_expression field: (field_identifier) @call.name)) @call.def
(call_expression function: (scoped_identifier name: (identifier) @call.name)) @call.def
"#;

/// The container a method belongs to: the `Self` type, plus the trait
/// being implemented if this is a trait impl (as opposed to an inherent
/// impl or a default trait-method body).
struct Container<'a> {
    type_name: &'a str,
    /// Raw trait reference text (e.g. `From<i32>`), present only for
    /// `impl Trait for Type` blocks. Distinguishing this in the qualified
    /// name is what lets `impl From<i32> for Wrapper` and
    /// `impl From<String> for Wrapper` produce different concept ids for
    /// their `from` methods instead of colliding.
    trait_name: Option<&'a str>,
}

/// Finds the containing `impl`/`trait` block of a `function_item`, if any,
/// treating the function as a method rather than a top-level function.
fn container_name<'a>(src: &'a str, function_node: Node) -> Option<Container<'a>> {
    let parent = function_node.parent()?;
    if parent.kind() != "declaration_list" {
        return None;
    }
    let grand = parent.parent()?;
    match grand.kind() {
        "impl_item" => {
            let type_field = grand.child_by_field_name("type")?;
            let type_text = node_text(src, type_field);
            let type_name = type_text.split('<').next().unwrap_or(type_text).trim();
            let trait_name = grand
                .child_by_field_name("trait")
                .map(|t| node_text(src, t));
            Some(Container {
                type_name,
                trait_name,
            })
        }
        "trait_item" => {
            let name_field = grand.child_by_field_name("name")?;
            Some(Container {
                type_name: node_text(src, name_field),
                trait_name: None,
            })
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
        true,
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
        let mut struct_vis = None;
        let mut enum_def = None;
        let mut enum_name = None;
        let mut enum_vis = None;
        let mut trait_def = None;
        let mut trait_name = None;
        let mut trait_vis = None;
        let mut fn_def = None;
        let mut fn_name = None;
        let mut fn_vis = None;
        let mut call_name = None;

        for cap in m.captures {
            let cap_name = query.capture_names()[cap.index as usize];
            match cap_name {
                "import" => import_node = Some(cap.node),
                "struct.def" => struct_def = Some(cap.node),
                "struct.name" => struct_name = Some(cap.node),
                "struct.vis" => struct_vis = Some(cap.node),
                "enum.def" => enum_def = Some(cap.node),
                "enum.name" => enum_name = Some(cap.node),
                "enum.vis" => enum_vis = Some(cap.node),
                "trait.def" => trait_def = Some(cap.node),
                "trait.name" => trait_name = Some(cap.node),
                "trait.vis" => trait_vis = Some(cap.node),
                "fn.def" => fn_def = Some(cap.node),
                "fn.name" => fn_name = Some(cap.node),
                "fn.vis" => fn_vis = Some(cap.node),
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
                struct_vis.is_some(),
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
                enum_vis.is_some(),
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
                trait_vis.is_some(),
            ));
        }

        if let (Some(def), Some(name)) = (fn_def, fn_name) {
            let fn_name_text = node_text(source, name);
            let (kind, qualified) = match container_name(source, def) {
                Some(Container {
                    type_name,
                    trait_name: Some(trait_name),
                }) => (
                    ConceptKind::Method,
                    format!(
                        "{}.{}.{}.{}",
                        module,
                        type_name,
                        slugify(trait_name),
                        fn_name_text
                    ),
                ),
                Some(Container {
                    type_name,
                    trait_name: None,
                }) => (
                    ConceptKind::Method,
                    format!("{}.{}.{}", module, type_name, fn_name_text),
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
                fn_vis.is_some(),
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

    #[test]
    fn captures_method_and_scoped_calls() {
        let src = r#"
struct Foo;
impl Foo {
    fn helper(&self) {
        self.other();
        Foo::other(self);
        std::mem::drop(1);
    }
    fn other(&self) {}
}
"#;
        let extraction = extract(src, "src/lib.rs").unwrap();
        let callee_names: Vec<_> = extraction
            .calls
            .iter()
            .map(|c| c.callee_name.as_str())
            .collect();
        assert!(
            callee_names.contains(&"other"),
            "expected self.other()/Foo::other() to be captured, got {callee_names:?}"
        );
        assert!(
            callee_names.contains(&"drop"),
            "expected std::mem::drop() to be captured, got {callee_names:?}"
        );
    }

    #[test]
    fn distinguishes_generic_trait_impls_with_the_same_method_name() {
        let src = r#"
struct Wrapper(String);

impl From<i32> for Wrapper {
    fn from(v: i32) -> Self { Wrapper(v.to_string()) }
}

impl From<String> for Wrapper {
    fn from(v: String) -> Self { Wrapper(v) }
}
"#;
        let extraction = extract(src, "src/wrapper.rs").unwrap();
        let from_methods: Vec<_> = extraction
            .concepts
            .iter()
            .filter(|c| c.name == "from")
            .collect();
        assert_eq!(
            from_methods.len(),
            2,
            "both `from` impls should be extracted"
        );

        let ids: std::collections::HashSet<_> = from_methods.iter().map(|c| &c.id).collect();
        assert_eq!(
            ids.len(),
            2,
            "the two `from` methods must not collide on id"
        );
    }

    #[test]
    fn detects_pub_visibility() {
        let src = r#"
pub struct Public;
struct Private;

pub fn public_fn() {}
fn private_fn() {}

impl Public {
    pub fn pub_method(&self) {}
    fn priv_method(&self) {}
}
"#;
        let extraction = extract(src, "src/lib.rs").unwrap();
        let find = |name: &str| extraction.concepts.iter().find(|c| c.name == name).unwrap();

        assert!(find("Public").is_public);
        assert!(!find("Private").is_public);
        assert!(find("public_fn").is_public);
        assert!(!find("private_fn").is_public);
        assert!(find("pub_method").is_public);
        assert!(!find("priv_method").is_public);
    }
}
