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

use okf_parser::{Concept, ConceptKind, RelationKind};
use std::collections::{HashMap, HashSet};

pub struct Graph<'a> {
    concepts: &'a [Concept],
    by_id: HashMap<&'a str, usize>,
    /// Module concept id, keyed by source file — lets any concept find
    /// the module that owns it (containment/ownership) without needing
    /// an explicit `MemberOf` relationship.
    module_by_file: HashMap<&'a str, &'a str>,
}

impl<'a> Graph<'a> {
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

    /// Every concept marked public (see [`Concept::is_public`]), sorted
    /// by id for deterministic output. `Module`/`Package` concepts are
    /// excluded — they're structural containers, not API surface.
    pub fn public_api(&self) -> Vec<&'a Concept> {
        let mut public: Vec<&Concept> = self
            .concepts
            .iter()
            .filter(|c| {
                c.is_public && !matches!(c.kind, ConceptKind::Module | ConceptKind::Package)
            })
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
        let mut tarjan = Tarjan::new(self);
        for concept in self.concepts {
            if !tarjan.index.contains_key(concept.id.as_str()) {
                tarjan.visit(&concept.id);
            }
        }
        let mut cycles: Vec<Vec<&str>> = tarjan
            .sccs
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

/// Standard recursive Tarjan's SCC algorithm, scoped to the `Calls` graph.
struct Tarjan<'a, 'g> {
    graph: &'g Graph<'a>,
    index_counter: usize,
    index: HashMap<&'a str, usize>,
    lowlink: HashMap<&'a str, usize>,
    on_stack: HashSet<&'a str>,
    stack: Vec<&'a str>,
    sccs: Vec<Vec<&'a str>>,
}

impl<'a, 'g> Tarjan<'a, 'g> {
    fn new(graph: &'g Graph<'a>) -> Self {
        Tarjan {
            graph,
            index_counter: 0,
            index: HashMap::new(),
            lowlink: HashMap::new(),
            on_stack: HashSet::new(),
            stack: Vec::new(),
            sccs: Vec::new(),
        }
    }

    fn visit(&mut self, id: &str) {
        let Some(concept) = self.graph.get(id) else {
            return;
        };
        let id = concept.id.as_str();

        self.index.insert(id, self.index_counter);
        self.lowlink.insert(id, self.index_counter);
        self.index_counter += 1;
        self.stack.push(id);
        self.on_stack.insert(id);

        for callee in self.graph.callees(id) {
            let callee_id = callee.id.as_str();
            if !self.index.contains_key(callee_id) {
                self.visit(callee_id);
                let callee_low = self.lowlink[callee_id];
                let my_low = self.lowlink[id];
                self.lowlink.insert(id, my_low.min(callee_low));
            } else if self.on_stack.contains(callee_id) {
                let callee_idx = self.index[callee_id];
                let my_low = self.lowlink[id];
                self.lowlink.insert(id, my_low.min(callee_idx));
            }
        }

        if self.lowlink[id] == self.index[id] {
            let mut scc = Vec::new();
            loop {
                let member = self.stack.pop().unwrap();
                self.on_stack.remove(member);
                scc.push(member);
                if member == id {
                    break;
                }
            }
            self.sccs.push(scc);
        }
    }
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
