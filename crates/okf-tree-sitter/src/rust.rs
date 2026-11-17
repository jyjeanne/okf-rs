use crate::common::{
    import_relationship, location, lsp_position, make_concept, module_concept, module_path,
    node_text, signature_before_body, slugify, smallest_containing,
};
use crate::rust_types::{normalize_type, resolve_type_text, self_type, HintContext};
use crate::{
    AliasFact, CallCandidate, CallSite, FieldFact, FileExtraction, FunctionFact, MethodFact,
    TypeFacts, TypeHint,
};
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
(function_signature_item name: (identifier) @sig.name) @sig.def
(field_declaration name: (field_identifier) @field.name type: (_) @field.type) @field.def
(type_item name: (type_identifier) @alias.name type: (_) @alias.type) @alias.def
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

/// Tag given to Rust functions and methods that are test code (see
/// [`is_test_code`]). Consumers such as `okf_query::plan_change` read it
/// to tell tests apart from the code they exercise — inline unit tests
/// otherwise look like any private function in the bundle.
pub const TEST_TAG: &str = "test";

/// The `#[...]` attributes written directly before `node`, nearest first.
/// In tree-sitter-rust, outer attributes are `attribute_item` siblings
/// preceding the item, possibly with doc or line comments between them.
fn preceding_attributes(node: Node) -> Vec<Node> {
    let mut attributes = Vec::new();
    let mut current = node.prev_sibling();
    while let Some(sibling) = current {
        match sibling.kind() {
            "attribute_item" => attributes.push(sibling),
            "line_comment" | "block_comment" => {}
            _ => break,
        }
        current = sibling.prev_sibling();
    }
    attributes
}

/// An attribute's text with all whitespace removed, e.g. `#[tokio::test]`.
fn compact_attribute(src: &str, attribute: Node) -> String {
    node_text(src, attribute)
        .chars()
        .filter(|c| !c.is_whitespace())
        .collect()
}

/// `#[test]`, any attribute whose path ends in `::test` (`#[tokio::test]`,
/// `#[async_std::test]`, `#[sqlx::test]`, ...), `#[rstest]`, or
/// `#[test_case(...)]`. Arguments are ignored.
fn is_test_attribute(compact: &str) -> bool {
    let inner = compact
        .strip_prefix("#[")
        .and_then(|s| s.strip_suffix(']'))
        .unwrap_or(compact);
    let path = inner.split('(').next().unwrap_or(inner);
    let last = path.rsplit("::").next().unwrap_or(path);
    matches!(last, "test" | "rstest" | "test_case")
}

/// Whether `function_node` is test code: it carries a test attribute (see
/// [`is_test_attribute`]), or it is declared, at any depth, inside a module
/// marked exactly `#[cfg(test)]`. Compound conditions (`cfg(all(test, ..))`)
/// and a test module kept in its own file (`#[cfg(test)] mod tests;`) are
/// not recognized: the attribute isn't visible from the file being parsed.
fn is_test_code(src: &str, function_node: Node) -> bool {
    let tested = preceding_attributes(function_node)
        .into_iter()
        .any(|a| is_test_attribute(&compact_attribute(src, a)));
    if tested {
        return true;
    }
    let mut ancestor = function_node.parent();
    while let Some(node) = ancestor {
        if node.kind() == "mod_item"
            && preceding_attributes(node)
                .into_iter()
                .any(|a| compact_attribute(src, a) == "#[cfg(test)]")
        {
            return true;
        }
        ancestor = node.parent();
    }
    false
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
    let mut module_concept = module_concept(Language::Rust, relative_path, source);

    let query = Query::new(&ts_lang, QUERY_SRC).context("invalid Rust query")?;
    let mut cursor = QueryCursor::new();
    let mut matches = cursor.matches(&query, tree.root_node(), source.as_bytes());

    let mut concepts: Vec<Concept> = Vec::new();
    let mut function_spans: Vec<(String, Range<usize>)> = Vec::new();
    let mut raw_calls: Vec<(Range<usize>, String, CallSite, Option<TypeHint>)> = Vec::new();
    let mut types = TypeFacts::default();
    let mut hints = HintContext::new(source);

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
        let mut sig_def = None;
        let mut sig_name = None;
        let mut field_def = None;
        let mut field_name = None;
        let mut field_type = None;
        let mut alias_def = None;
        let mut alias_name = None;
        let mut alias_type = None;

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
                "sig.def" => sig_def = Some(cap.node),
                "sig.name" => sig_name = Some(cap.node),
                "field.def" => field_def = Some(cap.node),
                "field.name" => field_name = Some(cap.node),
                "field.type" => field_type = Some(cap.node),
                "alias.def" => alias_def = Some(cap.node),
                "alias.name" => alias_name = Some(cap.node),
                "alias.type" => alias_type = Some(cap.node),
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
            let container = container_name(source, def);
            let (kind, qualified) = match &container {
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
            let mut concept = make_concept(
                kind,
                Language::Rust,
                fn_name_text,
                &qualified,
                location(relative_path, def),
                Some(signature_before_body(source, def)),
                fn_vis.is_some(),
            );
            if is_test_code(source, def) {
                concept.tags.push(TEST_TAG.to_string());
            }
            let returns = def
                .child_by_field_name("return_type")
                .and_then(|r| resolve_type_text(source, def, node_text(source, r)));
            match &container {
                Some(c) => {
                    if let Some(owner) = normalize_type(c.type_name) {
                        types.methods.push(MethodFact {
                            concept_id: Some(concept.id.clone()),
                            owner,
                            trait_name: c.trait_name.and_then(normalize_type),
                            name: fn_name_text.to_string(),
                            returns,
                        });
                    }
                }
                None => types.functions.push(FunctionFact {
                    name: fn_name_text.to_string(),
                    returns,
                }),
            }
            function_spans.push((concept.id.clone(), def.byte_range()));
            concepts.push(concept);
        }

        // A trait method declared without a body: no concept, but its
        // return type is what a call on a `dyn Trait`/`impl Trait`
        // receiver evaluates to.
        if let (Some(def), Some(name)) = (sig_def, sig_name) {
            if let Some(owner) = self_type(source, def) {
                types.methods.push(MethodFact {
                    concept_id: None,
                    owner,
                    trait_name: None,
                    name: node_text(source, name).to_string(),
                    returns: def
                        .child_by_field_name("return_type")
                        .and_then(|r| resolve_type_text(source, def, node_text(source, r))),
                });
            }
        }

        if let (Some(def), Some(name), Some(ty)) = (field_def, field_name, field_type) {
            let owner = def
                .parent()
                .and_then(|list| list.parent())
                .filter(|s| s.kind() == "struct_item")
                .and_then(|s| s.child_by_field_name("name"));
            if let (Some(owner), Some(ty)) =
                (owner, resolve_type_text(source, def, node_text(source, ty)))
            {
                types.fields.push(FieldFact {
                    owner: node_text(source, owner).to_string(),
                    field: node_text(source, name).to_string(),
                    ty,
                });
            }
        }

        // An associated type (`type Item = ...;` inside an impl or trait)
        // is not a project-wide alias.
        if let (Some(def), Some(name), Some(ty)) = (alias_def, alias_name, alias_type) {
            if def.parent().is_none_or(|p| p.kind() != "declaration_list") {
                types.aliases.push(AliasFact {
                    name: node_text(source, name).to_string(),
                    target: resolve_type_text(source, def, node_text(source, ty)),
                });
            }
        }

        if let Some(node) = call_name {
            raw_calls.push((
                node.byte_range(),
                node_text(source, node).to_string(),
                lsp_position(source, node),
                hints.receiver_hint(node),
            ));
        }
    }

    let mut calls = Vec::new();
    for (range, callee_name, call_site, receiver) in raw_calls {
        if let Some(caller_id) = smallest_containing(&function_spans, range.start) {
            calls.push(CallCandidate {
                caller_id: caller_id.to_string(),
                callee_name,
                call_site,
                receiver,
            });
        }
    }

    let mut all = vec![module_concept];
    all.extend(concepts);
    Ok(FileExtraction {
        concepts: all,
        calls,
        types,
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

    #[test]
    fn tags_test_functions_and_everything_inside_cfg_test_modules() {
        let src = r#"
pub fn production() {}

#[inline]
fn not_a_test() {}

#[test]
fn plain_test() {}

#[tokio::test(flavor = "multi_thread")]
async fn async_test() {}

#[rstest]
fn parameterized() {}

#[test]
/// A doc comment between the attribute and the function.
fn documented_test() {}

#[cfg(test)]
mod tests {
    fn helper() {}

    struct Fixture;
    impl Fixture {
        fn build() -> Self { Fixture }
    }

    mod nested {
        fn deep_helper() {}
    }
}

#[cfg(not(test))]
mod real {
    fn only_in_production() {}
}
"#;
        let extraction = extract(src, "src/lib.rs").unwrap();
        let tagged = |name: &str| {
            extraction
                .concepts
                .iter()
                .find(|c| c.name == name)
                .unwrap_or_else(|| panic!("no concept named {name}"))
                .tags
                .contains(&TEST_TAG.to_string())
        };

        for name in [
            "plain_test",
            "async_test",
            "parameterized",
            "documented_test",
            "helper",
            "build",
            "deep_helper",
        ] {
            assert!(tagged(name), "{name} should be tagged as test code");
        }
        for name in ["production", "not_a_test", "only_in_production"] {
            assert!(!tagged(name), "{name} should not be tagged");
        }
        // Types are never tagged, even inside a test module.
        assert!(!tagged("Fixture"));
    }

    #[test]
    fn test_attribute_paths() {
        for yes in [
            "#[test]",
            "#[tokio::test]",
            "#[tokio::test(flavor=\"multi_thread\")]",
            "#[async_std::test]",
            "#[rstest]",
            "#[test_case(1,2)]",
        ] {
            assert!(is_test_attribute(yes), "{yes}");
        }
        for no in [
            "#[inline]",
            "#[cfg(test)]",
            "#[testing]",
            "#[derive(Debug)]",
        ] {
            assert!(!is_test_attribute(no), "{no}");
        }
    }

    /// The receiver hint recorded for the call to `callee` inside the
    /// function or method named `caller`.
    fn hint_for(extraction: &FileExtraction, caller: &str, callee: &str) -> Option<TypeHint> {
        extraction
            .calls
            .iter()
            .find(|c| c.caller_id.ends_with(&format!("/{caller}")) && c.callee_name == callee)
            .unwrap_or_else(|| panic!("no call {caller} -> {callee}"))
            .receiver
            .clone()
    }

    fn named(name: &str) -> TypeHint {
        TypeHint::Named(name.to_string())
    }

    #[test]
    fn receiver_hints_follow_parameters_lets_fields_and_chains() {
        let src = r#"
struct Holder { store: Arc<Store> }
impl Holder {
    fn store(&self) -> &Arc<Store> { &self.store }
    fn via_field(&self) { self.store.concurrency(); }
    fn via_method(&self) { self.store().concurrency(); }
    fn via_self_path() -> Self { Self::new() }
}
fn param(store: &Arc<Store>) { store.concurrency(); }
fn typed_let() { let s: Store = make(); s.concurrency(); }
fn ctor() -> Result<()> { let mut f = GitFetch::new(1)?; f.fetch(); Ok(()) }
fn chained(repo: &dyn Repo) { repo.store().concurrency(); }
fn literal() { Store { x: 1 }.concurrency(); }
fn shadowed(x: Old) { let x = New::default(); x.go(); }
fn closure_param() { let f = |store| store.concurrency(); }
fn generic<R: Repo>(repo: &R) { repo.store(); }
fn unbounded<T>(t: T) { t.store(); }
fn module_path() { git::push_updates(); }
fn assoc(item: Self::Item) { item.go(); }
"#;
        let x = extract(src, "src/lib.rs").unwrap();
        assert_eq!(
            hint_for(&x, "via_field", "concurrency"),
            Some(TypeHint::Field {
                base: Box::new(named("Holder")),
                field: "store".into()
            })
        );
        assert_eq!(
            hint_for(&x, "via_method", "concurrency"),
            Some(TypeHint::MethodReturn {
                base: Box::new(named("Holder")),
                method: "store".into()
            })
        );
        assert_eq!(hint_for(&x, "via_self_path", "new"), Some(named("Holder")));
        assert_eq!(hint_for(&x, "param", "concurrency"), Some(named("Store")));
        assert_eq!(
            hint_for(&x, "typed_let", "concurrency"),
            Some(named("Store"))
        );
        assert_eq!(
            hint_for(&x, "ctor", "fetch"),
            Some(TypeHint::FnReturn {
                owner: Some("GitFetch".into()),
                function: "new".into()
            })
        );
        assert_eq!(
            hint_for(&x, "chained", "concurrency"),
            Some(TypeHint::MethodReturn {
                base: Box::new(named("Repo")),
                method: "store".into()
            })
        );
        assert_eq!(hint_for(&x, "literal", "concurrency"), Some(named("Store")));
        // The nearest preceding `let` wins over the parameter it shadows.
        assert_eq!(
            hint_for(&x, "shadowed", "go"),
            Some(TypeHint::FnReturn {
                owner: Some("New".into()),
                function: "default".into()
            })
        );
        // Unknowable receivers get no hint rather than a guess.
        assert_eq!(hint_for(&x, "closure_param", "concurrency"), None);
        assert_eq!(hint_for(&x, "generic", "store"), Some(named("Repo")));
        assert_eq!(hint_for(&x, "unbounded", "store"), None);
        assert_eq!(hint_for(&x, "module_path", "push_updates"), None);
        assert_eq!(hint_for(&x, "assoc", "go"), None);
        // A plain function call has no receiver at all.
        assert_eq!(hint_for(&x, "typed_let", "make"), None);
    }

    #[test]
    fn records_fields_methods_trait_signatures_and_aliases_as_type_facts() {
        let src = r#"
type Shared = Arc<Store>;
struct Tree { store: Arc<Store>, name: String }
trait Repo {
    type Item;
    fn store(&self) -> &Arc<Store>;
    fn base(&self) -> Self { todo!() }
}
impl Tree {
    fn new() -> Result<Self, Error> { todo!() }
}
impl Repo for Tree {
    type Item = u8;
    fn store(&self) -> &Arc<Store> { &self.store }
}
fn make() -> Option<Box<Store>> { None }
"#;
        let x = extract(src, "src/lib.rs").unwrap();
        let t = &x.types;
        assert!(t.fields.contains(&FieldFact {
            owner: "Tree".into(),
            field: "store".into(),
            ty: "Store".into()
        }));
        let method = |owner: &str, name: &str| {
            t.methods
                .iter()
                .find(|m| m.owner == owner && m.name == name)
                .unwrap_or_else(|| panic!("no fact for {owner}::{name}"))
        };
        assert_eq!(method("Tree", "new").returns.as_deref(), Some("Tree"));
        assert_eq!(method("Tree", "store").trait_name.as_deref(), Some("Repo"));
        assert!(method("Tree", "store").concept_id.is_some());
        // A bodiless trait signature: return type only, no concept.
        let signature = method("Repo", "store");
        assert_eq!(signature.returns.as_deref(), Some("Store"));
        assert!(signature.concept_id.is_none());
        // `Self` in a trait default method is the trait.
        assert_eq!(method("Repo", "base").returns.as_deref(), Some("Repo"));
        assert_eq!(
            t.functions,
            vec![FunctionFact {
                name: "make".into(),
                returns: Some("Store".into())
            }]
        );
        // Associated types are not project-wide aliases.
        assert_eq!(
            t.aliases,
            vec![AliasFact {
                name: "Shared".into(),
                target: Some("Store".into())
            }]
        );
    }
}
