//! Receiver-type call resolution: picks the method a call actually reaches
//! from the type of its receiver (`store.concurrency()` → the method on
//! `Store`), using the per-file [`TypeFacts`] the Rust extractor records
//! and the [`TypeHint`] on each call candidate.
//!
//! Name-only resolution (one function or method with that name in the
//! whole project) stays the first rule; this index refines it in two
//! directions:
//!
//! - **Ambiguous name, typed receiver:** if exactly one candidate belongs
//!   to the receiver's type (its own method, a trait method it implements,
//!   or a default method of a trait it implements), that candidate is the
//!   callee. The edge is recorded with resolver [`RESOLVER`].
//! - **Unique name, receiver of a type the project doesn't define** (a
//!   standard-library or third-party type): the call can't reach a project
//!   method, so the name-only edge is dropped instead of drawn. A project
//!   trait implemented for such a type makes it a project-known type, so
//!   that case is kept.
//!
//! Everything else keeps today's behavior. Type names are bare (no module
//! path), so two project types sharing a name pool their facts; a lookup
//! that becomes ambiguous that way simply doesn't resolve.

use okf_parser::{Concept, ConceptKind};
use okf_tree_sitter::{TypeFacts, TypeHint};
use std::collections::{BTreeSet, HashMap, HashSet};

/// Provenance recorded on an edge resolved by receiver type.
pub(crate) const RESOLVER: &str = "tree-sitter-receiver";

/// Methods whose result has the receiver's own type as far as method
/// lookup is concerned (clones, borrows, unwraps, lock guards), applied
/// only when the project itself defines no method of that name on the
/// type.
const IDENTITY_METHODS: &[&str] = &[
    "clone",
    "as_ref",
    "as_mut",
    "borrow",
    "borrow_mut",
    "to_owned",
    "unwrap",
    "expect",
    "unwrap_or_default",
    "deref",
    "deref_mut",
    "as_deref",
    "lock",
];

/// Bound on nested hint resolution (chained calls, fields of fields).
const MAX_DEPTH: usize = 12;

/// Project-wide type facts, indexed for [`TypeIndex::resolve`] and
/// [`TypeIndex::accepts`].
#[derive(Debug, Default)]
pub(crate) struct TypeIndex {
    /// Method concept id → (owning type, implemented trait).
    owner_of: HashMap<String, (String, Option<String>)>,
    /// (type, method) → return types seen; more than one means ambiguous.
    method_returns: HashMap<(String, String), BTreeSet<String>>,
    /// (type, field) → field types seen.
    field_types: HashMap<(String, String), BTreeSet<String>>,
    /// Free function name → return types seen.
    function_returns: HashMap<String, BTreeSet<String>>,
    /// Type → traits it implements (from `impl Trait for Type` methods).
    implements: HashMap<String, BTreeSet<String>>,
    /// Type alias → its targets (more than one means ambiguous).
    aliases: HashMap<String, BTreeSet<String>>,
    /// Every type name the project defines or implements methods for.
    project_types: HashSet<String>,
}

impl TypeIndex {
    /// Builds the index from every file's facts plus the type concepts
    /// (structs, enums, traits, ...) the project declares.
    pub(crate) fn new<'a>(
        facts: impl IntoIterator<Item = &'a TypeFacts>,
        concepts: &[Concept],
    ) -> Self {
        let mut index = TypeIndex::default();
        for concept in concepts {
            if matches!(
                concept.kind,
                ConceptKind::Struct
                    | ConceptKind::Enum
                    | ConceptKind::Trait
                    | ConceptKind::Class
                    | ConceptKind::Interface
            ) {
                index.project_types.insert(concept.name.clone());
            }
        }
        for facts in facts {
            for method in &facts.methods {
                index.project_types.insert(method.owner.clone());
                if let Some(id) = &method.concept_id {
                    index.owner_of.insert(
                        id.clone(),
                        (method.owner.clone(), method.trait_name.clone()),
                    );
                }
                if let Some(trait_name) = &method.trait_name {
                    index
                        .implements
                        .entry(method.owner.clone())
                        .or_default()
                        .insert(trait_name.clone());
                }
                if let Some(returns) = &method.returns {
                    index
                        .method_returns
                        .entry((method.owner.clone(), method.name.clone()))
                        .or_default()
                        .insert(returns.clone());
                }
            }
            for field in &facts.fields {
                index.project_types.insert(field.owner.clone());
                index
                    .field_types
                    .entry((field.owner.clone(), field.field.clone()))
                    .or_default()
                    .insert(field.ty.clone());
            }
            for alias in &facts.aliases {
                // An alias is a project name even when its target isn't
                // nameable, so a receiver typed with it is never "external".
                index.project_types.insert(alias.name.clone());
                if let Some(target) = &alias.target {
                    index
                        .aliases
                        .entry(alias.name.clone())
                        .or_default()
                        .insert(target.clone());
                }
            }
            for function in &facts.functions {
                if let Some(returns) = &function.returns {
                    index
                        .function_returns
                        .entry(function.name.clone())
                        .or_default()
                        .insert(returns.clone());
                }
            }
        }
        index
    }

    /// The type `hint` evaluates to, when every step of it is known and
    /// unambiguous.
    pub(crate) fn resolve(&self, hint: &TypeHint) -> Option<String> {
        self.resolve_at(hint, 0)
    }

    fn resolve_at(&self, hint: &TypeHint, depth: usize) -> Option<String> {
        if depth > MAX_DEPTH {
            return None;
        }
        match hint {
            TypeHint::Named(name) => match unique(self.aliases.get(name)) {
                Some(target) if &target != name => {
                    self.resolve_at(&TypeHint::Named(target), depth + 1)
                }
                _ => Some(name.clone()),
            },
            TypeHint::Field { base, field } => {
                let base = self.resolve_at(base, depth + 1)?;
                unique(self.field_types.get(&(base, field.clone())))
            }
            TypeHint::MethodReturn { base, method } => {
                let base = self.resolve_at(base, depth + 1)?;
                match self.method_returns.get(&(base.clone(), method.clone())) {
                    Some(returns) => unique(Some(returns)),
                    None => IDENTITY_METHODS.contains(&method.as_str()).then_some(base),
                }
            }
            TypeHint::FnReturn {
                owner: Some(owner),
                function,
            } => {
                let owner = self.resolve_at(&TypeHint::Named(owner.clone()), depth + 1)?;
                match self.method_returns.get(&(owner.clone(), function.clone())) {
                    Some(returns) => unique(Some(returns)),
                    // `ExternalType::new()`, `::from(..)`, `::default()`:
                    // an external type's associated functions return that
                    // type (or another external one) — either way not a
                    // project type, which is all the caller needs to know.
                    None if self.is_external(&owner) => Some(owner),
                    None => None,
                }
            }
            TypeHint::FnReturn {
                owner: None,
                function,
            } => unique(self.function_returns.get(function)),
        }
    }

    /// Whether a call on a receiver of type `ty` can reach method concept
    /// `callee`: it's `ty`'s own method, a trait method `ty` implements, or
    /// a default method of a trait `ty` implements.
    pub(crate) fn accepts(&self, callee: &str, ty: &str) -> bool {
        let Some((owner, trait_name)) = self.owner_of.get(callee) else {
            return false;
        };
        owner == ty
            || trait_name.as_deref() == Some(ty)
            || self
                .implements
                .get(ty)
                .is_some_and(|traits| traits.contains(owner))
    }

    /// Whether `ty` is a type the project neither defines nor implements
    /// any method for — a standard-library or third-party type, whose
    /// methods are never project concepts.
    pub(crate) fn is_external(&self, ty: &str) -> bool {
        !self.project_types.contains(ty)
    }
}

fn unique(set: Option<&BTreeSet<String>>) -> Option<String> {
    let set = set?;
    if set.len() == 1 {
        set.iter().next().cloned()
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::RESOLVER;
    use okf_core::Project;
    use okf_parser::{Confidence, RelationKind, Relationship};
    use std::fs;

    /// Analyzes a one-file Rust project and returns the `Calls` edges of
    /// the function named `caller`.
    fn calls_of(source: &str, caller: &str) -> Vec<Relationship> {
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir_all(dir.path().join("src")).unwrap();
        fs::write(dir.path().join("src/lib.rs"), source).unwrap();
        let project = Project::load(dir.path()).unwrap();
        let result = crate::analyze(&project).unwrap();
        result
            .concepts
            .iter()
            .find(|c| c.name == caller)
            .unwrap_or_else(|| panic!("no concept named {caller}"))
            .relationships
            .iter()
            .filter(|r| r.kind == RelationKind::Calls)
            .cloned()
            .collect()
    }

    /// Two types share `concurrency`, so name matching alone can't pick.
    const STORES: &str = r#"
pub struct Store;
impl Store { pub fn concurrency(&self) -> usize { 1 } }
pub struct GitBackend;
impl GitBackend { pub fn concurrency(&self) -> usize { 1 } }
"#;

    #[test]
    fn an_ambiguous_method_resolves_through_a_typed_parameter() {
        let source = format!(
            "{STORES}\npub fn caller(store: &std::sync::Arc<Store>) {{ store.concurrency(); }}\n"
        );
        let calls = calls_of(&source, "caller");
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].target, "functions/src/Store/concurrency");
        assert_eq!(calls[0].resolved_by, RESOLVER);
        assert_eq!(calls[0].confidence, Confidence::Semantic);
    }

    #[test]
    fn a_chained_call_resolves_through_the_previous_method_s_return_type() {
        let source = format!(
            "{STORES}
pub struct Tree {{ store: std::sync::Arc<Store> }}
impl Tree {{ pub fn store(&self) -> &std::sync::Arc<Store> {{ &self.store }} }}
pub fn caller(tree: &Tree) {{ tree.store().concurrency(); }}
pub fn by_field(tree: &Tree) {{ tree.store.concurrency(); }}
"
        );
        let targets = |caller| {
            calls_of(&source, caller)
                .into_iter()
                .map(|r| r.target)
                .collect::<Vec<_>>()
        };
        assert!(targets("caller").contains(&"functions/src/Store/concurrency".to_string()));
        assert_eq!(targets("by_field"), vec!["functions/src/Store/concurrency"]);
    }

    #[test]
    fn a_constructor_result_types_the_binding() {
        let source = r#"
pub struct GitFetch;
impl GitFetch {
    pub fn new() -> Result<Self, ()> { Ok(GitFetch) }
    pub fn fetch(&mut self) {}
}
pub struct Index;
impl Index { pub fn fetch(&self) {} }
pub fn caller() -> Result<(), ()> {
    let mut git_fetch = GitFetch::new()?;
    git_fetch.fetch();
    Ok(())
}
"#;
        let targets: Vec<String> = calls_of(source, "caller")
            .into_iter()
            .map(|r| r.target)
            .collect();
        assert!(targets.contains(&"functions/src/GitFetch/fetch".to_string()));
        assert!(!targets.contains(&"functions/src/Index/fetch".to_string()));
    }

    #[test]
    fn a_trait_object_receiver_resolves_through_the_trait_signature() {
        let source = format!(
            "{STORES}
pub trait Repo {{ fn store(&self) -> &std::sync::Arc<Store>; }}
pub fn caller(repo: &dyn Repo) {{ repo.store().concurrency(); }}
"
        );
        let targets: Vec<String> = calls_of(&source, "caller")
            .into_iter()
            .map(|r| r.target)
            .collect();
        assert_eq!(targets, vec!["functions/src/Store/concurrency"]);
    }

    #[test]
    fn a_unique_name_called_on_a_third_party_type_draws_no_edge() {
        // Before: `temp_file.persist(..)` was drawn to the project's only
        // `persist` method, though `NamedTempFile` is not a project type.
        let source = r#"
pub struct RunWorkspace;
impl RunWorkspace { pub fn persist(&self) {} }
pub fn caller(temp_file: tempfile::NamedTempFile) { temp_file.persist("x"); }
pub fn real(ws: &RunWorkspace) { ws.persist(); }
"#;
        assert!(calls_of(source, "caller").is_empty());
        let real = calls_of(source, "real");
        assert_eq!(real.len(), 1);
        // Unique by name and consistent with the receiver: still the plain
        // name-match provenance.
        assert_eq!(real[0].resolved_by, "tree-sitter");
    }

    #[test]
    fn a_binding_built_by_an_external_constructor_is_external() {
        // `deps.insert(..)` on a `BTreeSet` used to be drawn to the
        // project's only `insert` method.
        let source = r#"
pub struct AnalysisCache;
impl AnalysisCache { pub fn insert(&mut self, _k: u8) {} }
pub fn caller() { let mut deps = std::collections::BTreeSet::new(); deps.insert(1); }
"#;
        assert!(calls_of(source, "caller").is_empty());
    }

    #[test]
    fn a_wrapper_constructor_takes_its_argument_s_type() {
        let source = format!(
            "{STORES}\npub fn caller() {{ let s = std::sync::Arc::new(Store); s.concurrency(); }}\n"
        );
        let calls = calls_of(&source, "caller");
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].target, "functions/src/Store/concurrency");
    }

    #[test]
    fn unknown_receivers_keep_name_only_behavior() {
        let source = format!(
            "{STORES}
pub fn unbounded<T>(t: T) {{ t.concurrency(); }}
pub struct Solo;
impl Solo {{ pub fn only_here(&self) {{}} }}
pub fn untyped_closure() {{ let f = |s: Solo| s.only_here(); }}
"
        );
        // Ambiguous name, unknown receiver: no edge, as before.
        assert!(calls_of(&source, "unbounded").is_empty());
        // Unique name, untraceable receiver: the name-only edge stays.
        assert_eq!(calls_of(&source, "untyped_closure").len(), 1);
    }

    #[test]
    fn a_default_trait_method_on_an_implementing_type_resolves() {
        let source = format!(
            "{STORES}
pub trait Tuned {{ fn tune(&self) {{}} fn concurrency(&self) -> usize {{ 2 }} }}
pub struct Plain;
impl Tuned for Plain {{ fn tune(&self) {{}} }}
pub fn caller(p: &Plain) {{ p.concurrency(); }}
"
        );
        let calls = calls_of(&source, "caller");
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].target, "functions/src/Tuned/concurrency");
        assert_eq!(calls[0].resolved_by, RESOLVER);
    }

    #[test]
    fn the_reason_text_names_receiver_typing() {
        let source = format!("{STORES}\npub fn caller(store: &Store) {{ store.concurrency(); }}\n");
        let reason = calls_of(&source, "caller")[0].reason();
        assert!(reason.contains("receiver type"), "{reason}");
    }
}
