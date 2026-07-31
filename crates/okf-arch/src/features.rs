//! REST-endpoint, database-model, and event-flow detection via
//! structural and naming heuristics — the same deterministic, explain-
//! don't-guess approach [`crate::patterns`] uses for design patterns,
//! applied to a different category of structural signal.
//!
//! **Known limitation:** every detector here works from a concept's
//! name/kind alone, since decorator/annotation/attribute source text
//! (e.g. Spring's `@RestController`/`@GetMapping`, Flask's
//! `@app.route(...)`, JPA's `@Entity`) isn't captured anywhere in the
//! `Concept` model today — extracting that would need a schema change
//! and per-language extractor work well beyond this pass. What's here
//! instead leans on naming conventions common enough to be worth
//! surfacing on their own: a `*Controller`-named type's public methods
//! (REST endpoints), a `*Model`/`*Entity`/`*Record`-named type (database
//! models), and `emit_*`/`publish_*`/`subscribe_*`/`on_*`/`*_handler`/
//! `*_event`-named functions and methods (event-flow participants). A
//! codebase that doesn't follow these naming conventions — or that
//! relies on decorators/attributes alone to mark a REST route or ORM
//! entity — won't show up here; that's a real gap, not a subtle one.

use crate::ownership::{group_methods_by_owner, is_type_kind, path_after_kind_dir};
use okf_parser::{Concept, ConceptKind};
use serde::Serialize;
use std::collections::HashMap;

/// A category [`detect_features`] knows how to look for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
pub enum FeatureKind {
    /// A public method on a `*Controller`-named type.
    RestEndpoint,
    /// A `*Model`/`*Entity`/`*Record`-named type.
    DatabaseModel,
    /// An `emit_*`/`publish_*`/`subscribe_*`/`on_*`/`*_handler`/
    /// `*_event`-named function or method.
    EventFlow,
}

impl FeatureKind {
    /// The category's display name, e.g. `"RestEndpoint"`.
    pub fn as_str(&self) -> &'static str {
        match self {
            FeatureKind::RestEndpoint => "RestEndpoint",
            FeatureKind::DatabaseModel => "DatabaseModel",
            FeatureKind::EventFlow => "EventFlow",
        }
    }
}

/// One structural match from [`detect_features`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DetectedFeature {
    /// Which category matched.
    pub kind: FeatureKind,
    /// The concept id representing the match: the method for
    /// `RestEndpoint`, the type for `DatabaseModel`, the function/method
    /// for `EventFlow`.
    pub concept_id: String,
    /// A one-line, human-readable justification for the match.
    pub evidence: String,
}

/// Runs every detector below over `concepts` in one pass and returns
/// every match, sorted by kind then concept id for deterministic output.
/// A type concept and a function/method concept are never both eligible
/// for the same detector, so a single pass branching on `c.kind` covers
/// every category without scanning `concepts` once per detector.
pub fn detect_features(concepts: &[Concept]) -> Vec<DetectedFeature> {
    let methods_by_owner = group_methods_by_owner(concepts);

    let mut found = Vec::new();
    for c in concepts {
        if is_type_kind(c.kind) {
            if c.name.ends_with("Controller") {
                found.extend(rest_endpoints_for_controller(c, &methods_by_owner));
            }
            found.extend(database_model_for(c));
        } else if matches!(c.kind, ConceptKind::Function | ConceptKind::Method) {
            found.extend(event_flow_for(c));
        }
    }

    found.sort_by(|a, b| (a.kind, &a.concept_id).cmp(&(b.kind, &b.concept_id)));
    found
}

/// Every public method on `controller` (ASP.NET MVC/Web API, Spring MVC,
/// NestJS naming convention) — one finding per method, since the roadmap
/// asks for endpoint-level detection, not just flagging the controller
/// type itself. Assumes `controller.name` already ends with
/// `"Controller"`.
fn rest_endpoints_for_controller(
    controller: &Concept,
    methods_by_owner: &HashMap<&str, Vec<&Concept>>,
) -> Vec<DetectedFeature> {
    methods_by_owner
        .get(path_after_kind_dir(&controller.id))
        .into_iter()
        .flatten()
        .filter(|m| m.is_public)
        .map(|m| DetectedFeature {
            kind: FeatureKind::RestEndpoint,
            concept_id: m.id.clone(),
            evidence: format!(
                "public method `{}` on `{}` (named *Controller)",
                m.name, controller.name
            ),
        })
        .collect()
}

const MODEL_SUFFIXES: &[&str] = &["Model", "Entity", "Record"];

/// Whether `c` is a type named `*Model`/`*Entity`/`*Record` — common
/// ORM/database-model naming conventions (Django/Rails-adjacent "Model",
/// JPA-adjacent "Entity", plain-data-object "Record"). Assumes
/// `is_type_kind(c.kind)`.
fn database_model_for(c: &Concept) -> Option<DetectedFeature> {
    let suffix = MODEL_SUFFIXES.iter().find(|s| c.name.ends_with(**s))?;
    Some(DetectedFeature {
        kind: FeatureKind::DatabaseModel,
        concept_id: c.id.clone(),
        evidence: format!(
            "`{}` is named *{suffix} (a common ORM/database-model naming convention)",
            c.name
        ),
    })
}

/// Whether `c` is a function/method named `emit_*`/`publish_*` (produces
/// an event), `subscribe_*`/`on_*` (reacts to one), or `*_handler`/
/// `*_event` (either role, less specifically named).
fn event_flow_for(c: &Concept) -> Option<DetectedFeature> {
    let lower = c.name.to_ascii_lowercase();
    let role = if lower.starts_with("emit_") || lower.starts_with("publish_") {
        "emits"
    } else if lower.starts_with("subscribe_") || lower.starts_with("on_") {
        "subscribes to"
    } else if lower.ends_with("_handler") || lower.ends_with("_event") {
        "handles"
    } else {
        return None;
    };
    Some(DetectedFeature {
        kind: FeatureKind::EventFlow,
        concept_id: c.id.clone(),
        evidence: format!("`{}` {role} an event, by naming convention", c.name),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use okf_parser::{Language, Location};

    fn type_concept(path: &str, kind: ConceptKind, is_public: bool) -> Concept {
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
            is_public,
            generated_at: None,
            relationships: Vec::new(),
        }
    }

    fn method(owner_path: &str, name: &str, is_public: bool) -> Concept {
        Concept {
            name: name.to_string(),
            ..type_concept(&format!("{owner_path}/{name}"), ConceptKind::Method, is_public)
        }
    }

    #[test]
    fn detects_a_rest_endpoint_via_public_method_on_a_controller() {
        let controller = type_concept("UserController", ConceptKind::Class, true);
        let public_action = method("UserController", "get_user", true);
        let private_helper = method("UserController", "format_response", false);

        let concepts = vec![controller, public_action, private_helper];
        let found = detect_features(&concepts);

        assert_eq!(found.len(), 1);
        assert_eq!(found[0].kind, FeatureKind::RestEndpoint);
        assert_eq!(found[0].concept_id, "functions/UserController/get_user");
    }

    #[test]
    fn a_non_controller_types_methods_are_not_flagged_as_endpoints() {
        let plain = type_concept("UserService", ConceptKind::Class, true);
        let public_method = method("UserService", "get_user", true);
        let concepts = vec![plain, public_method];
        assert!(detect_features(&concepts).is_empty());
    }

    #[test]
    fn detects_database_models_by_suffix() {
        let model = type_concept("User", ConceptKind::Struct, true); // no suffix match
        let user_model = type_concept("UserModel", ConceptKind::Struct, true);
        let user_entity = type_concept("UserEntity", ConceptKind::Class, true);
        let user_record = type_concept("UserRecord", ConceptKind::Struct, true);

        let concepts = vec![model, user_model, user_entity, user_record];
        let found = detect_features(&concepts);

        assert_eq!(found.len(), 3);
        assert!(found.iter().all(|f| f.kind == FeatureKind::DatabaseModel));
        let ids: Vec<&str> = found.iter().map(|f| f.concept_id.as_str()).collect();
        assert!(ids.contains(&"classes/UserModel"));
        assert!(ids.contains(&"classes/UserEntity"));
        assert!(ids.contains(&"classes/UserRecord"));
    }

    #[test]
    fn detects_event_emitters_subscribers_and_handlers() {
        let emitter = type_concept("emit_user_created", ConceptKind::Function, true);
        let subscriber = type_concept("on_user_created", ConceptKind::Function, true);
        let handler = type_concept("payment_event", ConceptKind::Function, true);
        let unrelated = type_concept("get_user", ConceptKind::Function, true);

        let concepts = vec![emitter, subscriber, handler, unrelated];
        let found = detect_features(&concepts);

        assert_eq!(found.len(), 3);
        assert!(found.iter().all(|f| f.kind == FeatureKind::EventFlow));
        let ids: Vec<&str> = found.iter().map(|f| f.concept_id.as_str()).collect();
        assert!(ids.contains(&"functions/emit_user_created"));
        assert!(ids.contains(&"functions/on_user_created"));
        assert!(ids.contains(&"functions/payment_event"));
    }

    #[test]
    fn results_are_sorted_by_kind_then_id() {
        let controller = type_concept("XController", ConceptKind::Class, true);
        let action = method("XController", "list", true);
        let db_model = type_concept("XModel", ConceptKind::Struct, true);
        let event = type_concept("emit_x", ConceptKind::Function, true);

        let concepts = vec![controller, action, db_model, event];
        let found = detect_features(&concepts);

        assert_eq!(found.len(), 3);
        assert_eq!(found[0].kind, FeatureKind::RestEndpoint);
        assert_eq!(found[1].kind, FeatureKind::DatabaseModel);
        assert_eq!(found[2].kind, FeatureKind::EventFlow);
    }
}
