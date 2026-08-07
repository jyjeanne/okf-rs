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
            "name": "search_semantic",
            "description": "Ranks concepts by embedding-cosine similarity to the query (\"find by meaning\") instead of exact/substring or lexical-relevance matching, via an OpenAI-compatible /embeddings endpoint configured through this server's OKF_ENRICH_BASE_URL/OKF_ENRICH_MODEL(/OKF_ENRICH_API_KEY) environment variables. Only concepts with a description are considered — run `generate --enrich` first if the bundle has none. Errors clearly if the endpoint isn't configured; prefer search/search_ranked unless you specifically need meaning-based matching.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "query": { "type": "string", "description": "Search text" },
                    "limit": { "type": "integer", "description": "Maximum hits to return (default 25)" },
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
            "name": "coverage",
            "description": "Content-completeness metrics for the bundle: percentage of concepts with a description, percentage with at least one tag, and percentage participating in the call graph. Distinct from validation, which is pass/fail rather than a metrics report.",
            "inputSchema": { "type": "object", "properties": {} },
        }),
        json!({
            "name": "graph",
            "description": "Unified entry point for graph-topology and architecture queries — one tool covering what used to be a dozen-plus single-purpose graph_* tools, to keep the schema this server contributes to every session's system prompt small. Pick a `relation`:\n- callers (needs `id`): concepts that directly call `id`\n- callees (needs `id`): concepts `id` directly calls\n- path (needs `from`, `to`): shortest call path between two concept ids\n- explain (needs `from`, `to`): why a relationship exists between two concepts — the relation kind plus a human-readable reason derived from its provenance (e.g. \"resolved via Tree-sitter's unambiguous name match\", or which language server resolved it); falls back to explaining the shortest call path hop-by-hop when there's no single direct relationship\n- api: the project's public API surface (public functions, methods, and types)\n- cycles: groups of concepts that call each other in a cycle (direct or mutual recursion)\n- modules: cross-module call dependency edges\n- isolated: concepts with no Calls/CalledBy edge in either direction — candidates for dead code or unresolved calls\n- stats: concept-kind breakdown, relationship edge counts by kind, and connected components of the Calls/CalledBy graph\n- layers: each package's layer in the package dependency graph (layer 0 = depends on no other package in the bundle)\n- domains: clusters of packages that depend on each other, directly or transitively\n- communities: package communities from modularity-optimization detection — finer-grained than domains\n- patterns: design patterns (Builder, Singleton, Factory, Visitor) detected via structural/naming heuristics — a signal to review, not a guarantee\n- features: REST endpoints, database models, and event-flow participants detected via naming heuristics (e.g. a *Controller-named type, an emit_*-named function)",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "relation": {
                        "type": "string",
                        "enum": ["callers", "callees", "path", "explain", "api", "cycles", "modules", "isolated", "stats", "layers", "domains", "communities", "patterns", "features"],
                        "description": "Which graph query to run",
                    },
                    "id": { "type": "string", "description": "Concept id — required for relation=callers|callees (find it with the search tool)" },
                    "from": { "type": "string", "description": "Starting concept id — required for relation=path|explain" },
                    "to": { "type": "string", "description": "Target concept id — required for relation=path|explain" },
                },
                "required": ["relation"],
            },
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
        "search_semantic" => {
            let query = arg_str(arguments, "query")?;
            let limit = arguments
                .get("limit")
                .and_then(|v| v.as_u64())
                .unwrap_or(25) as usize;
            let config = enrich_config_from_env()?;
            let client = okf_enrich::EnrichClient::new(config);
            okf_query::search_semantic(bundle, &client, &query, limit)
        }
        "explore" => okf_query::explore(bundle, &arg_str(arguments, "query")?),
        "coverage" => okf_query::coverage(bundle),
        "graph" => graph_relation(bundle, arguments),
        other => Err(anyhow!("unknown tool `{other}`")),
    }
}

/// Dispatches the consolidated `graph` tool's `relation` argument to the
/// matching `okf-query` function — the single point every former
/// `graph_*` tool now funnels through, so the MCP schema itself only
/// grows by one `enum` variant per new graph query instead of one whole
/// tool (with its own name and JSON Schema, repeated in every session's
/// system prompt) per query.
fn graph_relation(bundle: &Path, arguments: &Value) -> Result<String> {
    let relation = arg_str(arguments, "relation")?;
    match relation.as_str() {
        "callers" => okf_query::graph_callers(bundle, &arg_str(arguments, "id")?),
        "callees" => okf_query::graph_callees(bundle, &arg_str(arguments, "id")?),
        "path" => okf_query::graph_path(
            bundle,
            &arg_str(arguments, "from")?,
            &arg_str(arguments, "to")?,
        ),
        "explain" => okf_query::explain(
            bundle,
            &arg_str(arguments, "from")?,
            &arg_str(arguments, "to")?,
        ),
        "api" => okf_query::graph_api(bundle),
        "cycles" => okf_query::graph_cycles(bundle),
        "modules" => okf_query::graph_modules(bundle),
        "isolated" => okf_query::graph_isolated(bundle),
        "stats" => okf_query::graph_stats(bundle),
        "layers" => okf_query::graph_layers(bundle),
        "domains" => okf_query::graph_domains(bundle),
        "communities" => okf_query::graph_communities(bundle),
        "patterns" => okf_query::graph_patterns(bundle),
        "features" => okf_query::graph_features(bundle),
        other => Err(anyhow!(
            "unknown relation `{other}` for the graph tool — expected one of: callers, callees, path, explain, api, cycles, modules, isolated, stats, layers, domains, communities, patterns, features"
        )),
    }
}

fn arg_str(arguments: &Value, key: &str) -> Result<String> {
    arguments
        .get(key)
        .and_then(|v| v.as_str())
        .map(str::to_string)
        .ok_or_else(|| anyhow!("missing required argument `{key}`"))
}

/// Resolves an embedding-endpoint config from `OKF_ENRICH_BASE_URL`/
/// `OKF_ENRICH_MODEL`/`OKF_ENRICH_API_KEY` — the same environment
/// variables `okf-rs generate --enrich`/`search --semantic` already fall
/// back to, but the only source of config available here: an MCP tool
/// call has no equivalent of a CLI flag, and this server is started with
/// just a project root (see `main.rs`), so there's nowhere else for a
/// client to pass endpoint settings through.
fn enrich_config_from_env() -> Result<okf_enrich::EnrichConfig> {
    let base_url = std::env::var("OKF_ENRICH_BASE_URL").map_err(|_| {
        anyhow!("search_semantic requires the OKF_ENRICH_BASE_URL environment variable to be set for this server")
    })?;
    let model = std::env::var("OKF_ENRICH_MODEL").map_err(|_| {
        anyhow!("search_semantic requires the OKF_ENRICH_MODEL environment variable to be set for this server")
    })?;
    let api_key = std::env::var("OKF_ENRICH_API_KEY").ok();
    Ok(okf_enrich::EnrichConfig {
        base_url,
        model,
        api_key,
    })
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
        let layers = call("graph", &json!({ "relation": "layers" }), dir.path()).unwrap();
        assert!(layers.contains("No packages found"));
        let domains = call("graph", &json!({ "relation": "domains" }), dir.path()).unwrap();
        assert!(domains.contains("No packages found"));
        let patterns = call("graph", &json!({ "relation": "patterns" }), dir.path()).unwrap();
        assert_eq!(patterns, "No design patterns detected");
        let features = call("graph", &json!({ "relation": "features" }), dir.path()).unwrap();
        assert_eq!(
            features,
            "No REST endpoints, database models, or event-flow participants detected"
        );
    }

    #[test]
    fn search_semantic_without_an_endpoint_configured_is_a_clear_error() {
        // No OKF_ENRICH_BASE_URL/MODEL set in this test process -- the
        // tool must report exactly why, not fail some other way.
        std::env::remove_var("OKF_ENRICH_BASE_URL");
        std::env::remove_var("OKF_ENRICH_MODEL");
        let dir = sample_bundle();
        let err = call("search_semantic", &json!({ "query": "token" }), dir.path()).unwrap_err();
        assert!(err.to_string().contains("OKF_ENRICH_BASE_URL"));
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
            "graph",
            &json!({ "relation": "callers", "id": "functions/auth/decode_jwt" }),
            dir.path(),
        )
        .unwrap();
        assert!(callers.contains("functions/auth/verify_token"));

        let callees = call(
            "graph",
            &json!({ "relation": "callees", "id": "functions/auth/verify_token" }),
            dir.path(),
        )
        .unwrap();
        assert!(callees.contains("functions/auth/decode_jwt"));
    }

    #[test]
    fn graph_path_finds_the_direct_edge() {
        let dir = sample_bundle();
        let text = call(
            "graph",
            &json!({ "relation": "path", "from": "functions/auth/verify_token", "to": "functions/auth/decode_jwt" }),
            dir.path(),
        )
        .unwrap();
        assert_eq!(
            text,
            "functions/auth/verify_token -> functions/auth/decode_jwt"
        );
    }

    #[test]
    fn graph_explain_renders_the_relation_and_a_reason() {
        let dir = sample_bundle();
        let text = call(
            "graph",
            &json!({ "relation": "explain", "from": "functions/auth/verify_token", "to": "functions/auth/decode_jwt" }),
            dir.path(),
        )
        .unwrap();
        assert!(text.starts_with("functions/auth/verify_token"));
        assert!(text.contains("calls"));
        assert!(text.contains("Reason:"));
    }

    #[test]
    fn unknown_concept_id_is_a_clear_error() {
        let dir = sample_bundle();
        let err = call(
            "graph",
            &json!({ "relation": "callers", "id": "functions/nope" }),
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
    fn unknown_relation_on_the_graph_tool_is_a_clear_error() {
        let dir = sample_bundle();
        let err = call("graph", &json!({ "relation": "bogus" }), dir.path()).unwrap_err();
        assert!(err.to_string().contains("unknown relation `bogus`"));
    }

    #[test]
    fn unknown_tool_is_a_clear_error() {
        let dir = sample_bundle();
        let err = call("not_a_tool", &json!({}), dir.path()).unwrap_err();
        assert!(err.to_string().contains("unknown tool"));
    }

    #[test]
    fn missing_bundle_points_at_generate() {
        let err = call(
            "graph",
            &json!({ "relation": "api" }),
            Path::new("/nonexistent"),
        )
        .unwrap_err();
        assert!(err.to_string().contains("okf-rs generate"));
    }
}
