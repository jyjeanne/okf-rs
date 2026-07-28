//! Tool definitions and dispatch for the okf-mcp server: MCP's `tools/list`
//! result and the implementation behind `tools/call`, both wrapping
//! `okf-search`/`okf-graph`/`okf_parser::read_bundle` the same way
//! `okf-rs search`/`graph` do from the CLI.

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
        "search" => search(arguments, bundle),
        "graph_callers" => graph_callers(arguments, bundle),
        "graph_callees" => graph_callees(arguments, bundle),
        "graph_api" => graph_api(bundle),
        "graph_cycles" => graph_cycles(bundle),
        "graph_modules" => graph_modules(bundle),
        "graph_path" => graph_path(arguments, bundle),
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

fn require_bundle(bundle: &Path) -> Result<()> {
    if bundle.is_dir() {
        Ok(())
    } else {
        Err(anyhow!(
            "no bundle found at {} — run `okf-rs generate` first",
            bundle.display()
        ))
    }
}

fn load_concepts(bundle: &Path) -> Result<Vec<okf_parser::Concept>> {
    require_bundle(bundle)?;
    okf_parser::read_bundle(bundle)
}

fn require_concept<'a>(graph: &okf_graph::Graph<'a>, id: &str) -> Result<()> {
    if graph.get(id).is_some() {
        Ok(())
    } else {
        Err(anyhow!(
            "no concept with id `{id}` (use the `search` tool to find valid ids)"
        ))
    }
}

fn search(arguments: &Value, bundle: &Path) -> Result<String> {
    let query = arg_str(arguments, "query")?;
    require_bundle(bundle)?;
    let index = okf_search::SearchIndex::build(bundle)?;
    let hits = index.search(&query);
    if hits.is_empty() {
        return Ok(format!("No matches for `{query}`."));
    }
    let mut out = String::new();
    for hit in hits {
        out.push_str(&format!(
            "{:>3}  {:<24} {:<20} {}\n",
            hit.score, hit.entry.title, hit.entry.concept_type, hit.entry.id
        ));
    }
    Ok(out)
}

fn graph_callers(arguments: &Value, bundle: &Path) -> Result<String> {
    let id = arg_str(arguments, "id")?;
    let concepts = load_concepts(bundle)?;
    let graph = okf_graph::Graph::build(&concepts);
    require_concept(&graph, &id)?;
    let callers = graph.callers(&id);
    if callers.is_empty() {
        return Ok(format!("No callers found for `{id}`."));
    }
    Ok(concept_lines(&callers))
}

fn graph_callees(arguments: &Value, bundle: &Path) -> Result<String> {
    let id = arg_str(arguments, "id")?;
    let concepts = load_concepts(bundle)?;
    let graph = okf_graph::Graph::build(&concepts);
    require_concept(&graph, &id)?;
    let callees = graph.callees(&id);
    if callees.is_empty() {
        return Ok(format!(
            "`{id}` doesn't call anything (or only calls unresolved/ambiguous targets)."
        ));
    }
    Ok(concept_lines(&callees))
}

fn graph_api(bundle: &Path) -> Result<String> {
    let concepts = load_concepts(bundle)?;
    let graph = okf_graph::Graph::build(&concepts);
    let api = graph.public_api();
    if api.is_empty() {
        return Ok("No public concepts found.".to_string());
    }
    let mut out = format!("{} public concepts:\n", api.len());
    for concept in api {
        out.push_str(&format!(
            "  {:<12} {}\n",
            concept.frontmatter_type(),
            concept.id
        ));
    }
    Ok(out)
}

fn graph_cycles(bundle: &Path) -> Result<String> {
    let concepts = load_concepts(bundle)?;
    let graph = okf_graph::Graph::build(&concepts);
    let cycles = graph.cycles();
    if cycles.is_empty() {
        return Ok("No cycles found in the call graph.".to_string());
    }
    Ok(cycles
        .into_iter()
        .map(|cycle| cycle.join(" -> "))
        .collect::<Vec<_>>()
        .join("\n"))
}

fn graph_modules(bundle: &Path) -> Result<String> {
    let concepts = load_concepts(bundle)?;
    let graph = okf_graph::Graph::build(&concepts);
    let deps = graph.module_dependencies();
    if deps.is_empty() {
        return Ok("No cross-module call dependencies found.".to_string());
    }
    Ok(deps
        .into_iter()
        .map(|(from, to)| format!("{from} -> {to}"))
        .collect::<Vec<_>>()
        .join("\n"))
}

fn graph_path(arguments: &Value, bundle: &Path) -> Result<String> {
    let from = arg_str(arguments, "from")?;
    let to = arg_str(arguments, "to")?;
    let concepts = load_concepts(bundle)?;
    let graph = okf_graph::Graph::build(&concepts);
    require_concept(&graph, &from)?;
    require_concept(&graph, &to)?;
    match graph.shortest_call_path(&from, &to) {
        Some(steps) => Ok(steps.join(" -> ")),
        None => Ok(format!("No call path found from `{from}` to `{to}`.")),
    }
}

fn concept_lines(concepts: &[&okf_parser::Concept]) -> String {
    concepts
        .iter()
        .map(|c| format!("{} — {}", c.id, c.frontmatter_type()))
        .collect::<Vec<_>>()
        .join("\n")
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
