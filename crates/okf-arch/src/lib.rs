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
use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};

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
    let edges: HashSet<(String, String)> = resolved_package_edges(graph).collect();
    let mut edges: Vec<_> = edges.into_iter().collect();
    edges.sort();
    edges
}

/// For every cross-module dependency edge (`graph.module_dependencies()`),
/// the pair of owning package ids on each side — shared by
/// [`package_dependencies`] (deduplicated, unweighted, directed) and
/// [`weighted_package_edges`] (counted, undirected, for modularity-based
/// community detection) so both derive their edges from exactly one
/// resolution step, instead of two independently maintained copies of
/// the same `owning_package` lookup and same-package skip. A module
/// dependency whose owning package can't be resolved on either side, or
/// that resolves to the same package on both sides, yields nothing.
fn resolved_package_edges<'g>(graph: &'g Graph<'_>) -> impl Iterator<Item = (String, String)> + 'g {
    graph
        .module_dependencies()
        .into_iter()
        .filter_map(|(from_module, to_module)| {
            let (Some(from_pkg), Some(to_pkg)) = (
                graph.owning_package(from_module),
                graph.owning_package(to_module),
            ) else {
                return None;
            };
            if from_pkg.id == to_pkg.id {
                return None;
            }
            Some((from_pkg.id.clone(), to_pkg.id.clone()))
        })
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
        adjacency
            .entry(from.as_str())
            .or_default()
            .insert(to.as_str());
        adjacency
            .entry(to.as_str())
            .or_default()
            .insert(from.as_str());
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

/// A cluster of packages found by [`communities`]'s modularity
/// optimization, as opposed to [`domains`]'s plain connected components:
/// two packages that are technically reachable from each other can still
/// land in different communities here if their connection is weak
/// relative to how strongly each already collaborates within its own
/// cluster — the distinction [`domains`] alone can't draw, since
/// "reachable at all" and "how strongly connected" are different
/// questions. Like [`Domain`], a package with no edges to any other
/// package is still its own singleton community.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Community {
    /// Every package in this community, sorted by id.
    pub package_ids: Vec<String>,
}

/// Package-level dependency edges, weighted by how many distinct
/// module-to-module dependency pairs (`graph.module_dependencies()`)
/// contribute to it — a proxy for "how strongly do these two packages
/// actually collaborate," not just "is there at least one edge," which
/// is all [`package_dependencies`]'s deduplicated pairs can say. Directed
/// module edges collapse into one undirected package-pair weight (a
/// dependency in either direction is still evidence the two packages are
/// linked), keyed by the lexicographically smaller package id first so
/// the same pair always hashes to the same key regardless of which
/// direction was observed.
fn weighted_package_edges(graph: &Graph<'_>) -> HashMap<(String, String), usize> {
    let mut weights: HashMap<(String, String), usize> = HashMap::new();
    for (from, to) in resolved_package_edges(graph) {
        let key = if from <= to { (from, to) } else { (to, from) };
        *weights.entry(key).or_default() += 1;
    }
    weights
}

/// See [`Community`]: modularity-based community detection over the
/// package dependency graph, layered onto [`weighted_package_edges`] the
/// same way [`layers`]/[`domains`] are layered onto
/// [`package_dependencies`]. Sorted by each community's own sorted
/// package ids.
pub fn communities(graph: &Graph<'_>) -> Vec<Community> {
    let packages: Vec<String> = graph
        .of_kind(ConceptKind::Package)
        .iter()
        .map(|c| c.id.clone())
        .collect();
    if packages.is_empty() {
        return Vec::new();
    }
    let edges = weighted_package_edges(graph);
    greedy_modularity_communities(&packages, &edges)
}

/// Greedy modularity-optimization community detection (the
/// Clauset-Newman-Moore agglomerative algorithm): starting with every
/// package as its own community, repeatedly merges whichever pair of
/// communities currently connected by an edge would increase modularity
/// the most, stopping once no remaining merge would improve it.
///
/// Deliberately this single-level agglomerative variant rather than full
/// multi-level Louvain (which additionally aggregates communities into
/// super-nodes and repeats the whole process): a package-dependency graph
/// is small enough in practice that a second aggregation level buys
/// nothing a careful single pass doesn't already find, and skipping it
/// keeps the one property that actually matters here non-negotiable —
/// determinism. Every tie (an equal modularity gain from two different
/// candidate merges) is broken by the lexicographically smaller
/// community-id pair, and nothing here iterates a `HashMap` when order
/// matters, so identical input always produces identical output — see
/// `communities_is_deterministic_across_repeated_runs`.
///
/// Pure and graph-agnostic (only [`communities`] wires this to a real
/// [`Graph`]), so it's directly testable against a hand-built weighted
/// edge map — including the textbook case this whole analysis exists
/// for: two densely-interconnected clusters joined by one thin bridge
/// edge, which [`domains`]'s connected-components approach can't tell
/// apart from one real domain, but a real modularity signal can (see
/// `greedy_modularity_communities_splits_two_dense_clusters_joined_by_a_thin_bridge`).
fn greedy_modularity_communities(
    package_ids: &[String],
    edge_weights: &HashMap<(String, String), usize>,
) -> Vec<Community> {
    let total_weight: f64 = edge_weights.values().map(|&w| w as f64).sum();

    // No cross-package edges at all: every package is its own community
    // (mirrors `domains()`'s singleton behavior for an edgeless graph),
    // and there's nothing to divide by below.
    if total_weight == 0.0 {
        let mut ids: Vec<String> = package_ids.to_vec();
        ids.sort();
        return ids
            .into_iter()
            .map(|id| Community {
                package_ids: vec![id],
            })
            .collect();
    }

    // Community state, keyed by one representative member package id
    // (arbitrary but stable — the id of whichever package started the
    // community, since packages only ever merge into an existing key,
    // never rename one): member package ids, and total edge-weight
    // degree (sum of every edge weight touching any member).
    let mut members: BTreeMap<String, Vec<String>> = package_ids
        .iter()
        .map(|p| (p.clone(), vec![p.clone()]))
        .collect();
    let mut degree: BTreeMap<String, f64> = package_ids.iter().map(|p| (p.clone(), 0.0)).collect();
    for ((a, b), &w) in edge_weights {
        *degree
            .get_mut(a)
            .expect("edge endpoint must be a known package") += w as f64;
        *degree
            .get_mut(b)
            .expect("edge endpoint must be a known package") += w as f64;
    }

    // Community-to-community edge weight, keyed the same
    // smaller-id-first way `edge_weights` already is.
    let mut comm_edges: BTreeMap<(String, String), f64> = edge_weights
        .iter()
        .map(|((a, b), &w)| ((a.clone(), b.clone()), w as f64))
        .collect();

    loop {
        // Modularity gain from merging communities a and b:
        // ΔQ = 2 * (w_ab/m - deg(a)*deg(b) / (2*m^2)). `comm_edges`
        // (a `BTreeMap`) iterates in ascending key order; reversing that
        // to descending means `max_by`'s documented "last element wins a
        // tie" rule picks the lexicographically *smallest* pair on an
        // exact delta tie, matching the deterministic tie-break this
        // algorithm requires.
        let best = comm_edges
            .iter()
            .rev()
            .map(|((a, b), &w)| {
                let delta = 2.0
                    * (w / total_weight
                        - (degree[a] * degree[b]) / (2.0 * total_weight * total_weight));
                ((a.clone(), b.clone()), delta)
            })
            .max_by(|x, y| x.1.total_cmp(&y.1));

        let Some(((a, b), delta)) = best else {
            break;
        };
        if delta <= 0.0 {
            break;
        }

        // Merge b into a (comm_edges keys always have a <= b, by
        // construction above and by how re-keying below preserves it).
        let b_members = members.remove(&b).expect("b must be a live community");
        members
            .get_mut(&a)
            .expect("a must be a live community")
            .extend(b_members);

        let b_degree = degree.remove(&b).expect("b must have a degree entry");
        *degree.get_mut(&a).expect("a must have a degree entry") += b_degree;

        // Re-key every edge touching b onto a, summing weights if a
        // already had its own edge to that third community.
        let touching_b: Vec<((String, String), f64)> = comm_edges
            .iter()
            .filter(|((x, y), _)| x == &b || y == &b)
            .map(|(k, &v)| (k.clone(), v))
            .collect();
        for ((x, y), w) in touching_b {
            comm_edges.remove(&(x.clone(), y.clone()));
            let other = if x == b { y } else { x };
            if other == a {
                continue; // the a-b edge itself; already folded into the merge above
            }
            let key = if a <= other {
                (a.clone(), other.clone())
            } else {
                (other.clone(), a.clone())
            };
            *comm_edges.entry(key).or_insert(0.0) += w;
        }
    }

    let mut result: Vec<Community> = members
        .into_values()
        .map(|mut ids| {
            ids.sort();
            Community { package_ids: ids }
        })
        .collect();
    result.sort_by(|a, b| a.package_ids.cmp(&b.package_ids));
    result
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
        concept.relationships.push(Relationship::new(
            RelationKind::MemberOf,
            package_id,
            package_id,
        ));
    }

    fn calls(concept: &mut Concept, target_id: &str) {
        concept
            .relationships
            .push(Relationship::new(RelationKind::Calls, target_id, target_id));
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
        let callee = concept(
            "functions/core_fn",
            ConceptKind::Function,
            "core/src/lib.rs",
        );

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

    fn edge(weights: &mut HashMap<(String, String), usize>, a: &str, b: &str, weight: usize) {
        let key = if a <= b {
            (a.to_string(), b.to_string())
        } else {
            (b.to_string(), a.to_string())
        };
        *weights.entry(key).or_default() += weight;
    }

    #[test]
    fn greedy_modularity_communities_returns_all_singletons_when_there_are_no_edges() {
        let packages = vec!["packages/a".to_string(), "packages/b".to_string()];
        let result = greedy_modularity_communities(&packages, &HashMap::new());
        assert_eq!(
            result,
            vec![
                Community {
                    package_ids: vec!["packages/a".to_string()]
                },
                Community {
                    package_ids: vec!["packages/b".to_string()]
                },
            ]
        );
    }

    #[test]
    fn greedy_modularity_communities_merges_a_single_connected_pair() {
        let packages = vec!["packages/app".to_string(), "packages/core".to_string()];
        let mut weights = HashMap::new();
        edge(&mut weights, "packages/app", "packages/core", 1);

        let result = greedy_modularity_communities(&packages, &weights);
        assert_eq!(
            result,
            vec![Community {
                package_ids: vec!["packages/app".to_string(), "packages/core".to_string()]
            }]
        );
    }

    /// The textbook case this whole analysis exists for: two triangles
    /// (`a1`/`a2`/`a3` and `b1`/`b2`/`b3`), each densely interconnected,
    /// joined by exactly one thin bridge edge (`a1`-`b1`). `domains()`
    /// (plain connected components) can only report this as one domain
    /// of six packages — every package really is reachable from every
    /// other. A modularity signal can tell the difference: splitting
    /// into the two triangles increases modularity despite losing the
    /// one bridge edge, because each triangle is so much more densely
    /// connected internally than the bridge is externally.
    #[test]
    fn greedy_modularity_communities_splits_two_dense_clusters_joined_by_a_thin_bridge() {
        let packages: Vec<String> = ["a1", "a2", "a3", "b1", "b2", "b3"]
            .iter()
            .map(|s| format!("packages/{s}"))
            .collect();
        let mut weights = HashMap::new();
        for (x, y) in [("a1", "a2"), ("a1", "a3"), ("a2", "a3")] {
            edge(
                &mut weights,
                &format!("packages/{x}"),
                &format!("packages/{y}"),
                10,
            );
        }
        for (x, y) in [("b1", "b2"), ("b1", "b3"), ("b2", "b3")] {
            edge(
                &mut weights,
                &format!("packages/{x}"),
                &format!("packages/{y}"),
                10,
            );
        }
        edge(&mut weights, "packages/a1", "packages/b1", 1);

        let result = greedy_modularity_communities(&packages, &weights);
        assert_eq!(
            result,
            vec![
                Community {
                    package_ids: vec![
                        "packages/a1".to_string(),
                        "packages/a2".to_string(),
                        "packages/a3".to_string(),
                    ]
                },
                Community {
                    package_ids: vec![
                        "packages/b1".to_string(),
                        "packages/b2".to_string(),
                        "packages/b3".to_string(),
                    ]
                },
            ],
            "the two dense triangles must stay separate communities despite the bridge edge"
        );
    }

    #[test]
    fn communities_reports_all_singletons_without_any_package_dependency() {
        let a_pkg = concept("packages/a", ConceptKind::Package, "a/Cargo.toml");
        let b_pkg = concept("packages/b", ConceptKind::Package, "b/Cargo.toml");
        let concepts = vec![a_pkg, b_pkg];
        let graph = Graph::build(&concepts);

        assert_eq!(
            communities(&graph),
            vec![
                Community {
                    package_ids: vec!["packages/a".to_string()]
                },
                Community {
                    package_ids: vec!["packages/b".to_string()]
                },
            ]
        );
    }

    #[test]
    fn communities_is_empty_without_any_package() {
        let no_packages: Vec<Concept> = Vec::new();
        let graph = Graph::build(&no_packages);
        assert!(communities(&graph).is_empty());
    }

    /// The determinism condition this whole feature is gated on: running
    /// community detection repeatedly against the exact same graph must
    /// produce byte-identical output every time, not just an
    /// equivalent-up-to-reordering result.
    #[test]
    fn communities_is_deterministic_across_repeated_runs() {
        let concepts = two_layer_project();
        let graph = Graph::build(&concepts);

        let first = communities(&graph);
        for _ in 0..10 {
            assert_eq!(communities(&graph), first);
        }
    }
}
