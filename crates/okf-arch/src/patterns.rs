//! Design-pattern detection via structural and naming heuristics over
//! concept data `okf-analyzer` already produces — not a semantic
//! verification that a match really implements the pattern's intent,
//! just a deterministic, explainable structural signal a human reviewing
//! the bundle can accept or dismiss. See each `detect_*` function for
//! exactly what it checks.
//!
//! Every detector here correlates a method to its owning type via
//! [`crate::ownership::owner_path`] rather than
//! [`Concept::qualified_name`] — see that module's own docs for why.

use crate::ownership::{group_methods_by_owner, is_type_kind, owner_path, path_after_kind_dir};
use okf_parser::{Concept, ConceptKind};
use serde::Serialize;
use std::collections::HashMap;

/// A design pattern [`detect_patterns`] knows how to look for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
pub enum PatternKind {
    /// A `*Builder`-named type with a `build` method on it.
    Builder,
    /// A type with an `instance`/`get_instance`/`shared`/`singleton`-named
    /// method on it.
    Singleton,
    /// A function/method named `create_*`/`make_*`, or a method on a
    /// `*Factory`-named type.
    Factory,
    /// A type with two or more `visit_*`-named methods on it.
    Visitor,
}

impl PatternKind {
    /// The pattern's display name, e.g. `"Builder"`.
    pub fn as_str(&self) -> &'static str {
        match self {
            PatternKind::Builder => "Builder",
            PatternKind::Singleton => "Singleton",
            PatternKind::Factory => "Factory",
            PatternKind::Visitor => "Visitor",
        }
    }
}

/// One structural match from [`detect_patterns`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DetectedPattern {
    /// Which pattern matched.
    pub kind: PatternKind,
    /// The concept id most representative of the match: the type for
    /// `Builder`/`Singleton`/`Visitor`, the function/method itself for
    /// `Factory`.
    pub concept_id: String,
    /// A one-line, human-readable justification for the match.
    pub evidence: String,
}

/// Runs every detector below over `concepts` in one pass and returns
/// every match, sorted by kind then concept id for deterministic output.
/// Builder/Singleton/Visitor are all evaluated per type concept (a type
/// can match more than one), Factory per function/method concept — a
/// single pass branching on `c.kind` covers every category without
/// scanning `concepts` once per detector.
pub fn detect_patterns(concepts: &[Concept]) -> Vec<DetectedPattern> {
    let methods_by_owner = group_methods_by_owner(concepts);
    let types_by_path: HashMap<&str, &Concept> = concepts
        .iter()
        .filter(|c| is_type_kind(c.kind))
        .map(|c| (path_after_kind_dir(&c.id), c))
        .collect();

    let mut found = Vec::new();
    for c in concepts {
        if is_type_kind(c.kind) {
            let methods = methods_by_owner.get(path_after_kind_dir(&c.id));
            found.extend(builder_for(c, methods));
            found.extend(singleton_for(c, methods));
            found.extend(visitor_for(c, methods));
        } else if matches!(c.kind, ConceptKind::Function | ConceptKind::Method) {
            found.extend(factory_for(c, &types_by_path));
        }
    }

    found.sort_by(|a, b| (a.kind, &a.concept_id).cmp(&(b.kind, &b.concept_id)));
    found
}

fn has_method_named(methods: Option<&Vec<&Concept>>, names: &[&str]) -> Option<String> {
    let methods = methods?;
    methods
        .iter()
        .find(|m| names.contains(&m.name.to_ascii_lowercase().as_str()))
        .map(|m| m.name.clone())
}

/// Whether `c` is a type named `*Builder` (case-insensitive) with a
/// `build` method on it, e.g. `RequestBuilder::build`. Assumes
/// `is_type_kind(c.kind)`.
fn builder_for(c: &Concept, methods: Option<&Vec<&Concept>>) -> Option<DetectedPattern> {
    if !c.name.to_ascii_lowercase().ends_with("builder") {
        return None;
    }
    let method_name = has_method_named(methods, &["build"])?;
    Some(DetectedPattern {
        kind: PatternKind::Builder,
        concept_id: c.id.clone(),
        evidence: format!("`{}` (named *Builder) has a `{method_name}` method", c.name),
    })
}

const SINGLETON_METHOD_NAMES: &[&str] =
    &["instance", "get_instance", "getinstance", "shared", "singleton"];

/// Whether `c` is a type with an `instance`/`get_instance`/`shared`/
/// `singleton`-named method on it, e.g. `Logger::get_instance`. Assumes
/// `is_type_kind(c.kind)`.
fn singleton_for(c: &Concept, methods: Option<&Vec<&Concept>>) -> Option<DetectedPattern> {
    let method_name = has_method_named(methods, SINGLETON_METHOD_NAMES)?;
    Some(DetectedPattern {
        kind: PatternKind::Singleton,
        concept_id: c.id.clone(),
        evidence: format!("`{}` has a `{method_name}` method", c.name),
    })
}

/// Whether `c` is a function/method named `create_*`/`make_*`, or a
/// method on a `*Factory`-named type, e.g. `create_user` or
/// `AuthFactory::build_token`. Assumes `c.kind` is `Function` or
/// `Method`.
fn factory_for(c: &Concept, types_by_path: &HashMap<&str, &Concept>) -> Option<DetectedPattern> {
    let lower = c.name.to_ascii_lowercase();
    if lower.starts_with("create_") || lower.starts_with("make_") {
        return Some(DetectedPattern {
            kind: PatternKind::Factory,
            concept_id: c.id.clone(),
            evidence: format!("`{}` is named create_*/make_*", c.name),
        });
    }
    if c.kind == ConceptKind::Method {
        let owner = types_by_path.get(owner_path(&c.id)?)?;
        if owner.name.to_ascii_lowercase().ends_with("factory") {
            return Some(DetectedPattern {
                kind: PatternKind::Factory,
                concept_id: c.id.clone(),
                evidence: format!(
                    "`{}` is a method on `{}` (named *Factory)",
                    c.name, owner.name
                ),
            });
        }
    }
    None
}

/// Whether `c` is a type with two or more `visit_*`-named methods on it,
/// e.g. a `NodeVisitor` trait/interface with `visit_binary`/
/// `visit_literal`. Assumes `is_type_kind(c.kind)`.
fn visitor_for(c: &Concept, methods: Option<&Vec<&Concept>>) -> Option<DetectedPattern> {
    let methods = methods?;
    let visit_methods: Vec<&str> = methods
        .iter()
        .filter(|m| m.name.to_ascii_lowercase().starts_with("visit"))
        .map(|m| m.name.as_str())
        .collect();
    if visit_methods.len() < 2 {
        return None;
    }
    Some(DetectedPattern {
        kind: PatternKind::Visitor,
        concept_id: c.id.clone(),
        evidence: format!(
            "`{}` has {} visit_*-named methods ({})",
            c.name,
            visit_methods.len(),
            visit_methods.join(", ")
        ),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use okf_parser::{Language, Location};

    /// Builds a type concept whose id is `<kind's own bundle_dir>/<path>`
    /// — `path` deliberately carries *no* kind-dir prefix of its own, the
    /// same way a real qualified path never does; only `Concept::make_id`
    /// prepends one, once, per concept. [`method`] below builds a
    /// method's id the same way, so the two only ever share `path` itself
    /// (e.g. `"RequestBuilder"`), matching how [`super::owner_path`]
    /// expects to correlate them.
    fn type_concept(path: &str, kind: ConceptKind) -> Concept {
        let id = format!("{}/{path}", kind.bundle_dir());
        Concept {
            id,
            kind,
            language: Language::Rust,
            name: path.rsplit('/').next().unwrap().to_string(),
            qualified_name: path.replace('/', "."),
            description: None,
            location: Location {
                file: "src/lib.rs".to_string(),
                start_line: 1,
                end_line: 1,
            },
            signature: None,
            tags: Vec::new(),
            is_public: true,
            generated_at: None,
            relationships: Vec::new(),
        }
    }

    /// A method whose owner is the type built by `type_concept(owner_path, _)`.
    fn method(owner_path: &str, name: &str) -> Concept {
        Concept {
            name: name.to_string(),
            ..type_concept(&format!("{owner_path}/{name}"), ConceptKind::Method)
        }
    }

    #[test]
    fn detects_a_builder() {
        let builder = type_concept("RequestBuilder", ConceptKind::Struct);
        let build_method = method("RequestBuilder", "build");
        let unrelated = type_concept("Plain", ConceptKind::Struct);

        let concepts = vec![builder, build_method, unrelated];
        let found = detect_patterns(&concepts);

        assert_eq!(found.len(), 1);
        assert_eq!(found[0].kind, PatternKind::Builder);
        assert_eq!(found[0].concept_id, "classes/RequestBuilder");
    }

    #[test]
    fn a_builder_named_type_without_a_build_method_is_not_flagged() {
        let builder = type_concept("RequestBuilder", ConceptKind::Struct);
        let other_method = method("RequestBuilder", "with_header");

        let concepts = vec![builder, other_method];
        assert!(detect_patterns(&concepts).is_empty());
    }

    #[test]
    fn detects_a_singleton_by_get_instance() {
        let logger = type_concept("Logger", ConceptKind::Class);
        let accessor = method("Logger", "get_instance");

        let concepts = vec![logger, accessor];
        let found = detect_patterns(&concepts);

        assert_eq!(found.len(), 1);
        assert_eq!(found[0].kind, PatternKind::Singleton);
        assert_eq!(found[0].concept_id, "classes/Logger");
    }

    #[test]
    fn detects_a_factory_by_name_prefix_and_by_owning_type_suffix() {
        let free_fn = type_concept("create_user", ConceptKind::Function);
        let factory_type = type_concept("AuthFactory", ConceptKind::Class);
        let factory_method = method("AuthFactory", "build_token");

        let concepts = vec![free_fn, factory_type, factory_method];
        let found = detect_patterns(&concepts);

        assert_eq!(found.len(), 2);
        assert!(found.iter().all(|p| p.kind == PatternKind::Factory));
        let ids: Vec<&str> = found.iter().map(|p| p.concept_id.as_str()).collect();
        assert!(ids.contains(&"functions/create_user"));
        assert!(ids.contains(&"functions/AuthFactory/build_token"));
    }

    #[test]
    fn a_new_prefixed_constructor_is_not_flagged_as_a_factory() {
        // `new` is an idiomatic Rust constructor name, not a distinguishing
        // Factory-pattern signal on its own -- only create_*/make_* are.
        let ctor = method("Widget", "new");
        assert!(detect_patterns(&[ctor]).is_empty());
    }

    #[test]
    fn detects_a_visitor_by_two_or_more_visit_methods() {
        let visitor = type_concept("NodeVisitor", ConceptKind::Interface);
        let visit_a = method("NodeVisitor", "visit_binary");
        let visit_b = method("NodeVisitor", "visit_literal");

        let concepts = vec![visitor, visit_a, visit_b];
        let found = detect_patterns(&concepts);

        assert_eq!(found.len(), 1);
        assert_eq!(found[0].kind, PatternKind::Visitor);
        assert_eq!(found[0].concept_id, "interfaces/NodeVisitor");
    }

    #[test]
    fn a_single_visit_method_is_not_enough_to_flag_a_visitor() {
        let visitor = type_concept("NodeVisitor", ConceptKind::Interface);
        let visit_a = method("NodeVisitor", "visit_binary");

        let concepts = vec![visitor, visit_a];
        assert!(detect_patterns(&concepts).is_empty());
    }

    #[test]
    fn results_are_sorted_by_kind_then_id() {
        let builder = type_concept("FooBuilder", ConceptKind::Struct);
        let build_method = method("FooBuilder", "build");
        let logger = type_concept("Logger", ConceptKind::Class);
        let accessor = method("Logger", "instance");

        let concepts = vec![builder, build_method, logger, accessor];
        let found = detect_patterns(&concepts);

        assert_eq!(found.len(), 2);
        assert_eq!(found[0].kind, PatternKind::Builder);
        assert_eq!(found[1].kind, PatternKind::Singleton);
    }
}
