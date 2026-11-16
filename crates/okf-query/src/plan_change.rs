//! `plan-change`: one structured change plan for a single concept — who
//! calls it, how far a change reaches, which of the affected surface is
//! public API, which tests reach it, and what to inspect first.
//!
//! The contract is `docs/OKF_Agent_Integration_Spec_EN_v1.md` §4.1. In
//! short:
//!
//! - Input is an **exact** concept id. Unlike [`crate::explore`], free
//!   text is never resolved to a "best guess" target: a plan for the
//!   wrong concept is worse than no plan, so an unknown id is an error
//!   that lists ranked candidates instead.
//! - Every field is computed from the graph alone ([`okf_graph::Graph`]
//!   primitives plus the path/name test heuristic below) — no model is
//!   involved, and every list is explicitly sorted and deduplicated, so
//!   the same bundle always produces byte-identical output.
//! - The JSON shape is versioned by [`SCHEMA`]; field order in the JSON
//!   is the declaration order of [`PlanChange`].

use anyhow::{anyhow, Error, Result};
use okf_parser::{Concept, ConceptKind};
use serde::Serialize;
use std::collections::BTreeSet;
use std::path::Path;

/// Schema tag written into every [`PlanChange`] as its `schema` field.
/// Bumped whenever a field is added, removed, or changes meaning.
pub const SCHEMA: &str = "okf-plan-change/1";

/// At most this many entries are returned in
/// [`PlanChange::recommended_inspection`].
const MAX_INSPECTIONS: usize = 5;

/// At most this many ranked candidates are listed when the requested id
/// isn't in the bundle.
const MAX_CANDIDATES: usize = 5;

/// Project-level knobs for the risk rules, read from `okf.toml`'s
/// `[plan_change]` table by the CLI and the MCP server (this crate
/// doesn't read config itself — see the crate-level docs).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlanChangeOptions {
    /// The `large_blast_radius` risk signal fires once
    /// [`PlanChange::affected_concepts`] has at least this many entries.
    /// Default 10.
    pub large_blast_radius: usize,
    /// Module-path segments that mark a sensitive boundary (e.g. `auth`,
    /// `payment`). The `auth_boundary` signal fires when any affected
    /// module id has a path segment equal to one of these, compared
    /// case-insensitively. Default empty, so the signal never fires
    /// unless a project configures it.
    pub boundary_terms: Vec<String>,
}

impl Default for PlanChangeOptions {
    fn default() -> Self {
        PlanChangeOptions {
            large_blast_radius: 10,
            boundary_terms: Vec::new(),
        }
    }
}

/// A change plan for one concept. Field order here is the JSON key order.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PlanChange {
    /// Always [`SCHEMA`].
    pub schema: &'static str,
    /// The concept the plan is for.
    pub target: PlanTarget,
    /// Concepts with a direct `Calls` edge to the target, excluding the
    /// target itself (a self-call is reported by
    /// [`PlanTarget::in_cycle`] instead).
    pub direct_callers: Vec<String>,
    /// [`Self::affected_concepts`] minus [`Self::direct_callers`].
    pub indirect_callers: Vec<String>,
    /// The blast radius: every concept that transitively calls the
    /// target, excluding the target itself.
    pub affected_concepts: Vec<String>,
    /// Concepts the target directly calls, excluding the target itself.
    pub callees: Vec<String>,
    /// Public-API members among the target and
    /// [`Self::affected_concepts`].
    pub public_api: Vec<String>,
    /// The other members of the target's call cycle. Empty when the
    /// target is in no cycle, or is only self-recursive.
    pub cycles: Vec<String>,
    /// Owning module of the target and of every affected concept,
    /// skipping concepts with no owning module.
    pub modules: Vec<String>,
    /// Affected concepts recognized as tests by the path/name heuristic
    /// (see [`Self::limits`] for what it can miss).
    pub tests: Vec<String>,
    /// Risk signals, in the fixed rule order of spec §4.1.6.
    pub risk_signals: Vec<RiskSignal>,
    /// What to read first, at most five entries, in rule order.
    pub recommended_inspection: Vec<Inspection>,
    /// Known limits of this plan (currently: the test heuristic).
    pub limits: Vec<String>,
}

/// The concept a [`PlanChange`] is for.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PlanTarget {
    /// Exact concept id.
    pub id: String,
    /// Concept kind, e.g. `Function` or `Method`.
    pub kind: &'static str,
    /// Owning module id, or `None` when the concept has none.
    pub module: Option<String>,
    /// Whether the target is public API ([`okf_graph::is_public_api`]).
    pub is_public_api: bool,
    /// Whether the target belongs to a call cycle: a strongly-connected
    /// component with more than one member, or a self-recursive call
    /// ([`okf_graph::Graph::cycles`]).
    pub in_cycle: bool,
}

/// One fired risk rule.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RiskSignal {
    /// Rule name, e.g. `public_api`.
    pub signal: &'static str,
    /// Counts behind the rule, from a fixed template.
    pub detail: String,
}

/// One entry of [`PlanChange::recommended_inspection`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Inspection {
    /// Concept id to inspect.
    #[serde(rename = "ref")]
    pub reference: String,
    /// Why, as one of the fixed reason strings of spec §4.1.6.
    pub reason: &'static str,
}

/// [`plan_change_from_concepts`], reading `bundle` fresh.
pub fn plan_change(bundle: &Path, id: &str, options: &PlanChangeOptions) -> Result<PlanChange> {
    let concepts = crate::load_concepts(bundle)?;
    plan_change_from_concepts(&concepts, id, options)
}

/// Builds the change plan for the concept with exact id `id`.
///
/// Errors when `id` isn't in the bundle (listing up to five ranked
/// candidates) or names a structural concept (`Module`, `Package`,
/// `Document`), which has no call edges to plan around.
pub fn plan_change_from_concepts(
    concepts: &[Concept],
    id: &str,
    options: &PlanChangeOptions,
) -> Result<PlanChange> {
    let graph = okf_graph::Graph::build(concepts);
    let Some(target) = graph.get(id) else {
        return Err(unknown_id_error(concepts, id));
    };
    if okf_graph::is_structural(target.kind) {
        return Err(anyhow!(
            "`{id}` is a {} concept; plan-change needs a concept with call edges \
             (a function, method, or type), not a structural one",
            target.kind.as_str()
        ));
    }

    let target_module = graph.owning_module(id).map(|m| m.id.clone());

    let direct_callers = sorted_ids(graph.callers(id), id);
    let callees = sorted_ids(graph.callees(id), id);
    let affected = graph.transitive_callers(id, None);
    let affected_concepts = sorted_ids(affected.iter().copied(), id);
    let direct_set: BTreeSet<&str> = direct_callers.iter().map(String::as_str).collect();
    let indirect_callers: Vec<String> = affected_concepts
        .iter()
        .filter(|c| !direct_set.contains(c.as_str()))
        .cloned()
        .collect();

    let mut public_api: BTreeSet<String> = affected
        .iter()
        .filter(|c| okf_graph::is_public_api(c))
        .map(|c| c.id.clone())
        .collect();
    if okf_graph::is_public_api(target) {
        public_api.insert(target.id.clone());
    }

    // `cycles()` returns every non-trivial SCC (more than one member, or
    // a self-loop) with members already sorted; pick the one containing
    // the target, if any.
    let component = graph.cycles().into_iter().find(|scc| scc.contains(&id));
    let in_cycle = component.is_some();
    let cycles: Vec<String> = component
        .unwrap_or_default()
        .into_iter()
        .filter(|member| *member != id)
        .map(str::to_string)
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();

    let modules: Vec<String> = std::iter::once(id)
        .chain(affected.iter().map(|c| c.id.as_str()))
        .filter_map(|c| graph.owning_module(c).map(|m| m.id.clone()))
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();

    let mut matched_rules: BTreeSet<TestRule> = BTreeSet::new();
    let tests: Vec<String> = affected
        .iter()
        .filter(|c| c.id != id)
        .filter_map(|c| {
            let rules = test_rules(c);
            if rules.is_empty() {
                None
            } else {
                matched_rules.extend(rules);
                Some(c.id.clone())
            }
        })
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();

    let target = PlanTarget {
        id: target.id.clone(),
        kind: target.kind.as_str(),
        module: target_module,
        is_public_api: okf_graph::is_public_api(target),
        in_cycle,
    };

    let mut plan = PlanChange {
        schema: SCHEMA,
        target,
        direct_callers,
        indirect_callers,
        affected_concepts,
        callees,
        public_api: public_api.into_iter().collect(),
        cycles,
        modules,
        tests,
        risk_signals: Vec::new(),
        recommended_inspection: Vec::new(),
        limits: test_limits(&matched_rules),
    };
    plan.risk_signals = risk_signals(&plan, options);
    plan.recommended_inspection = recommended_inspection(&plan, &graph);
    Ok(plan)
}

impl PlanChange {
    /// The plan as pretty-printed JSON (two-space indentation, keys in
    /// [`PlanChange`] declaration order) — the form the MCP server
    /// returns and `okf-rs graph plan-change --format json` prints.
    pub fn to_json(&self) -> String {
        serde_json::to_string_pretty(self)
            .expect("PlanChange contains only strings, bools, and lists, which always serialize")
    }

    /// A human-readable rendering of the same data, for
    /// `okf-rs graph plan-change --format text`. Not a stable format.
    pub fn to_text(&self) -> String {
        let t = &self.target;
        let mut out = format!("Change plan for {} ({})\n", t.id, t.kind);
        out.push_str(&format!(
            "Module: {} · Public API: {} · In a call cycle: {}\n\n",
            t.module.as_deref().unwrap_or("none"),
            yes_no(t.is_public_api),
            yes_no(t.in_cycle),
        ));
        for (label, ids) in [
            ("Direct callers", &self.direct_callers),
            ("Indirect callers", &self.indirect_callers),
            ("Callees", &self.callees),
            ("Public API in scope", &self.public_api),
            ("Other cycle members", &self.cycles),
            ("Modules", &self.modules),
            ("Tests", &self.tests),
        ] {
            out.push_str(&format!("{label} ({}): {}\n", ids.len(), list_or_none(ids)));
        }

        out.push_str("\nRisk signals:\n");
        if self.risk_signals.is_empty() {
            out.push_str("  none\n");
        }
        for s in &self.risk_signals {
            out.push_str(&format!("  - {}: {}\n", s.signal, s.detail));
        }

        out.push_str("\nRecommended inspection:\n");
        if self.recommended_inspection.is_empty() {
            out.push_str("  none\n");
        }
        for i in &self.recommended_inspection {
            out.push_str(&format!("  - {} — {}\n", i.reference, i.reason));
        }

        out.push_str("\nLimits:\n");
        for l in &self.limits {
            out.push_str(&format!("  - {l}\n"));
        }
        out.trim_end().to_string()
    }
}

fn yes_no(b: bool) -> &'static str {
    if b {
        "yes"
    } else {
        "no"
    }
}

fn list_or_none(ids: &[String]) -> String {
    if ids.is_empty() {
        "none".to_string()
    } else {
        ids.join(", ")
    }
}

/// Ids of `concepts`, excluding `exclude`, sorted and deduplicated.
/// `Graph::callers`/`callees` return relationship order, so ordering is
/// never left to the graph.
fn sorted_ids<'a>(concepts: impl IntoIterator<Item = &'a Concept>, exclude: &str) -> Vec<String> {
    concepts
        .into_iter()
        .filter(|c| c.id != exclude)
        .map(|c| c.id.clone())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect()
}

fn unknown_id_error(concepts: &[Concept], id: &str) -> Error {
    let mut message = format!(
        "no concept with id `{id}` (plan-change needs an exact id; use `okf-rs search`/the `search` tool to find one)"
    );
    // Rank by the id's last segment (the symbol name): a full id is full
    // of `/` separators that make a poor free-text query. Any failure
    // here only costs the candidate list, never the error itself.
    let query = id.rsplit('/').next().unwrap_or(id);
    let candidates: Vec<String> = okf_search::FullTextIndex::build_from_concepts(concepts)
        .and_then(|index| index.search(query, MAX_CANDIDATES))
        .map(|hits| hits.into_iter().map(|hit| hit.id).collect())
        .unwrap_or_default();
    if !candidates.is_empty() {
        message.push_str("\ncandidates:");
        for candidate in candidates {
            message.push_str(&format!("\n  - {candidate}"));
        }
    }
    anyhow!(message)
}

/// The three test-detection rules of spec §4.1.5, in reporting order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum TestRule {
    PathSegment,
    NamePattern,
    SymbolPattern,
}

impl TestRule {
    fn as_str(self) -> &'static str {
        match self {
            TestRule::PathSegment => "path_segment",
            TestRule::NamePattern => "name_pattern",
            TestRule::SymbolPattern => "symbol_pattern",
        }
    }
}

/// Which test rules `concept` matches; empty when it isn't a test.
/// Structural concepts are never tests.
fn test_rules(concept: &Concept) -> Vec<TestRule> {
    if okf_graph::is_structural(concept.kind) {
        return Vec::new();
    }
    let file = concept.location.file.as_str();
    let (dirs, basename) = match file.rsplit_once('/') {
        Some((dirs, basename)) => (dirs, basename),
        None => ("", file),
    };
    let mut rules = Vec::new();
    if dirs.split('/').any(|segment| {
        ["tests", "test", "__tests__"]
            .iter()
            .any(|t| segment.eq_ignore_ascii_case(t))
    }) {
        rules.push(TestRule::PathSegment);
    }
    if matches_test_file_name(basename) {
        rules.push(TestRule::NamePattern);
    }
    if matches_test_symbol(concept) {
        rules.push(TestRule::SymbolPattern);
    }
    rules
}

/// `test_*.*`, `*_test.*`, `*.test.*`, `*.spec.*` (glob semantics, so
/// `*` may span dots), or a stem — the part before the first `.` —
/// ending in `Test`/`Tests`. Case-sensitive, so `latest.rs` is not a
/// test file.
fn matches_test_file_name(basename: &str) -> bool {
    let starts_test_prefix = basename
        .strip_prefix("test_")
        .is_some_and(|rest| rest.contains('.'));
    let stem = basename.split('.').next().unwrap_or(basename);
    starts_test_prefix
        || basename.contains("_test.")
        || basename.contains(".test.")
        || basename.contains(".spec.")
        || stem.ends_with("Test")
        || stem.ends_with("Tests")
}

/// A name starting with `test_`, or `Test` followed by an uppercase
/// letter (Go's `TestX`, not `Tester`). Only for code-unit kinds, never
/// variables or constants.
fn matches_test_symbol(concept: &Concept) -> bool {
    let eligible = matches!(
        concept.kind,
        ConceptKind::Function | ConceptKind::Method | ConceptKind::Class | ConceptKind::Struct
    );
    if !eligible {
        return false;
    }
    let name = concept.name.as_str();
    name.starts_with("test_")
        || name
            .strip_prefix("Test")
            .and_then(|rest| rest.chars().next())
            .is_some_and(|c| c.is_ascii_uppercase())
}

fn test_limits(matched: &BTreeSet<TestRule>) -> Vec<String> {
    let first = if matched.is_empty() {
        "tests: heuristic (no affected concept matched a test rule)".to_string()
    } else {
        let rules: Vec<&str> = matched.iter().map(|r| r.as_str()).collect();
        format!("tests: heuristic (rules: {})", rules.join(", "))
    };
    vec![
        first,
        "tests reached only through mocks, dynamic dispatch, or separate binaries are not Calls edges and are not listed".to_string(),
        "test functions declared inline in source files (e.g. Rust #[cfg(test)] modules) are recognized only if their name or file matches a rule".to_string(),
    ]
}

/// Spec §4.1.6 risk rules, in fixed order.
fn risk_signals(plan: &PlanChange, options: &PlanChangeOptions) -> Vec<RiskSignal> {
    let mut signals = Vec::new();
    let t = &plan.target;

    if t.is_public_api {
        signals.push(signal("public_api", "target is public API".to_string()));
    }
    if t.in_cycle {
        let detail = if plan.cycles.is_empty() {
            "target calls itself".to_string()
        } else {
            format!(
                "target is in a call cycle with {} other concept(s)",
                plan.cycles.len()
            )
        };
        signals.push(signal("cycle_member", detail));
    }
    let affected = plan.affected_concepts.len();
    if affected >= options.large_blast_radius {
        signals.push(signal(
            "large_blast_radius",
            format!(
                "{affected} concept(s) transitively depend on the target (threshold {})",
                options.large_blast_radius
            ),
        ));
    }
    if plan.direct_callers.len() >= 2 {
        signals.push(signal(
            "multiple_direct_callers",
            format!("{} direct callers", plan.direct_callers.len()),
        ));
    }
    let other_modules = plan
        .modules
        .iter()
        .filter(|m| Some(m.as_str()) != t.module.as_deref())
        .count();
    if other_modules > 0 {
        let detail = match t.module {
            Some(_) => format!("affects {other_modules} module(s) besides the target's own"),
            None => format!("affects {other_modules} module(s); the target has no owning module"),
        };
        signals.push(signal("module_boundary", detail));
    }
    // Segments after the leading kind directory (`modules/`), so a term
    // can't match the prefix every module id shares.
    let boundary_modules: Vec<&str> = plan
        .modules
        .iter()
        .filter(|m| {
            m.split('/').skip(1).any(|segment| {
                options
                    .boundary_terms
                    .iter()
                    .any(|term| segment.eq_ignore_ascii_case(term))
            })
        })
        .map(String::as_str)
        .collect();
    if !boundary_modules.is_empty() {
        signals.push(signal(
            "auth_boundary",
            format!(
                "module(s) matching a boundary term: {}",
                boundary_modules.join(", ")
            ),
        ));
    }
    if affected > 0 && plan.tests.is_empty() {
        signals.push(signal(
            "no_test_reaches_target",
            format!("no test concept found among {affected} affected concept(s)"),
        ));
    }
    signals
}

fn signal(signal: &'static str, detail: String) -> RiskSignal {
    RiskSignal { signal, detail }
}

/// Spec §4.1.6 inspection rules: each concept appears once, under the
/// first rule that selects it, capped at [`MAX_INSPECTIONS`].
fn recommended_inspection(plan: &PlanChange, graph: &okf_graph::Graph<'_>) -> Vec<Inspection> {
    // `out` never holds more than MAX_INSPECTIONS entries, so a linear
    // scan is the whole dedup.
    fn push(out: &mut Vec<Inspection>, reference: &str, reason: &'static str) {
        if out.len() < MAX_INSPECTIONS && !out.iter().any(|i| i.reference == reference) {
            out.push(Inspection {
                reference: reference.to_string(),
                reason,
            });
        }
    }

    let mut out: Vec<Inspection> = Vec::new();
    // Rule 1: direct callers that are themselves public API.
    for caller in &plan.direct_callers {
        if graph.get(caller).is_some_and(okf_graph::is_public_api) {
            push(&mut out, caller, "direct caller that is itself public API");
        }
    }
    // Rule 2: the sole direct caller.
    if let [only] = plan.direct_callers.as_slice() {
        push(&mut out, only, "sole direct caller");
    }
    // Rule 3: indirect callers in another module (owning modules compared
    // as optional values: both absent counts as the same module).
    let target_module = plan.target.module.as_deref();
    for caller in &plan.indirect_callers {
        let module = graph.owning_module(caller).map(|m| m.id.as_str());
        if module != target_module {
            push(&mut out, caller, "indirect caller in another module");
        }
    }
    // Rule 4: remaining direct callers.
    for caller in &plan.direct_callers {
        push(&mut out, caller, "direct caller");
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use okf_parser::{Language, Location, RelationKind, Relationship};

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

    fn func(id: &str, file: &str, is_public: bool) -> Concept {
        concept(id, ConceptKind::Function, file, is_public)
    }

    fn module(id: &str, file: &str) -> Concept {
        concept(id, ConceptKind::Module, file, true)
    }

    /// Adds a resolved call edge `caller -> callee`, populating both
    /// sides the way real analyzer output does.
    fn call(concepts: &mut [Concept], caller: &str, callee: &str) {
        for c in concepts.iter_mut() {
            if c.id == caller {
                c.relationships
                    .push(Relationship::new(RelationKind::Calls, callee, callee));
            }
            if c.id == callee {
                c.relationships
                    .push(Relationship::new(RelationKind::CalledBy, caller, caller));
            }
        }
    }

    /// The spec §4.1.3 example, as a graph: `login` (auth, public) calls
    /// `authenticate` (auth, public); `test_login` (auth, an inline test
    /// recognized by `symbol_pattern`) calls `login`; `handle` (oauth)
    /// calls `login`; `authenticate` calls `verify_password` and `find`.
    fn example() -> Vec<Concept> {
        let mut concepts = vec![
            module("modules/auth", "src/auth.rs"),
            module("modules/oauth", "src/oauth.rs"),
            func("functions/authenticate", "src/auth.rs", true),
            func("functions/login", "src/auth.rs", true),
            func("functions/test_login", "src/auth.rs", false),
            func("functions/handle", "src/oauth.rs", false),
            func("functions/verify_password", "src/hash.rs", false),
            func("functions/find", "src/store.rs", false),
        ];
        call(&mut concepts, "functions/login", "functions/authenticate");
        call(&mut concepts, "functions/test_login", "functions/login");
        call(&mut concepts, "functions/handle", "functions/login");
        call(
            &mut concepts,
            "functions/authenticate",
            "functions/verify_password",
        );
        call(&mut concepts, "functions/authenticate", "functions/find");
        concepts
    }

    fn plan(concepts: &[Concept], id: &str) -> PlanChange {
        plan_change_from_concepts(concepts, id, &PlanChangeOptions::default()).unwrap()
    }

    fn signal_names(plan: &PlanChange) -> Vec<&'static str> {
        plan.risk_signals.iter().map(|s| s.signal).collect()
    }

    #[test]
    fn computes_every_field_for_the_spec_example() {
        let p = plan(&example(), "functions/authenticate");
        assert_eq!(p.schema, SCHEMA);
        assert_eq!(
            p.target,
            PlanTarget {
                id: "functions/authenticate".into(),
                kind: "Function",
                module: Some("modules/auth".into()),
                is_public_api: true,
                in_cycle: false,
            }
        );
        assert_eq!(p.direct_callers, vec!["functions/login"]);
        assert_eq!(
            p.indirect_callers,
            vec!["functions/handle", "functions/test_login"]
        );
        assert_eq!(
            p.affected_concepts,
            vec![
                "functions/handle",
                "functions/login",
                "functions/test_login"
            ]
        );
        assert_eq!(
            p.callees,
            vec!["functions/find", "functions/verify_password"]
        );
        assert_eq!(
            p.public_api,
            vec!["functions/authenticate", "functions/login"]
        );
        assert!(p.cycles.is_empty());
        assert_eq!(p.modules, vec!["modules/auth", "modules/oauth"]);
        assert_eq!(p.tests, vec!["functions/test_login"]);
        assert_eq!(p.limits[0], "tests: heuristic (rules: symbol_pattern)");
        assert_eq!(signal_names(&p), vec!["public_api", "module_boundary"]);
        assert_eq!(
            p.risk_signals[1].detail,
            "affects 1 module(s) besides the target's own"
        );
        assert_eq!(
            p.recommended_inspection,
            vec![
                Inspection {
                    reference: "functions/login".into(),
                    reason: "direct caller that is itself public API"
                },
                Inspection {
                    reference: "functions/handle".into(),
                    reason: "indirect caller in another module"
                },
            ]
        );
    }

    #[test]
    fn lists_are_sorted_even_when_relationships_are_not() {
        // Callers declared in reverse id order on the target.
        let mut concepts = vec![
            func("functions/t", "src/t.rs", false),
            func("functions/c", "src/c.rs", false),
            func("functions/b", "src/b.rs", false),
            func("functions/a", "src/a.rs", false),
        ];
        call(&mut concepts, "functions/c", "functions/t");
        call(&mut concepts, "functions/b", "functions/t");
        call(&mut concepts, "functions/a", "functions/t");
        let p = plan(&concepts, "functions/t");
        assert_eq!(
            p.direct_callers,
            vec!["functions/a", "functions/b", "functions/c"]
        );
    }

    #[test]
    fn a_two_concept_cycle_lists_the_other_member() {
        let mut concepts = vec![
            func("functions/a", "src/a.rs", false),
            func("functions/b", "src/b.rs", false),
        ];
        call(&mut concepts, "functions/a", "functions/b");
        call(&mut concepts, "functions/b", "functions/a");
        let p = plan(&concepts, "functions/a");
        assert!(p.target.in_cycle);
        assert_eq!(p.cycles, vec!["functions/b"]);
        assert!(signal_names(&p).contains(&"cycle_member"));
        // The target is never its own caller or callee.
        assert_eq!(p.direct_callers, vec!["functions/b"]);
        assert_eq!(p.callees, vec!["functions/b"]);
        assert_eq!(p.affected_concepts, vec!["functions/b"]);
    }

    #[test]
    fn a_self_recursive_target_is_in_a_cycle_with_no_other_members() {
        let mut concepts = vec![func("functions/r", "src/r.rs", false)];
        call(&mut concepts, "functions/r", "functions/r");
        let p = plan(&concepts, "functions/r");
        assert!(p.target.in_cycle);
        assert!(p.cycles.is_empty());
        assert!(p.direct_callers.is_empty());
        assert!(p.callees.is_empty());
        assert_eq!(p.risk_signals[0].signal, "cycle_member");
        assert_eq!(p.risk_signals[0].detail, "target calls itself");
    }

    #[test]
    fn large_blast_radius_and_multiple_direct_callers_follow_the_threshold() {
        let mut concepts = vec![func("functions/t", "src/t.rs", false)];
        for i in 0..3 {
            let id = format!("functions/c{i}");
            concepts.push(func(&id, "src/c.rs", false));
            call(&mut concepts, &id, "functions/t");
        }
        let default = plan(&concepts, "functions/t");
        assert!(!signal_names(&default).contains(&"large_blast_radius"));
        assert!(signal_names(&default).contains(&"multiple_direct_callers"));

        let low = PlanChangeOptions {
            large_blast_radius: 3,
            ..PlanChangeOptions::default()
        };
        let p = plan_change_from_concepts(&concepts, "functions/t", &low).unwrap();
        let large = p
            .risk_signals
            .iter()
            .find(|s| s.signal == "large_blast_radius")
            .unwrap();
        assert_eq!(
            large.detail,
            "3 concept(s) transitively depend on the target (threshold 3)"
        );
    }

    #[test]
    fn boundary_terms_match_whole_path_segments_case_insensitively() {
        let concepts = example();
        let with = |terms: &[&str]| {
            let options = PlanChangeOptions {
                boundary_terms: terms.iter().map(|t| t.to_string()).collect(),
                ..PlanChangeOptions::default()
            };
            plan_change_from_concepts(&concepts, "functions/authenticate", &options).unwrap()
        };
        let p = with(&["AUTH"]);
        let s = p
            .risk_signals
            .iter()
            .find(|s| s.signal == "auth_boundary")
            .unwrap();
        assert_eq!(s.detail, "module(s) matching a boundary term: modules/auth");
        // Whole segments only, and never the shared `modules/` prefix.
        assert!(!signal_names(&with(&["aut"])).contains(&"auth_boundary"));
        assert!(!signal_names(&with(&["modules"])).contains(&"auth_boundary"));
        assert!(!signal_names(&with(&[])).contains(&"auth_boundary"));
    }

    #[test]
    fn module_boundary_counts_modules_when_the_target_has_none() {
        let mut concepts = vec![
            module("modules/a", "src/a.rs"),
            func("functions/t", "src/loose.rs", false),
            func("functions/c", "src/a.rs", false),
        ];
        call(&mut concepts, "functions/c", "functions/t");
        let p = plan(&concepts, "functions/t");
        assert_eq!(p.target.module, None);
        assert_eq!(p.modules, vec!["modules/a"]);
        let s = p
            .risk_signals
            .iter()
            .find(|s| s.signal == "module_boundary")
            .unwrap();
        assert_eq!(
            s.detail,
            "affects 1 module(s); the target has no owning module"
        );
    }

    #[test]
    fn an_unrecognized_inline_test_fires_no_test_reaches_target() {
        // Same shape as the example, but the inline test's name matches
        // no rule -- the heuristic misses it, and the plan says so.
        let mut concepts = vec![
            func("functions/t", "src/lib.rs", false),
            func("functions/parses_input", "src/lib.rs", false),
        ];
        call(&mut concepts, "functions/parses_input", "functions/t");
        let p = plan(&concepts, "functions/t");
        assert!(p.tests.is_empty());
        assert!(signal_names(&p).contains(&"no_test_reaches_target"));
        assert_eq!(
            p.limits[0],
            "tests: heuristic (no affected concept matched a test rule)"
        );
    }

    #[test]
    fn test_rules_each_recognize_their_own_case() {
        let mut concepts = vec![
            func("functions/t", "src/lib.rs", false),
            func("functions/in_tests_dir", "crates/x/Tests/e2e.rs", false),
            func("functions/in_go_file", "pkg/foo_test.go", false),
            func("functions/in_spec_file", "web/foo.spec.ts", false),
            func("functions/in_java_file", "src/FooTest.java", false),
            func("functions/test_parses", "src/lib.rs", false),
            func("functions/TestParse", "pkg/parse.go", false),
            func("functions/Tester", "src/latest.rs", false),
        ];
        let callers: Vec<String> = concepts[1..].iter().map(|c| c.id.clone()).collect();
        for caller in &callers {
            call(&mut concepts, caller, "functions/t");
        }
        let p = plan(&concepts, "functions/t");
        assert_eq!(
            p.tests,
            vec![
                "functions/TestParse",
                "functions/in_go_file",
                "functions/in_java_file",
                "functions/in_spec_file",
                "functions/in_tests_dir",
                "functions/test_parses",
            ]
        );
        assert_eq!(
            p.limits[0],
            "tests: heuristic (rules: path_segment, name_pattern, symbol_pattern)"
        );
        assert!(!signal_names(&p).contains(&"no_test_reaches_target"));
    }

    #[test]
    fn test_file_name_patterns() {
        for yes in [
            "test_foo.py",
            "foo_test.go",
            "foo_test.pb.go",
            "foo.test.ts",
            "foo.spec.ts",
            "FooTest.java",
            "FooTests.cs",
        ] {
            assert!(matches_test_file_name(yes), "{yes} should match");
        }
        for no in ["latest.rs", "contest.py", "test_", "lib.rs", "testing.go"] {
            assert!(!matches_test_file_name(no), "{no} should not match");
        }
    }

    #[test]
    fn structural_concepts_are_never_tests() {
        let m = module("modules/tests/helpers", "tests/helpers.rs");
        assert!(test_rules(&m).is_empty());
    }

    #[test]
    fn recommended_inspection_is_capped_and_deduplicated() {
        let mut concepts = vec![func("functions/t", "src/t.rs", false)];
        for i in 0..7 {
            let id = format!("functions/p{i}");
            concepts.push(func(&id, "src/p.rs", true));
            call(&mut concepts, &id, "functions/t");
        }
        let p = plan(&concepts, "functions/t");
        assert_eq!(p.recommended_inspection.len(), MAX_INSPECTIONS);
        let refs: BTreeSet<&str> = p
            .recommended_inspection
            .iter()
            .map(|i| i.reference.as_str())
            .collect();
        assert_eq!(refs.len(), MAX_INSPECTIONS);
        assert!(p
            .recommended_inspection
            .iter()
            .all(|i| i.reason == "direct caller that is itself public API"));
    }

    #[test]
    fn a_sole_private_direct_caller_is_recommended_as_such() {
        let mut concepts = vec![
            func("functions/t", "src/t.rs", false),
            func("functions/c", "src/t.rs", false),
        ];
        call(&mut concepts, "functions/c", "functions/t");
        let p = plan(&concepts, "functions/t");
        assert_eq!(
            p.recommended_inspection,
            vec![Inspection {
                reference: "functions/c".into(),
                reason: "sole direct caller"
            }]
        );
    }

    #[test]
    fn an_unknown_id_is_an_error_with_candidates() {
        let err = plan_change_from_concepts(
            &example(),
            "functions/authenticat",
            &PlanChangeOptions::default(),
        )
        .unwrap_err()
        .to_string();
        assert!(err.contains("no concept with id `functions/authenticat`"));
        assert!(err.contains("exact id"));
    }

    #[test]
    fn a_structural_id_is_an_error() {
        let err =
            plan_change_from_concepts(&example(), "modules/auth", &PlanChangeOptions::default())
                .unwrap_err()
                .to_string();
        assert!(err.contains("is a Module concept"));
    }

    #[test]
    fn output_is_deterministic_and_keys_follow_the_spec_order() {
        let a = plan(&example(), "functions/authenticate").to_json();
        let b = plan(&example(), "functions/authenticate").to_json();
        assert_eq!(a, b);
        let keys = [
            "\"schema\"",
            "\"target\"",
            "\"direct_callers\"",
            "\"indirect_callers\"",
            "\"affected_concepts\"",
            "\"callees\"",
            "\"public_api\"",
            "\"cycles\"",
            "\"modules\"",
            "\"tests\"",
            "\"risk_signals\"",
            "\"recommended_inspection\"",
            "\"limits\"",
        ];
        let positions: Vec<usize> = keys.iter().map(|k| a.find(k).unwrap()).collect();
        assert!(positions.windows(2).all(|w| w[0] < w[1]));
        assert!(a.contains("\"ref\": \"functions/login\""));
        assert!(a.starts_with("{\n  \"schema\""));
    }

    #[test]
    fn text_rendering_covers_every_section() {
        let text = plan(&example(), "functions/authenticate").to_text();
        assert!(text.starts_with("Change plan for functions/authenticate (Function)"));
        assert!(text.contains("Direct callers (1): functions/login"));
        assert!(text.contains("- public_api: target is public API"));
        assert!(text.contains("- functions/login — direct caller that is itself public API"));
        assert!(text.contains("Limits:"));
    }
}
