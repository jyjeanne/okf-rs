//! Shared method-to-owning-type correlation, used by both the
//! design-pattern (`patterns.rs`) and REST-endpoint/database-model/
//! event-flow (`features.rs`) detectors.
//!
//! Every consumer correlates a method to its owning type via
//! [`owner_path`] rather than [`Concept::qualified_name`], since that
//! field's separator isn't stable across a fresh analysis (`.`, what
//! every language extractor emits) and a bundle read back off disk
//! (`::`, `okf_parser::read_bundle`'s best-effort reconstruction, since
//! frontmatter doesn't store `qualified_name` itself) — [`Concept::id`],
//! built once by [`Concept::make_id`], always uses `/` regardless of
//! which path a caller's concepts came from.

use okf_parser::{Concept, ConceptKind};
use std::collections::HashMap;

/// The id path after its kind-directory prefix (e.g. `structs/`,
/// `functions/`) — the qualified path [`Concept::make_id`] encodes,
/// stripped of the kind-specific directory so a type and its own methods
/// (which live under a *different* kind directory: `structs/` vs
/// `functions/`) can be correlated by comparing this instead of the full
/// id.
pub(crate) fn path_after_kind_dir(id: &str) -> &str {
    id.split_once('/').map(|(_, rest)| rest).unwrap_or(id)
}

/// A method's owning type/module path, derived from its own id — the
/// counterpart of a type concept's own [`path_after_kind_dir`], so the
/// two can be compared directly to correlate a method to its declaring
/// type.
pub(crate) fn owner_path(method_id: &str) -> Option<&str> {
    path_after_kind_dir(method_id)
        .rsplit_once('/')
        .map(|(owner, _)| owner)
}

pub(crate) fn is_type_kind(kind: ConceptKind) -> bool {
    matches!(
        kind,
        ConceptKind::Struct | ConceptKind::Class | ConceptKind::Interface | ConceptKind::Trait
    )
}

pub(crate) fn group_methods_by_owner(concepts: &[Concept]) -> HashMap<&str, Vec<&Concept>> {
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
