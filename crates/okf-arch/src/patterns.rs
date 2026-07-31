//! Design-pattern detection via structural and naming heuristics over
//! concept data `okf-analyzer` already produces — not a semantic
//! verification that a match really implements the pattern's intent,
//! just a deterministic, explainable structural signal a human reviewing
//! the bundle can accept or dismiss. See each `detect_*` function for
//! exactly what it checks.
//!
//! Every detector here correlates a method to its owning type via
//! [`owner_path`] rather than [`Concept::qualified_name`], since that
//! field's separator isn't stable across a fresh analysis (`.`, what
//! every language extractor emits) and a bundle read back off disk
//! (`::`, `okf_parser::read_bundle`'s best-effort reconstruction, since
//! frontmatter doesn't store `qualified_name` itself) — [`Concept::id`],
//! built once by [`Concept::make_id`], always uses `/` regardless of
//! which path a caller's concepts came from.

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

/// The id path after its kind-directory prefix (e.g. `structs/`,
/// `functions/`) — the qualified path [`Concept::make_id`] encodes,
/// stripped of the kind-specific directory so a type and its own methods
/// (which live under a *different* kind directory: `structs/` vs
/// `functions/`) can be correlated by comparing this instead of the full
/// id.
fn path_after_kind_dir(id: &str) -> &str {
    id.split_once('/').map(|(_, rest)| rest).unwrap_or(id)
}

/// A method's owning type/module path, derived from its own id — the
/// counterpart of a type concept's own [`path_after_kind_dir`], so the
/// two can be compared directly to correlate a method to its declaring
/// type.
fn owner_path(method_id: &str) -> Option<&str> {
    path_after_kind_dir(method_id)
        .rsplit_once('/')
        .map(|(owner, _)| owner)
}

fn is_type_kind(kind: ConceptKind) -> bool {
    matches!(
        kind,
        ConceptKind::Struct | ConceptKind::Class | ConceptKind::Interface | ConceptKind::Trait
    )
}

fn group_methods_by_owner(concepts: &[Concept]) -> HashMap<&str, Vec<&Concept>> {
    let mut map: HashMap<&str, Vec<&Concept>> = HashMap::new();
    for c in concepts {
        if c.kind != ConceptKind::Method {
            continue;
        }
        if let Some(owner) = owner_path(&c.id) {
            map.entry(owner).or_default().push(c);
        }
    }
    map
}

/// Runs every detector below over `concepts` and returns every match,
/// sorted by kind then concept id for deterministic output.
pub fn detect_patterns(concepts: &[Concept]) -> Vec<DetectedPattern> {
    let methods_by_owner = group_methods_by_owner(concepts);
    let types_by_path: HashMap<&str, &Concept> = concepts
        .iter()
        .filter(|c| is_type_kind(c.kind))
        .map(|c| (path_after_kind_dir(&c.id), c))
        .collect();

    let mut found = Vec::new();
    found.extend(detect_builder(concepts, &methods_by_owner));
    found.extend(detect_singleton(concepts, &methods_by_owner));
    found.extend(detect_factory(concepts, &types_by_path));
    found.extend(detect_visitor(concepts, &methods_by_owner));

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

/// A type named `*Builder` (case-insensitive) with a `build` method on
/// it, e.g. `RequestBuilder::build`.
fn detect_builder<'a>(
    concepts: &'a [Concept],
    methods_by_owner: &HashMap<&str, Vec<&'a Concept>>,
) -> Vec<DetectedPattern> {
    concepts
        .iter()
        .filter(|c| is_type_kind(c.kind) && c.name.to_ascii_lowercase().ends_with("builder"))
        .filter_map(|c| {
            let method_name =
                has_method_named(methods_by_owner.get(path_after_kind_dir(&c.id)), &["build"])?;
            Some(DetectedPattern {
                kind: PatternKind::Builder,
                concept_id: c.id.clone(),
                evidence: format!("`{}` (named *Builder) has a `{method_name}` method", c.name),
            })
        })
        .collect()
}

/// A type with an `instance`/`get_instance`/`shared`/`singleton`-named
/// method on it, e.g. `Logger::get_instance`.
fn detect_singleton<'a>(
    concepts: &'a [Concept],
    methods_by_owner: &HashMap<&str, Vec<&'a Concept>>,
) -> Vec<DetectedPattern> {
    const NAMES: &[&str] = &["instance", "get_instance", "getinstance", "shared", "singleton"];
    concepts
        .iter()
        .filter(|c| is_type_kind(c.kind))
        .filter_map(|c| {
            let method_name =
                has_method_named(methods_by_owner.get(path_after_kind_dir(&c.id)), NAMES)?;
            Some(DetectedPattern {
                kind: PatternKind::Singleton,
                concept_id: c.id.clone(),
                evidence: format!("`{}` has a `{method_name}` method", c.name),
            })
        })
        .collect()
}

/// A function/method named `create_*`/`make_*`, or a method on a
/// `*Factory`-named type, e.g. `create_user` or `AuthFactory::build_token`.
fn detect_factory<'a>(
    concepts: &'a [Concept],
    types_by_path: &HashMap<&str, &'a Concept>,
) -> Vec<DetectedPattern> {
    concepts
        .iter()
        .filter(|c| matches!(c.kind, ConceptKind::Function | ConceptKind::Method))
        .filter_map(|c| {
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
                        evidence: format!("`{}` is a method on `{}` (named *Factory)", c.name, owner.name),
                    });
                }
            }
            None
        })
        .collect()
}

/// A type with two or more `visit_*`-named methods on it, e.g. a
/// `NodeVisitor` trait/interface with `visit_binary`/`visit_literal`.
fn detect_visitor<'a>(
    concepts: &'a [Concept],
    methods_by_owner: &HashMap<&str, Vec<&'a Concept>>,
) -> Vec<DetectedPattern> {
    concepts
        .iter()
        .filter(|c| is_type_kind(c.kind))
        .filter_map(|c| {
            let methods = methods_by_owner.get(path_after_kind_dir(&c.id))?;
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
        })
        .collect()
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
