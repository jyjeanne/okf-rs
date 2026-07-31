//! Cross-module, ownership, API-surface, and cycle queries over an
//! okf-rs concept graph.
//!
//! `Graph` is built directly from a `&[Concept]` slice and doesn't care
//! where those concepts came from: a fresh `okf-analyzer` run, or a
//! previously written bundle read back with `okf_parser::read_bundle`
//! (which restores the `Calls`/`CalledBy`/`Imports`/... relationships
//! from the bundle's `relationships` frontmatter field). `okf-rs graph`
//! uses the latter — it queries an existing bundle on disk rather than
//! re-analyzing the project from source, the same way `okf-search` and
//! `okf-validator` do.
//!
//! # Stability
//!
//! This is a public, documented library crate — not just an internal
//! implementation detail of `okf-cli`/`okf-mcp` — versioned together with
//! the rest of the `okf-rs` workspace (a single `Cargo.toml`-wide semver
//! number, bumped by the same release process for every crate). A
//! breaking change to `Graph`'s public API is a breaking change for the
//! whole workspace, not something that can land silently. Every public
//! item is documented (enforced by `#![deny(missing_docs)]` below), so a
//! consumer other than `okf-cli`/`okf-mcp` — e.g. `okf-server` in Phase 4
//! — can embed this crate directly against concepts it already has in
//! memory, without shelling out to the `okf-rs` binary and parsing its
//! text output.
//!
//! # Example
//!
//! ```
//! # use okf_parser::{Concept, ConceptKind, Language, Location, RelationKind, Relationship};
//! # fn concept(id: &str) -> Concept {
//! #     Concept {
//! #         id: id.to_string(),
//! #         kind: ConceptKind::Function,
//! #         language: Language::Rust,
//! #         name: id.to_string(),
//! #         qualified_name: id.to_string(),
//! #         description: None,
//! #         location: Location { file: "src/lib.rs".to_string(), start_line: 1, end_line: 1 },
//! #         signature: None,
//! #         tags: Vec::new(),
//! #         is_public: true,
//! #         generated_at: None,
//! #         relationships: Vec::new(),
//! #     }
//! # }
//! # fn edge(concept: &mut Concept, kind: RelationKind, target: &str) {
//! #     concept.relationships.push(Relationship {
//! #         kind,
//! #         target: target.to_string(),
//! #         target_display: target.to_string(),
//! #     });
//! # }
//! use okf_graph::Graph;
//!
//! // A real bundle carries both sides of a resolved call edge (`Calls`
//! // on the caller, `CalledBy` on the callee) — `okf_parser::read_bundle`
//! // hands back concepts already shaped this way.
//! let (mut a, mut b) = (concept("a"), concept("b"));
//! edge(&mut a, RelationKind::Calls, "b");
//! edge(&mut b, RelationKind::CalledBy, "a");
//!
//! let concepts = vec![a, b];
//! let graph = Graph::build(&concepts);
//!
//! assert_eq!(graph.callees("a")[0].id, "b");
//! assert_eq!(graph.callers("b")[0].id, "a");
//! assert!(graph.cycles().is_empty());
//! ```

#![deny(missing_docs)]

use okf_parser::{Concept, ConceptKind, RelationKind};
use std::collections::{HashMap, HashSet, VecDeque};

/// A queryable graph over a set of [`Concept`]s: cross-module links,
/// ownership, public API surface, and cycle/dependency analysis. Borrows
/// its concepts rather than owning them, so building one is cheap and
/// doesn't duplicate the underlying data.
pub struct Graph<'a> {
    concepts: &'a [Concept],
    by_id: HashMap<&'a str, usize>,
    /// Module concept id, keyed by source file — lets any concept find
    /// the module that owns it (containment/ownership) without needing
    /// an explicit `MemberOf` relationship.
    module_by_file: HashMap<&'a str, &'a str>,
}

impl<'a> Graph<'a> {
    /// Indexes `concepts` by id (and, for `Module` concepts, by owning
    /// source file) so every query below runs in roughly constant time
    /// per lookup rather than rescanning the slice.
    pub fn build(concepts: &'a [Concept]) -> Self {
        let by_id = concepts
            .iter()
            .enumerate()
            .map(|(i, c)| (c.id.as_str(), i))
            .collect();

        let module_by_file = concepts
            .iter()
            .filter(|c| c.kind == ConceptKind::Module)
            .map(|c| (c.location.file.as_str(), c.id.as_str()))
            .collect();

        Graph {
            concepts,
            by_id,
            module_by_file,
        }
    }

    /// Looks up a concept by its exact id, if it's in the graph.
    pub fn get(&self, id: &str) -> Option<&'a Concept> {
        self.by_id.get(id).map(|&i| &self.concepts[i])
    }

    /// Concepts this concept directly calls.
    pub fn callees(&self, id: &str) -> Vec<&'a Concept> {
        self.related(id, RelationKind::Calls)
    }

    /// Concepts that directly call this concept.
    pub fn callers(&self, id: &str) -> Vec<&'a Concept> {
        self.related(id, RelationKind::CalledBy)
    }

    fn related(&self, id: &str, kind: RelationKind) -> Vec<&'a Concept> {
        let Some(concept) = self.get(id) else {
            return Vec::new();
        };
        concept
            .relationships
            .iter()
            .filter(|r| r.kind == kind)
            .filter_map(|r| self.get(&r.target))
            .collect()
    }

    /// The module concept that owns `id` (declared in the same source
    /// file), if any.
    pub fn owning_module(&self, id: &str) -> Option<&'a Concept> {
        let concept = self.get(id)?;
        let module_id = self.module_by_file.get(concept.location.file.as_str())?;
        self.get(module_id)
    }

    /// The package concept that owns `id`: either `id`'s own `MemberOf`
    /// relationship if it's a `Module` (which carries one directly, for a
    /// multi-package workspace/monorepo — see `okf-analyzer`'s
    /// aggregation), or its owning module's `MemberOf`, for anything else.
    pub fn owning_package(&self, id: &str) -> Option<&'a Concept> {
        if let Some(package) = self.related(id, RelationKind::MemberOf).into_iter().next() {
            return Some(package);
        }
        let module = self.owning_module(id)?;
        self.related(&module.id, RelationKind::MemberOf)
            .into_iter()
            .next()
    }

    /// Every concept declared in `module_id`'s source file, excluding the
    /// module concept itself.
    pub fn members_of(&self, module_id: &str) -> Vec<&'a Concept> {
        let Some(module) = self.get(module_id) else {
            return Vec::new();
        };
        if module.kind != ConceptKind::Module {
            return Vec::new();
        }
        self.concepts
            .iter()
            .filter(|c| c.id != module.id && c.location.file == module.location.file)
            .collect()
    }

    /// Every concept of a given kind, sorted by id for deterministic
    /// output — e.g. `of_kind(ConceptKind::Package)` to enumerate every
    /// package in the bundle, the way `okf-arch`'s architecture-layer and
    /// domain-boundary analysis does.
    pub fn of_kind(&self, kind: ConceptKind) -> Vec<&'a Concept> {
        let mut matches: Vec<&Concept> = self.concepts.iter().filter(|c| c.kind == kind).collect();
        matches.sort_by(|a, b| a.id.cmp(&b.id));
        matches
    }

    /// Every concept marked public (see [`Concept::is_public`]), sorted
    /// by id for deterministic output. `Module`/`Package`/`Document`
    /// concepts are excluded — they're structural containers or
    /// documentation, not API surface.
    pub fn public_api(&self) -> Vec<&'a Concept> {
        let mut public: Vec<&Concept> = self
            .concepts
            .iter()
            .filter(|c| c.is_public && !is_structural(c.kind))
            .collect();
        public.sort_by(|a, b| a.id.cmp(&b.id));
        public
    }

    /// Cross-module dependency edges: for every `Calls` relationship
    /// whose caller and callee live in different modules, the pair of
    /// owning module ids — deduplicated and sorted for determinism.
    pub fn module_dependencies(&self) -> Vec<(&'a str, &'a str)> {
        let mut edges = HashSet::new();
        for concept in self.concepts {
            for rel in &concept.relationships {
                if rel.kind != RelationKind::Calls {
                    continue;
                }
                let (Some(from), Some(to)) = (
                    self.owning_module(&concept.id),
                    self.owning_module(&rel.target),
                ) else {
                    continue;
                };
                if from.id != to.id {
                    edges.insert((from.id.as_str(), to.id.as_str()));
                }
            }
        }
        let mut edges: Vec<_> = edges.into_iter().collect();
        edges.sort();
        edges
    }

    /// Concepts with no `Calls`/`CalledBy` edge in either direction:
    /// never observed calling anything, and never observed being called.
    /// `Module`/`Package`/`Document` concepts are excluded — they're
    /// structural containers or documentation, not call-graph
    /// participants, so having no `Calls` edge of their own is expected
    /// rather than a quality signal (see `public_api`, which excludes
    /// them for the same reason). Sorted by id for deterministic output.
    ///
    /// This is a different notion of "orphan" than `okf-validator`'s
    /// index-reachability check: that one asks whether a concept is
    /// linked into the bundle's markdown *narrative* starting from
    /// `index.md`; this one asks whether it participates in the *call*
    /// graph at all. A concept can be reachable from `index.md` (every
    /// concept is, once `okf-generator` emits its owning module/package
    /// index) while still calling nothing and being called by nothing —
    /// dead code, an unused public entry point, or a call the analyzer
    /// simply couldn't resolve.
    pub fn isolated_concepts(&self) -> Vec<&'a Concept> {
        let mut isolated: Vec<&Concept> = self
            .concepts
            .iter()
            .filter(|c| !is_structural(c.kind))
            .filter(|c| {
                !c.relationships
                    .iter()
                    .any(|r| matches!(r.kind, RelationKind::Calls | RelationKind::CalledBy))
            })
            .collect();
        isolated.sort_by(|a, b| a.id.cmp(&b.id));
        isolated
    }

    /// Connected components of the *undirected* graph formed by treating
    /// every `Calls`/`CalledBy` edge as bidirectional — "which clusters of
    /// mutually-reachable-by-call concepts exist," as opposed to
    /// `cycles()` (directed cycles only) or `isolated_concepts()` (only
    /// the zero-edge case). Only `Calls`/`CalledBy` form the adjacency,
    /// not every relationship kind: mixing in `Imports`/`MemberOf`/...
    /// would conflate structural containment with call topology and
    /// answer a different question than "how many islands of
    /// collaborating code are there."
    ///
    /// A concept with no `Calls`/`CalledBy` edge at all forms its own
    /// trivial single-concept component; those are omitted here since
    /// they're already reported, with more context, by
    /// `isolated_concepts()`. A self-recursive concept (`Calls`/
    /// `CalledBy` pointing at itself — the same shape `cycles()` treats
    /// as "direct self-recursion") is *not* omitted even though it's
    /// also a single-concept component: it has a real edge and
    /// `isolated_concepts()` correctly doesn't report it either (it does
    /// have a `Calls`/`CalledBy` relationship), so dropping it here too
    /// would make it invisible to both. Every other single-concept
    /// component (no self-loop) is dropped. Each component's ids are
    /// sorted; components themselves are sorted by their first id, for
    /// deterministic output.
    pub fn connected_components(&self) -> Vec<Vec<&'a str>> {
        let mut adjacency: HashMap<&str, HashSet<&str>> = HashMap::new();
        for concept in self.concepts {
            for rel in &concept.relationships {
                if !matches!(rel.kind, RelationKind::Calls | RelationKind::CalledBy) {
                    continue;
                }
                let Some(target) = self.get(&rel.target) else {
                    continue;
                };
                adjacency
                    .entry(concept.id.as_str())
                    .or_default()
                    .insert(target.id.as_str());
                adjacency
                    .entry(target.id.as_str())
                    .or_default()
                    .insert(concept.id.as_str());
            }
        }

        let mut visited: HashSet<&str> = HashSet::new();
        let mut components: Vec<Vec<&str>> = Vec::new();
        for concept in self.concepts {
            let id = concept.id.as_str();
            if visited.contains(id) || !adjacency.contains_key(id) {
                continue;
            }
            let mut component = Vec::new();
            let mut queue = VecDeque::new();
            queue.push_back(id);
            visited.insert(id);
            while let Some(current) = queue.pop_front() {
                component.push(current);
                for &neighbor in adjacency.get(current).into_iter().flatten() {
                    if visited.insert(neighbor) {
                        queue.push_back(neighbor);
                    }
                }
            }
            let has_self_loop = adjacency
                .get(id)
                .is_some_and(|neighbors| neighbors.contains(id));
            if component.len() > 1 || has_self_loop {
                component.sort();
                components.push(component);
            }
        }
        components.sort();
        components
    }

    /// Groups of concepts that call each other in a cycle through the
    /// `Calls` graph (mutual/indirect recursion across two or more
    /// concepts, or direct self-recursion), found via Tarjan's strongly
    /// connected components algorithm. Each returned group's ids are
    /// sorted; groups themselves are sorted by their first id, so output
    /// is deterministic. This reports *which concepts* form a cycle, not
    /// every distinct path through it — enumerating all elementary
    /// cycles is combinatorially expensive and rarely what "does this
    /// have a cycle" questions actually need.
    pub fn cycles(&self) -> Vec<Vec<&'a str>> {
        let nodes = self.concepts.iter().map(|c| c.id.as_str());
        let sccs = tarjan_scc(nodes, |id| {
            self.callees(id).iter().map(|c| c.id.as_str()).collect()
        });
        let mut cycles: Vec<Vec<&str>> = sccs
            .into_iter()
            .filter(|scc| {
                scc.len() > 1
                    || scc
                        .first()
                        .is_some_and(|&id| self.callees(id).iter().any(|c| c.id == id))
            })
            .map(|mut scc| {
                scc.sort();
                scc
            })
            .collect();
        cycles.sort();
        cycles
    }

    /// Every concept transitively affected if `id` changes: `id`'s direct
    /// and indirect callers, found by breadth-first search over
    /// `CalledBy` edges. This is the "blast radius" primitive — who
    /// depends on `id`, directly or through a chain of callers — used by
    /// both `okf-rs impact` (change-impact analysis between two git
    /// refs) and the `explore` composite query (blast radius of a single
    /// concept in the current bundle).
    ///
    /// `max_depth` bounds how many hops to follow (`Some(1)` is exactly
    /// [`Graph::callers`]; `None` is unbounded). Excludes `id` itself,
    /// even if it's part of a call cycle back to itself. Sorted by id,
    /// deduplicated, for deterministic output.
    pub fn transitive_callers(&self, id: &str, max_depth: Option<usize>) -> Vec<&'a Concept> {
        let mut visited: HashSet<&str> = HashSet::new();
        visited.insert(id);
        let mut frontier: Vec<&str> = vec![id];
        let mut depth = 0;

        while !frontier.is_empty() && max_depth.is_none_or(|max| depth < max) {
            let mut next_frontier = Vec::new();
            for &current in &frontier {
                for caller in self.callers(current) {
                    if visited.insert(caller.id.as_str()) {
                        next_frontier.push(caller.id.as_str());
                    }
                }
            }
            frontier = next_frontier;
            depth += 1;
        }

        visited.remove(id);
        let mut result: Vec<&Concept> = visited.into_iter().filter_map(|i| self.get(i)).collect();
        result.sort_by(|a, b| a.id.cmp(&b.id));
        result
    }

    /// Shortest directed path from `from` to `to` following `Calls`
    /// edges (breadth-first), or `None` if unreachable. Includes both
    /// endpoints.
    ///
    /// `from`/`to` are taken by ordinary `&str` (not tied to the graph's
    /// `'a`), since they're only ever used to look up a concept — the
    /// returned path is always built from the concepts' own `id` strings.
    pub fn shortest_call_path(&self, from: &str, to: &str) -> Option<Vec<&'a str>> {
        let from = self.get(from)?.id.as_str();
        let to = self.get(to)?.id.as_str();
        if from == to {
            return Some(vec![from]);
        }

        let mut visited: HashSet<&str> = HashSet::new();
        let mut queue: std::collections::VecDeque<Vec<&str>> = std::collections::VecDeque::new();
        queue.push_back(vec![from]);
        visited.insert(from);

        while let Some(path) = queue.pop_front() {
            let current = *path.last().unwrap();
            for callee in self.callees(current) {
                if callee.id == to {
                    let mut full = path.clone();
                    full.push(callee.id.as_str());
                    return Some(full);
                }
                if visited.insert(callee.id.as_str()) {
                    let mut next = path.clone();
                    next.push(callee.id.as_str());
                    queue.push_back(next);
                }
            }
        }
        None
    }
}

/// A structural concept: a container (`Module`/`Package`) or
/// documentation (`Document`), never itself a call-graph participant or
/// API surface — see [`Graph::public_api`]/[`Graph::isolated_concepts`],
/// which both exclude these kinds for the same reason. Exposed for reuse
/// wherever else the same structural/non-structural distinction applies
/// (e.g. `okf_query`'s coverage report).
pub fn is_structural(kind: ConceptKind) -> bool {
    matches!(
        kind,
        ConceptKind::Module | ConceptKind::Package | ConceptKind::Document
    )
}

/// Standard recursive Tarjan's strongly-connected-components algorithm,
/// generic over any string-id node space and neighbor lookup — shared by
/// [`Graph::cycles`] (scoped to a concept graph's `Calls` edges) and
/// `okf_arch`'s package-dependency-cycle collapsing (scoped to an
/// explicit package-id adjacency map), which would otherwise each need
/// their own copy of the same algorithm. `nodes` is walked explicitly
/// (not just discovered via `neighbors`) so a node with no edges at all
/// still gets its own single-node SCC, in the order `nodes` lists them —
/// deterministic as long as the caller passes a deterministically
/// ordered `nodes`.
pub fn tarjan_scc<'a>(
    nodes: impl IntoIterator<Item = &'a str>,
    neighbors: impl Fn(&'a str) -> Vec<&'a str>,
) -> Vec<Vec<&'a str>> {
    struct Tarjan<'a, F> {
        neighbors: F,
        index_counter: usize,
        index: HashMap<&'a str, usize>,
        lowlink: HashMap<&'a str, usize>,
        on_stack: HashSet<&'a str>,
        stack: Vec<&'a str>,
        sccs: Vec<Vec<&'a str>>,
    }

    impl<'a, F: Fn(&'a str) -> Vec<&'a str>> Tarjan<'a, F> {
        fn visit(&mut self, node: &'a str) {
            self.index.insert(node, self.index_counter);
            self.lowlink.insert(node, self.index_counter);
            self.index_counter += 1;
            self.stack.push(node);
            self.on_stack.insert(node);

            for neighbor in (self.neighbors)(node) {
                if !self.index.contains_key(neighbor) {
                    self.visit(neighbor);
                    let neighbor_low = self.lowlink[neighbor];
                    let my_low = self.lowlink[node];
                    self.lowlink.insert(node, my_low.min(neighbor_low));
                } else if self.on_stack.contains(neighbor) {
                    let neighbor_idx = self.index[neighbor];
                    let my_low = self.lowlink[node];
                    self.lowlink.insert(node, my_low.min(neighbor_idx));
                }
            }

            if self.lowlink[node] == self.index[node] {
                let mut scc = Vec::new();
                loop {
                    let member = self.stack.pop().unwrap();
                    self.on_stack.remove(member);
                    scc.push(member);
                    if member == node {
                        break;
                    }
                }
                self.sccs.push(scc);
            }
        }
    }

    let mut tarjan = Tarjan {
        neighbors,
        index_counter: 0,
        index: HashMap::new(),
        lowlink: HashMap::new(),
        on_stack: HashSet::new(),
        stack: Vec::new(),
        sccs: Vec::new(),
    };
    for node in nodes {
        if !tarjan.index.contains_key(node) {
            tarjan.visit(node);
        }
    }
    tarjan.sccs
}

#[cfg(test)]
mod tests {
    use super::*;
    use okf_parser::{Language, Location, Relationship};

    fn concept(id: &str, kind: ConceptKind, file: &str, is_public: bool) -> Concept {
        Concept {
            id: id.to_string(),
            kind,
            language: Language::Rust,
            name: id.rsplit('/').next().unwrap().to_string(),
            qualified_name: id.to_string(),
            description: None,
            location: Location {
                file: file.to_string(),
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

    /// Pushes one side of a call edge. Real `okf-analyzer` output always
    /// populates both `Calls` (on the caller) and `CalledBy` (on the
    /// callee) when it resolves an edge, so tests build fixtures the same
    /// way — calling this once per concept per edge, including twice on
    /// the same concept for a self-call.
    fn add_edge(concept: &mut Concept, kind: RelationKind, target_id: &str) {
        concept.relationships.push(Relationship {
            kind,
            target: target_id.to_string(),
            target_display: target_id.to_string(),
        });
    }

    #[test]
    fn callers_and_callees_and_ownership() {
        let module_a = concept("modules/a", ConceptKind::Module, "a.rs", true);
        let module_b = concept("modules/b", ConceptKind::Module, "b.rs", true);
        let mut f = concept("functions/a/f", ConceptKind::Function, "a.rs", true);
        let mut g = concept("functions/b/g", ConceptKind::Function, "b.rs", true);
        add_edge(&mut f, RelationKind::Calls, "functions/b/g");
        add_edge(&mut g, RelationKind::CalledBy, "functions/a/f");

        let concepts = vec![module_a, module_b, f, g];
        let graph = Graph::build(&concepts);

        assert_eq!(graph.callees("functions/a/f")[0].id, "functions/b/g");
        assert_eq!(graph.callers("functions/b/g")[0].id, "functions/a/f");
        assert_eq!(
            graph.owning_module("functions/a/f").unwrap().id,
            "modules/a"
        );
        assert_eq!(graph.members_of("modules/a")[0].id, "functions/a/f",);

        let deps = graph.module_dependencies();
        assert_eq!(deps, vec![("modules/a", "modules/b")]);
    }

    #[test]
    fn owning_package_resolves_through_a_module_for_any_concept() {
        let package = concept("packages/demo", ConceptKind::Package, "Cargo.toml", true);
        let mut module = concept("modules/a", ConceptKind::Module, "a.rs", true);
        add_edge(&mut module, RelationKind::MemberOf, "packages/demo");
        let function = concept("functions/a/f", ConceptKind::Function, "a.rs", true);

        let concepts = vec![package, module, function];
        let graph = Graph::build(&concepts);

        assert_eq!(
            graph.owning_package("modules/a").unwrap().id,
            "packages/demo",
            "a Module resolves its own MemberOf relationship directly"
        );
        assert_eq!(
            graph.owning_package("functions/a/f").unwrap().id,
            "packages/demo",
            "a Function resolves transitively through its owning Module"
        );
    }

    #[test]
    fn owning_package_is_none_without_a_detected_package() {
        let module = concept("modules/a", ConceptKind::Module, "a.rs", true);
        let concepts = vec![module];
        let graph = Graph::build(&concepts);
        assert!(graph.owning_package("modules/a").is_none());
    }

    #[test]
    fn public_api_excludes_private_and_structural_kinds() {
        let module = concept("modules/a", ConceptKind::Module, "a.rs", true);
        let public_fn = concept("functions/a/pub_fn", ConceptKind::Function, "a.rs", true);
        let private_fn = concept("functions/a/priv_fn", ConceptKind::Function, "a.rs", false);

        let concepts = vec![module, public_fn, private_fn];
        let graph = Graph::build(&concepts);

        let api = graph.public_api();
        assert_eq!(api.len(), 1);
        assert_eq!(api[0].id, "functions/a/pub_fn");
    }

    #[test]
    fn document_concepts_are_excluded_from_public_api_and_isolated_concepts() {
        // A Document (e.g. an okf-dita-imported DITA topic) is
        // structural the same way Module/Package are: never expected to
        // carry a Calls/CalledBy edge, and not API surface.
        let doc = concept(
            "documents/readme",
            ConceptKind::Document,
            "readme.dita",
            true,
        );
        let concepts = vec![doc];
        let graph = Graph::build(&concepts);

        assert!(graph.public_api().is_empty());
        assert!(graph.isolated_concepts().is_empty());
    }

    #[test]
    fn of_kind_filters_and_sorts_by_id() {
        let module = concept("modules/a", ConceptKind::Module, "a.rs", true);
        let fn_b = concept("functions/b", ConceptKind::Function, "a.rs", true);
        let fn_a = concept("functions/a", ConceptKind::Function, "a.rs", true);

        let concepts = vec![module, fn_b, fn_a];
        let graph = Graph::build(&concepts);

        let functions = graph.of_kind(ConceptKind::Function);
        assert_eq!(
            functions.iter().map(|c| c.id.as_str()).collect::<Vec<_>>(),
            vec!["functions/a", "functions/b"]
        );
        assert_eq!(graph.of_kind(ConceptKind::Package), Vec::<&Concept>::new());
    }

    #[test]
    fn isolated_concepts_excludes_modules_and_connected_functions() {
        let module = concept("modules/a", ConceptKind::Module, "a.rs", true);
        let mut caller = concept("functions/a/caller", ConceptKind::Function, "a.rs", true);
        let mut callee = concept("functions/a/callee", ConceptKind::Function, "a.rs", true);
        let unused = concept("functions/a/unused", ConceptKind::Function, "a.rs", true);
        add_edge(&mut caller, RelationKind::Calls, "functions/a/callee");
        add_edge(&mut callee, RelationKind::CalledBy, "functions/a/caller");

        let concepts = vec![module, caller, callee, unused];
        let graph = Graph::build(&concepts);

        let isolated = graph.isolated_concepts();
        assert_eq!(isolated.len(), 1);
        assert_eq!(isolated[0].id, "functions/a/unused");
    }

    #[test]
    fn connected_components_groups_by_call_edges_and_excludes_singletons() {
        // Two separate two-node clusters (a<->b, c<->d) plus one isolated
        // concept with no Calls/CalledBy edge at all.
        let mut a = concept("functions/a", ConceptKind::Function, "x.rs", true);
        let mut b = concept("functions/b", ConceptKind::Function, "x.rs", true);
        let mut c = concept("functions/c", ConceptKind::Function, "x.rs", true);
        let mut d = concept("functions/d", ConceptKind::Function, "x.rs", true);
        let isolated = concept("functions/isolated", ConceptKind::Function, "x.rs", true);
        add_edge(&mut a, RelationKind::Calls, "functions/b");
        add_edge(&mut b, RelationKind::CalledBy, "functions/a");
        add_edge(&mut c, RelationKind::Calls, "functions/d");
        add_edge(&mut d, RelationKind::CalledBy, "functions/c");

        let concepts = vec![a, b, c, d, isolated];
        let graph = Graph::build(&concepts);

        let components = graph.connected_components();
        assert_eq!(
            components.len(),
            2,
            "expected two components: {components:?}"
        );
        assert_eq!(components[0], vec!["functions/a", "functions/b"]);
        assert_eq!(components[1], vec!["functions/c", "functions/d"]);
        assert!(
            !components
                .iter()
                .flatten()
                .any(|&id| id == "functions/isolated"),
            "an isolated concept should not appear as its own component: {components:?}"
        );
    }

    #[test]
    fn a_self_recursive_concept_is_its_own_component_not_dropped_as_a_singleton() {
        // A concept whose only Calls/CalledBy edge points at itself has
        // a real edge (isolated_concepts() correctly excludes it), so
        // dropping it here too — as if it were an ordinary edgeless
        // singleton — would make it invisible to both metrics.
        let mut recursive = concept("functions/recursive", ConceptKind::Function, "x.rs", true);
        add_edge(&mut recursive, RelationKind::Calls, "functions/recursive");
        add_edge(
            &mut recursive,
            RelationKind::CalledBy,
            "functions/recursive",
        );

        let concepts = vec![recursive];
        let graph = Graph::build(&concepts);

        assert!(
            graph.isolated_concepts().is_empty(),
            "a self-recursive concept has an edge, so it isn't isolated"
        );
        assert_eq!(
            graph.connected_components(),
            vec![vec!["functions/recursive"]],
            "a self-recursive concept should form its own single-concept component"
        );
    }

    #[test]
    fn detects_direct_and_mutual_cycles() {
        let mut a = concept("functions/a", ConceptKind::Function, "x.rs", true);
        let mut b = concept("functions/b", ConceptKind::Function, "x.rs", true);
        let mut self_recursive = concept("functions/c", ConceptKind::Function, "x.rs", true);
        add_edge(&mut a, RelationKind::Calls, "functions/b");
        add_edge(&mut b, RelationKind::Calls, "functions/a");
        add_edge(&mut self_recursive, RelationKind::Calls, "functions/c");

        let concepts = vec![a, b, self_recursive];
        let graph = Graph::build(&concepts);

        let cycles = graph.cycles();
        assert_eq!(
            cycles.len(),
            2,
            "expected the a<->b cycle and the c self-cycle: {cycles:?}"
        );
        assert!(cycles
            .iter()
            .any(|c| c == &vec!["functions/a", "functions/b"]));
        assert!(cycles.iter().any(|c| c == &vec!["functions/c"]));
    }

    #[test]
    fn no_false_positive_cycles_in_a_dag() {
        let mut a = concept("functions/a", ConceptKind::Function, "x.rs", true);
        let b = concept("functions/b", ConceptKind::Function, "x.rs", true);
        add_edge(&mut a, RelationKind::Calls, "functions/b");

        let concepts = vec![a, b];
        let graph = Graph::build(&concepts);
        assert!(graph.cycles().is_empty());
    }

    #[test]
    fn transitive_callers_follows_the_whole_caller_chain() {
        // c -> b -> a (c calls b, b calls a): changing `a` transitively
        // affects both b (direct caller) and c (indirect, through b).
        let mut a = concept("functions/a", ConceptKind::Function, "x.rs", true);
        let mut b = concept("functions/b", ConceptKind::Function, "x.rs", true);
        let c = concept("functions/c", ConceptKind::Function, "x.rs", true);
        add_edge(&mut a, RelationKind::CalledBy, "functions/b");
        add_edge(&mut b, RelationKind::Calls, "functions/a");
        add_edge(&mut b, RelationKind::CalledBy, "functions/c");

        let concepts = vec![a, b, c];
        let graph = Graph::build(&concepts);

        let affected = graph.transitive_callers("functions/a", None);
        assert_eq!(
            affected.iter().map(|c| c.id.as_str()).collect::<Vec<_>>(),
            vec!["functions/b", "functions/c"]
        );
    }

    #[test]
    fn transitive_callers_respects_max_depth() {
        let mut a = concept("functions/a", ConceptKind::Function, "x.rs", true);
        let mut b = concept("functions/b", ConceptKind::Function, "x.rs", true);
        let c = concept("functions/c", ConceptKind::Function, "x.rs", true);
        add_edge(&mut a, RelationKind::CalledBy, "functions/b");
        add_edge(&mut b, RelationKind::CalledBy, "functions/c");

        let concepts = vec![a, b, c];
        let graph = Graph::build(&concepts);

        assert_eq!(
            graph
                .transitive_callers("functions/a", Some(1))
                .iter()
                .map(|c| c.id.as_str())
                .collect::<Vec<_>>(),
            vec!["functions/b"],
            "depth 1 should match direct callers() exactly"
        );
        assert_eq!(
            graph
                .transitive_callers("functions/a", Some(2))
                .iter()
                .map(|c| c.id.as_str())
                .collect::<Vec<_>>(),
            vec!["functions/b", "functions/c"]
        );
    }

    #[test]
    fn transitive_callers_handles_cycles_without_including_the_start_or_looping_forever() {
        let mut a = concept("functions/a", ConceptKind::Function, "x.rs", true);
        let mut b = concept("functions/b", ConceptKind::Function, "x.rs", true);
        add_edge(&mut a, RelationKind::CalledBy, "functions/b");
        add_edge(&mut b, RelationKind::CalledBy, "functions/a");

        let concepts = vec![a, b];
        let graph = Graph::build(&concepts);

        assert_eq!(
            graph
                .transitive_callers("functions/a", None)
                .iter()
                .map(|c| c.id.as_str())
                .collect::<Vec<_>>(),
            vec!["functions/b"],
            "the start concept itself must never appear in its own blast radius"
        );
    }

    #[test]
    fn transitive_callers_is_empty_for_a_concept_with_no_callers() {
        let a = concept("functions/a", ConceptKind::Function, "x.rs", true);
        let concepts = vec![a];
        let graph = Graph::build(&concepts);
        assert!(graph.transitive_callers("functions/a", None).is_empty());
    }

    #[test]
    fn shortest_call_path_finds_indirect_route() {
        let mut a = concept("functions/a", ConceptKind::Function, "x.rs", true);
        let mut b = concept("functions/b", ConceptKind::Function, "x.rs", true);
        let c = concept("functions/c", ConceptKind::Function, "x.rs", true);
        add_edge(&mut a, RelationKind::Calls, "functions/b");
        add_edge(&mut b, RelationKind::Calls, "functions/c");

        let concepts = vec![a, b, c];
        let graph = Graph::build(&concepts);

        let path = graph
            .shortest_call_path("functions/a", "functions/c")
            .unwrap();
        assert_eq!(path, vec!["functions/a", "functions/b", "functions/c"]);
        assert!(graph
            .shortest_call_path("functions/c", "functions/a")
            .is_none());
    }
}
