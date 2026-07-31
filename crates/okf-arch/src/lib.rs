//! Architecture extraction: architectural layers, domain boundaries,
//! design-pattern detection, and REST-endpoint/database-model/event-flow
//! detection — derived entirely from the package dependency graph
//! (`okf-graph`) and each concept's own id/kind/name, deterministic and
//! offline, no AI dependency (see `okf-enrich` for the separate,
//! optional AI-enrichment pass this deliberately doesn't need).
//!
//! Every analysis here is a structural or naming heuristic over data
//! `okf-analyzer`/`okf-graph` already produce, not a semantic
//! understanding of what a package or type "means" — see each function's
//! own doc comment for exactly what it does and doesn't claim to detect.
//! A project with no `Package` concepts at all (no manifest file
//! `okf-analyzer` recognized) simply has nothing for [`layers`]/
//! [`domains`] to report — an empty result, not an error.

#![deny(missing_docs)]

mod features;
mod ownership;
mod patterns;

pub use features::{detect_features, DetectedFeature, FeatureKind};
pub use patterns::{detect_patterns, DetectedPattern, PatternKind};

use okf_graph::Graph;
use okf_parser::ConceptKind;
use serde::Serialize;
use std::collections::{HashMap, HashSet, VecDeque};

/// One package's position in the layered architecture derived from the
/// package-level dependency graph (packages aggregated from
/// [`okf_graph::Graph::module_dependencies`] via
/// [`okf_graph::Graph::owning_package`]): `layer` 0 is a package with no
/// dependency on any other package in the bundle — the foundation
/// everything else is built on. A package's layer is one more than the
/// highest layer among the packages it directly depends on. Packages in
/// a dependency cycle (mutual package dependencies) share the same
/// layer, since there's no well-defined shorter/longer path between two
/// things that depend on each other.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PackageLayer {
    /// The package's concept id.
    pub package_id: String,
    /// 0 = no dependency on any other package in the bundle.
    pub layer: usize,
}

/// A cluster of packages that depend on each other, directly or
/// transitively — connected components of the *undirected* package
/// dependency graph, the same notion [`okf_graph::Graph::connected_components`]
/// already applies at concept granularity, just one level up. A package
/// with no cross-package dependency in or out is still its own
/// singleton domain (unlike `connected_components`, which omits an
/// analogous singleton concept — a domain of one package is still a
/// complete answer to "what are the domain boundaries," not something to
/// hide). This is a structural signal (which packages actually
/// collaborate), not a semantic one — it doesn't know what a domain
/// "means," only which packages call into which.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Domain {
    /// Every package in this domain, sorted by id.
    pub package_ids: Vec<String>,
}

/// Package-level dependency edges, aggregated from
/// `graph.module_dependencies()` via each module's owning package —
/// deduplicated and sorted for determinism. A module dependency whose
/// owning package can't be resolved on either side (no `Package`
/// concept covers it — the whole bundle, if `okf-analyzer` found no
/// manifest at all) contributes no edge.
pub fn package_dependencies(graph: &Graph<'_>) -> Vec<(String, String)> {
    let mut edges: HashSet<(String, String)> = HashSet::new();
    for (from_module, to_module) in graph.module_dependencies() {
        let (Some(from_pkg), Some(to_pkg)) = (
            graph.owning_package(from_module),
            graph.owning_package(to_module),
        ) else {
            continue;
        };
        if from_pkg.id != to_pkg.id {
            edges.insert((from_pkg.id.clone(), to_pkg.id.clone()));
        }
    }
    let mut edges: Vec<_> = edges.into_iter().collect();
    edges.sort();
    edges
}

/// See [`PackageLayer`]. Sorted by `(layer, package_id)` for
/// deterministic output.
pub fn layers(graph: &Graph<'_>) -> Vec<PackageLayer> {
    let packages: Vec<&str> = graph
        .of_kind(ConceptKind::Package)
        .iter()
        .map(|c| c.id.as_str())
        .collect();
    if packages.is_empty() {
        return Vec::new();
    }
    let edges = package_dependencies(graph);

    let mut deps: HashMap<&str, HashSet<&str>> = HashMap::new();
    for &pkg in &packages {
        deps.entry(pkg).or_default();
    }
    for (from, to) in &edges {
        deps.entry(from.as_str()).or_default().insert(to.as_str());
    }

    let sccs = okf_graph::tarjan_scc(packages.iter().copied(), |node| {
        deps.get(node).into_iter().flatten().copied().collect()
    });
    let scc_of: HashMap<&str, usize> = sccs
        .iter()
        .enumerate()
        .flat_map(|(i, scc)| scc.iter().map(move |&pkg| (pkg, i)))
        .collect();

    let mut condensation: Vec<HashSet<usize>> = vec![HashSet::new(); sccs.len()];
    for (i, scc) in sccs.iter().enumerate() {
        for &pkg in scc {
            for &dep in deps.get(pkg).into_iter().flatten() {
                let dep_scc = scc_of[dep];
                if dep_scc != i {
                    condensation[i].insert(dep_scc);
                }
            }
        }
    }

    let mut memo: Vec<Option<usize>> = vec![None; sccs.len()];
    for i in 0..sccs.len() {
        scc_layer(i, &condensation, &mut memo);
    }

    let mut result: Vec<PackageLayer> = sccs
        .iter()
        .enumerate()
        .flat_map(|(i, scc)| {
            let layer = memo[i].unwrap();
            scc.iter().map(move |&pkg| PackageLayer {
                package_id: pkg.to_string(),
                layer,
            })
        })
        .collect();
    result.sort_by(|a, b| (a.layer, &a.package_id).cmp(&(b.layer, &b.package_id)));
    result
}

/// Longest-path layer of condensation node `i`: 0 if it has no
/// dependencies among the other (already cycle-free, since this is a
/// condensation) nodes, else one more than the deepest of its own
/// dependencies. Memoized since the same node is reached from multiple
/// dependents in a non-trivial condensation.
fn scc_layer(i: usize, condensation: &[HashSet<usize>], memo: &mut [Option<usize>]) -> usize {
    if let Some(layer) = memo[i] {
        return layer;
    }
    let layer = condensation[i]
        .iter()
        .map(|&dep| scc_layer(dep, condensation, memo) + 1)
        .max()
        .unwrap_or(0);
    memo[i] = Some(layer);
    layer
}

/// See [`Domain`]. Sorted by each domain's own sorted package ids.
pub fn domains(graph: &Graph<'_>) -> Vec<Domain> {
    let packages: Vec<&str> = graph
        .of_kind(ConceptKind::Package)
        .iter()
        .map(|c| c.id.as_str())
        .collect();
    if packages.is_empty() {
        return Vec::new();
    }
    let edges = package_dependencies(graph);

    let mut adjacency: HashMap<&str, HashSet<&str>> = HashMap::new();
    for &pkg in &packages {
        adjacency.entry(pkg).or_default();
    }
    for (from, to) in &edges {
        adjacency.entry(from.as_str()).or_default().insert(to.as_str());
        adjacency.entry(to.as_str()).or_default().insert(from.as_str());
    }

    let mut visited: HashSet<&str> = HashSet::new();
    let mut components: Vec<Vec<&str>> = Vec::new();
    for &pkg in &packages {
        if visited.contains(pkg) {
            continue;
        }
        let mut component = Vec::new();
        let mut queue = VecDeque::new();
        queue.push_back(pkg);
        visited.insert(pkg);
        while let Some(current) = queue.pop_front() {
            component.push(current);
            for &neighbor in adjacency.get(current).into_iter().flatten() {
                if visited.insert(neighbor) {
                    queue.push_back(neighbor);
                }
            }
        }
        component.sort();
        components.push(component);
    }
    components.sort();

    components
        .into_iter()
        .map(|ids| Domain {
            package_ids: ids.into_iter().map(String::from).collect(),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use okf_parser::{Concept, ConceptKind, Language, Location, RelationKind, Relationship};

    fn concept(id: &str, kind: ConceptKind, file: &str) -> Concept {
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
            is_public: true,
            generated_at: None,
            relationships: Vec::new(),
        }
    }

    fn member_of(concept: &mut Concept, package_id: &str) {
        concept.relationships.push(Relationship {
            kind: RelationKind::MemberOf,
            target: package_id.to_string(),
            target_display: package_id.to_string(),
        });
    }

    fn calls(concept: &mut Concept, target_id: &str) {
        concept.relationships.push(Relationship {
            kind: RelationKind::Calls,
            target: target_id.to_string(),
            target_display: target_id.to_string(),
        });
    }

    /// Two packages, `core` (no dependencies) and `app` (depends on
    /// `core` via a cross-module `Calls` edge): `app` at layer 1, `core`
    /// at layer 0.
    fn two_layer_project() -> Vec<Concept> {
        let core_pkg = concept("packages/core", ConceptKind::Package, "core/Cargo.toml");
        let app_pkg = concept("packages/app", ConceptKind::Package, "app/Cargo.toml");
        let mut core_mod = concept("modules/core", ConceptKind::Module, "core/src/lib.rs");
        member_of(&mut core_mod, "packages/core");
        let mut app_mod = concept("modules/app", ConceptKind::Module, "app/src/lib.rs");
        member_of(&mut app_mod, "packages/app");

        let mut caller = concept("functions/app_fn", ConceptKind::Function, "app/src/lib.rs");
        calls(&mut caller, "functions/core_fn");
        let callee = concept("functions/core_fn", ConceptKind::Function, "core/src/lib.rs");

        vec![core_pkg, app_pkg, core_mod, app_mod, caller, callee]
    }

    #[test]
    fn layers_a_simple_two_package_dependency() {
        let concepts = two_layer_project();
        let graph = Graph::build(&concepts);
        let mut result = layers(&graph);
        result.sort_by(|a, b| a.package_id.cmp(&b.package_id));

        assert_eq!(
            result,
            vec![
                PackageLayer {
                    package_id: "packages/app".to_string(),
                    layer: 1
                },
                PackageLayer {
                    package_id: "packages/core".to_string(),
                    layer: 0
                },
            ]
        );
    }

    #[test]
    fn layers_collapses_a_package_dependency_cycle_into_one_layer() {
        let a_pkg = concept("packages/a", ConceptKind::Package, "a/Cargo.toml");
        let b_pkg = concept("packages/b", ConceptKind::Package, "b/Cargo.toml");
        let mut a_mod = concept("modules/a", ConceptKind::Module, "a/src/lib.rs");
        member_of(&mut a_mod, "packages/a");
        let mut b_mod = concept("modules/b", ConceptKind::Module, "b/src/lib.rs");
        member_of(&mut b_mod, "packages/b");

        let mut a_fn = concept("functions/a_fn", ConceptKind::Function, "a/src/lib.rs");
        calls(&mut a_fn, "functions/b_fn");
        let mut b_fn = concept("functions/b_fn", ConceptKind::Function, "b/src/lib.rs");
        calls(&mut b_fn, "functions/a_fn");

        let concepts = vec![a_pkg, b_pkg, a_mod, b_mod, a_fn, b_fn];
        let graph = Graph::build(&concepts);
        let result = layers(&graph);

        assert!(
            result.iter().all(|l| l.layer == 0),
            "mutually dependent packages should share one layer: {result:?}"
        );
    }

    #[test]
    fn layers_reports_isolated_packages_at_layer_zero_and_is_empty_without_any_package() {
        let solo = concept("packages/solo", ConceptKind::Package, "solo/Cargo.toml");
        let concepts = vec![solo];
        let graph = Graph::build(&concepts);
        assert_eq!(
            layers(&graph),
            vec![PackageLayer {
                package_id: "packages/solo".to_string(),
                layer: 0
            }]
        );

        let no_packages: Vec<Concept> = Vec::new();
        let empty_graph = Graph::build(&no_packages);
        assert!(layers(&empty_graph).is_empty());
    }

    #[test]
    fn domains_groups_dependent_packages_and_keeps_isolated_ones_as_singletons() {
        let concepts = two_layer_project();
        let graph = Graph::build(&concepts);
        let result = domains(&graph);
        assert_eq!(
            result,
            vec![Domain {
                package_ids: vec!["packages/app".to_string(), "packages/core".to_string()]
            }]
        );

        let solo = concept("packages/solo", ConceptKind::Package, "solo/Cargo.toml");
        let solo_concepts = vec![solo];
        let solo_graph = Graph::build(&solo_concepts);
        assert_eq!(
            domains(&solo_graph),
            vec![Domain {
                package_ids: vec!["packages/solo".to_string()]
            }]
        );
    }

    #[test]
    fn domains_reports_two_unrelated_packages_as_separate_domains() {
        let a_pkg = concept("packages/a", ConceptKind::Package, "a/Cargo.toml");
        let b_pkg = concept("packages/b", ConceptKind::Package, "b/Cargo.toml");
        let concepts = vec![a_pkg, b_pkg];
        let graph = Graph::build(&concepts);

        let result = domains(&graph);
        assert_eq!(
            result,
            vec![
                Domain {
                    package_ids: vec!["packages/a".to_string()]
                },
                Domain {
                    package_ids: vec!["packages/b".to_string()]
                },
            ]
        );
    }
}
