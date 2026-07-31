//! Tool definitions and dispatch for the okf-mcp server: MCP's `tools/list`
//! result and the implementation behind `tools/call`, both wrapping the
//! same `okf-query` layer `okf-rs search`/`graph` use from the CLI.

use anyhow::{anyhow, Result};
use serde_json::{json, Value};
use std::path::Path;

/// The MCP `tools/list` result: name, human-readable description, and a
/// JSON Schema for the arguments `tools/call` expects.
pub fn list() -> Vec<Value> {
    vec![
        json!({
            "name": "search",
            "description": "Free-text search over the okf-rs knowledge bundle by symbol, package, module, type, or tag. Use this first to find a concept's id before calling the graph_* tools.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "query": { "type": "string", "description": "Search text" },
                },
                "required": ["query"],
            },
        }),
        json!({
            "name": "explore",
            "description": "One-call context for a concept: signature, description, direct callers, direct callees, blast radius (every concept transitively affected if this one changes), public-API membership, and call-cycle membership. Prefer this over separate search/graph_callers/graph_callees calls when you need more than one of these facts about the same concept — it's the same total information in one round trip instead of several.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "query": { "type": "string", "description": "A concept id, or free text to resolve via ranked search (the top hit is used)" },
                },
                "required": ["query"],
            },
        }),
        json!({
            "name": "search_ranked",
            "description": "Ranked, relevance-scored full-text search over the knowledge bundle (title, type, description, signature, and tags), via Tantivy/BM25. Unlike the search tool's exact/substring matching, this also searches description and signature prose and orders results by relevance — better for a natural-language query (e.g. \"parses a jwt\") than an exact symbol name.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "query": { "type": "string", "description": "Search text" },
                },
                "required": ["query"],
            },
        }),
        json!({
            "name": "graph_callers",
            "description": "List concepts that directly call the given concept id.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "id": { "type": "string", "description": "Concept id, e.g. functions/src/auth/verify_token (find it with the search tool)" },
                },
                "required": ["id"],
            },
        }),
        json!({
            "name": "graph_callees",
            "description": "List concepts the given concept id directly calls.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "id": { "type": "string", "description": "Concept id (find it with the search tool)" },
                },
                "required": ["id"],
            },
        }),
        json!({
            "name": "coverage",
            "description": "Content-completeness metrics for the bundle: percentage of concepts with a description, percentage with at least one tag, and percentage participating in the call graph. Distinct from validation, which is pass/fail rather than a metrics report.",
            "inputSchema": { "type": "object", "properties": {} },
        }),
        json!({
            "name": "graph_api",
            "description": "List the project's public API surface (public functions, methods, and types).",
            "inputSchema": { "type": "object", "properties": {} },
        }),
        json!({
            "name": "graph_cycles",
            "description": "List groups of concepts that call each other in a cycle (direct or mutual recursion).",
            "inputSchema": { "type": "object", "properties": {} },
        }),
        json!({
            "name": "graph_modules",
            "description": "List cross-module call dependency edges: which modules call into which.",
            "inputSchema": { "type": "object", "properties": {} },
        }),
        json!({
            "name": "graph_isolated",
            "description": "List concepts with no Calls/CalledBy edge in either direction (never observed calling anything, and never observed being called) — candidates for dead code or unresolved calls.",
            "inputSchema": { "type": "object", "properties": {} },
        }),
        json!({
            "name": "graph_stats",
            "description": "Graph topology metrics: concept-kind breakdown, relationship edge counts by kind, and connected components of the Calls/CalledBy graph.",
            "inputSchema": { "type": "object", "properties": {} },
        }),
        json!({
            "name": "graph_path",
            "description": "Find the shortest call path between two concept ids.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "from": { "type": "string", "description": "Starting concept id" },
                    "to": { "type": "string", "description": "Target concept id" },
                },
                "required": ["from", "to"],
            },
        }),
        json!({
            "name": "graph_layers",
            "description": "Report each package's layer in the layered architecture derived from the package dependency graph (layer 0 = depends on no other package in the bundle).",
            "inputSchema": { "type": "object", "properties": {} },
        }),
        json!({
            "name": "graph_domains",
            "description": "List domain boundaries: clusters of packages that depend on each other, directly or transitively. A package with no cross-package dependency is its own singleton domain.",
            "inputSchema": { "type": "object", "properties": {} },
        }),
        json!({
            "name": "graph_patterns",
            "description": "List design patterns (Builder, Singleton, Factory, Visitor) detected via structural/naming heuristics — a structural signal to review, not a semantic guarantee the pattern is really implemented as intended.",
            "inputSchema": { "type": "object", "properties": {} },
        }),
        json!({
            "name": "graph_features",
            "description": "List REST endpoints, database models, and event-flow participants detected via structural/naming heuristics (e.g. a public method on a *Controller-named type, a *Model/*Entity-named type, an emit_*/on_*-named function) — a naming-convention signal, not decorator/annotation-aware detection.",
            "inputSchema": { "type": "object", "properties": {} },
        }),
    ]
}

/// Runs `name` with `arguments` against the bundle at `bundle`, returning
/// the text to surface to the model. Errors here become a tool-level
/// error result (`isError: true`), not a JSON-RPC protocol error — a
/// missing bundle or unknown concept id is something the calling agent
/// can react to, not a malformed request.
pub fn call(name: &str, arguments: &Value, bundle: &Path) -> Result<String> {
    match name {
        "search" => okf_query::search(bundle, &arg_str(arguments, "query")?),
        "search_ranked" => okf_query::search_ranked(bundle, &arg_str(arguments, "query")?),
        "explore" => okf_query::explore(bundle, &arg_str(arguments, "query")?),
        "coverage" => okf_query::coverage(bundle),
        "graph_callers" => okf_query::graph_callers(bundle, &arg_str(arguments, "id")?),
        "graph_callees" => okf_query::graph_callees(bundle, &arg_str(arguments, "id")?),
        "graph_api" => okf_query::graph_api(bundle),
        "graph_cycles" => okf_query::graph_cycles(bundle),
        "graph_modules" => okf_query::graph_modules(bundle),
        "graph_isolated" => okf_query::graph_isolated(bundle),
        "graph_stats" => okf_query::graph_stats(bundle),
        "graph_path" => okf_query::graph_path(
            bundle,
            &arg_str(arguments, "from")?,
            &arg_str(arguments, "to")?,
        ),
        "graph_layers" => okf_query::graph_layers(bundle),
        "graph_domains" => okf_query::graph_domains(bundle),
        "graph_patterns" => okf_query::graph_patterns(bundle),
        "graph_features" => okf_query::graph_features(bundle),
        other => Err(anyhow!("unknown tool `{other}`")),
    }
}

fn arg_str(arguments: &Value, key: &str) -> Result<String> {
    arguments
        .get(key)
        .and_then(|v| v.as_str())
        .map(str::to_string)
        .ok_or_else(|| anyhow!("missing required argument `{key}`"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn write(dir: &Path, relative: &str, content: &str) {
        let path = dir.join(relative);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, content).unwrap();
    }

    fn sample_bundle() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        write(
            dir.path(),
            "functions/auth/verify_token.md",
            "---\ntype: Rust Function\ntitle: verify_token\nresource: src/auth.rs#L1\nrelationships:\n  calls:\n    - functions/auth/decode_jwt\n---\n\nbody\n",
        );
        write(
            dir.path(),
            "functions/auth/decode_jwt.md",
            "---\ntype: Rust Function\ntitle: decode_jwt\nresource: src/auth.rs#L5\nrelationships:\n  called_by:\n    - functions/auth/verify_token\n---\n\nbody\n",
        );
        dir
    }

    #[test]
    fn search_finds_a_concept_by_title() {
        let dir = sample_bundle();
        let text = call("search", &json!({ "query": "decode_jwt" }), dir.path()).unwrap();
        assert!(text.contains("decode_jwt"));
        assert!(text.contains("functions/auth/decode_jwt"));
    }

    #[test]
    fn search_ranked_finds_a_concept_by_title() {
        let dir = sample_bundle();
        let text = call(
            "search_ranked",
            &json!({ "query": "decode_jwt" }),
            dir.path(),
        )
        .unwrap();
        assert!(text.contains("functions/auth/decode_jwt"));
    }

    #[test]
    fn graph_layers_domains_patterns_and_features_run_without_error() {
        let dir = sample_bundle();
        // No Package concepts in this fixture -- both should report the
        // clear "none found" text, not error.
        let layers = call("graph_layers", &json!({}), dir.path()).unwrap();
        assert!(layers.contains("No packages found"));
        let domains = call("graph_domains", &json!({}), dir.path()).unwrap();
        assert!(domains.contains("No packages found"));
        let patterns = call("graph_patterns", &json!({}), dir.path()).unwrap();
        assert_eq!(patterns, "No design patterns detected");
        let features = call("graph_features", &json!({}), dir.path()).unwrap();
        assert_eq!(
            features,
            "No REST endpoints, database models, or event-flow participants detected"
        );
    }

    #[test]
    fn explore_bundles_signature_callers_callees_and_blast_radius_in_one_call() {
        let dir = sample_bundle();
        let text = call(
            "explore",
            &json!({ "query": "functions/auth/decode_jwt" }),
            dir.path(),
        )
        .unwrap();
        assert!(text.starts_with("functions/auth/decode_jwt — Rust Function"));
        assert!(text.contains("Callers (1): functions/auth/verify_token"));
        assert!(text.contains("Blast radius"));
    }

    #[test]
    fn graph_callers_and_callees_round_trip() {
        let dir = sample_bundle();
        let callers = call(
            "graph_callers",
            &json!({ "id": "functions/auth/decode_jwt" }),
            dir.path(),
        )
        .unwrap();
        assert!(callers.contains("functions/auth/verify_token"));

        let callees = call(
            "graph_callees",
            &json!({ "id": "functions/auth/verify_token" }),
            dir.path(),
        )
        .unwrap();
        assert!(callees.contains("functions/auth/decode_jwt"));
    }

    #[test]
    fn graph_path_finds_the_direct_edge() {
        let dir = sample_bundle();
        let text = call(
            "graph_path",
            &json!({ "from": "functions/auth/verify_token", "to": "functions/auth/decode_jwt" }),
            dir.path(),
        )
        .unwrap();
        assert_eq!(
            text,
            "functions/auth/verify_token -> functions/auth/decode_jwt"
        );
    }

    #[test]
    fn unknown_concept_id_is_a_clear_error() {
        let dir = sample_bundle();
        let err = call(
            "graph_callers",
            &json!({ "id": "functions/nope" }),
            dir.path(),
        )
        .unwrap_err();
        assert!(err.to_string().contains("no concept with id"));
    }

    #[test]
    fn missing_required_argument_is_a_clear_error() {
        let dir = sample_bundle();
        let err = call("search", &json!({}), dir.path()).unwrap_err();
        assert!(err.to_string().contains("missing required argument"));
    }

    #[test]
    fn unknown_tool_is_a_clear_error() {
        let dir = sample_bundle();
        let err = call("not_a_tool", &json!({}), dir.path()).unwrap_err();
        assert!(err.to_string().contains("unknown tool"));
    }

    #[test]
    fn missing_bundle_points_at_generate() {
        let err = call("graph_api", &json!({}), Path::new("/nonexistent")).unwrap_err();
        assert!(err.to_string().contains("okf-rs generate"));
    }
}
