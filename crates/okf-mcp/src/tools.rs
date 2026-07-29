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
        "graph_callers" => okf_query::graph_callers(bundle, &arg_str(arguments, "id")?),
        "graph_callees" => okf_query::graph_callees(bundle, &arg_str(arguments, "id")?),
        "graph_api" => okf_query::graph_api(bundle),
        "graph_cycles" => okf_query::graph_cycles(bundle),
        "graph_modules" => okf_query::graph_modules(bundle),
        "graph_path" => okf_query::graph_path(
            bundle,
            &arg_str(arguments, "from")?,
            &arg_str(arguments, "to")?,
        ),
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
