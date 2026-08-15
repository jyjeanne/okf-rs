//! Minimal Model Context Protocol (MCP) server exposing okf-rs bundle
//! queries (search, call graph, API surface) to AI agents.
//!
//! Speaks JSON-RPC 2.0 over stdio, one message per line (MCP's stdio
//! transport): requests are read from stdin, responses written to stdout.
//! Notifications (messages with no `id`) never get a response, even on
//! error, per the JSON-RPC spec. All non-protocol output (parse errors,
//! diagnostics) goes to stderr — stdout is reserved for protocol messages
//! only, since a stray `println!` would corrupt the stream for whatever
//! is reading it.
//!
//! The server is bound to a single project at startup (its first non-flag
//! argument, or the current directory), the same way `okf-rs search`/
//! `validate`/`graph` resolve a bundle: an explicit bundle path wins,
//! otherwise `okf.toml`'s `output` under the project root, otherwise
//! `knowledge`. Concept-consuming tool calls go through a per-process
//! [`cache::BundleCache`] rather than re-parsing the bundle from scratch
//! every time — see that module's docs for the freshness guarantee this
//! still makes: a `generate` run between two calls is always picked up,
//! without needing a restart, the same as before the cache existed.
//! `search`/`search_ranked` build their own index straight from the
//! bundle path and aren't cached.
//!
//! `--benchmark` skips the stdio JSON-RPC loop entirely and instead
//! prints a one-shot local session-level cost report to stdout, then
//! exits — see [`benchmark`] for what it measures and why.

mod benchmark;
mod cache;
mod tool_selection_benchmark;
mod tools;

use anyhow::Result;
use serde_json::{json, Value};
use std::io::{self, BufRead, Write};
use std::path::PathBuf;

const PROTOCOL_VERSION: &str = "2024-11-05";

/// Up to how many sampled concepts `--benchmark` reports on — enough to
/// average over more than one data point without making a diagnostic
/// command slow on a large bundle (each sample re-walks the whole project
/// source tree once, per concept, to compute its naive grep-and-read
/// cost).
const BENCHMARK_SAMPLE_SIZE: usize = 5;

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let benchmark_mode = args.iter().any(|a| a == "--benchmark");
    let project_root = args
        .into_iter()
        .find(|a| a != "--benchmark")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."));
    let project_root = project_root
        .canonicalize()
        .unwrap_or_else(|_| project_root.clone());

    if benchmark_mode {
        let report = benchmark::run(&project_root, BENCHMARK_SAMPLE_SIZE)?;
        print!("{}", report.render());
        return Ok(());
    }

    let bundle = okf_core::config::resolve_bundle(&project_root, None);
    let cache = cache::BundleCache::new();

    let stdin = io::stdin();
    let mut stdout = io::stdout();

    for line in stdin.lock().lines() {
        let line = match line {
            Ok(line) => line,
            Err(e) => {
                eprintln!("okf-mcp: failed to read request line: {e}");
                continue;
            }
        };
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let request: Value = match serde_json::from_str(line) {
            Ok(v) => v,
            Err(e) => {
                eprintln!("okf-mcp: failed to parse request: {e}");
                continue;
            }
        };
        if let Some(response) = handle_message(&request, &bundle, &cache) {
            writeln!(stdout, "{}", serde_json::to_string(&response)?)?;
            stdout.flush()?;
        }
    }
    Ok(())
}

/// Dispatches one JSON-RPC message, returning the response to write (or
/// `None` for notifications, which never get one). `cache` amortizes
/// bundle parsing across calls within this one server process — see the
/// `cache` module for the freshness guarantee it makes.
fn handle_message(
    request: &Value,
    bundle: &std::path::Path,
    cache: &cache::BundleCache,
) -> Option<Value> {
    let method = request.get("method")?.as_str()?;
    let id = request.get("id").cloned();

    let result = match method {
        "initialize" => Ok(json!({
            "protocolVersion": PROTOCOL_VERSION,
            "capabilities": { "tools": {} },
            "serverInfo": { "name": "okf-mcp", "version": env!("CARGO_PKG_VERSION") },
        })),
        "ping" => Ok(json!({})),
        "tools/list" => Ok(json!({ "tools": tools::list() })),
        "tools/call" => {
            let params = request.get("params").cloned().unwrap_or(Value::Null);
            let name = params.get("name").and_then(|v| v.as_str()).unwrap_or("");
            let arguments = params
                .get("arguments")
                .cloned()
                .unwrap_or_else(|| json!({}));
            match tools::call(name, &arguments, bundle, cache) {
                Ok(text) => Ok(json!({
                    "content": [{ "type": "text", "text": text }],
                    "isError": false,
                })),
                Err(e) => Ok(json!({
                    "content": [{ "type": "text", "text": e.to_string() }],
                    "isError": true,
                })),
            }
        }
        // Notifications this server doesn't need to act on.
        "notifications/initialized"
        | "notifications/cancelled"
        | "notifications/roots/list_changed" => {
            return None;
        }
        other => Err(format!("method not found: {other}")),
    };

    // A notification (no `id`) never gets a response, even on error.
    let id = id?;
    Some(match result {
        Ok(value) => json!({ "jsonrpc": "2.0", "id": id, "result": value }),
        Err(message) => json!({
            "jsonrpc": "2.0",
            "id": id,
            "error": { "code": -32601, "message": message },
        }),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn write(dir: &std::path::Path, relative: &str, content: &str) {
        let path = dir.join(relative);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, content).unwrap();
    }

    #[test]
    fn initialize_reports_capabilities() {
        let response = handle_message(
            &json!({ "jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {} }),
            std::path::Path::new("knowledge"),
            &cache::BundleCache::new(),
        )
        .unwrap();
        assert_eq!(response["result"]["protocolVersion"], PROTOCOL_VERSION);
        assert!(response["result"]["capabilities"]["tools"].is_object());
    }

    #[test]
    fn notifications_get_no_response() {
        let response = handle_message(
            &json!({ "jsonrpc": "2.0", "method": "notifications/initialized" }),
            std::path::Path::new("knowledge"),
            &cache::BundleCache::new(),
        );
        assert!(response.is_none());
    }

    #[test]
    fn unknown_method_on_a_request_errors_but_unknown_notification_is_silent() {
        let response = handle_message(
            &json!({ "jsonrpc": "2.0", "id": 1, "method": "bogus" }),
            std::path::Path::new("knowledge"),
            &cache::BundleCache::new(),
        )
        .unwrap();
        assert_eq!(response["error"]["code"], -32601);

        let response = handle_message(
            &json!({ "jsonrpc": "2.0", "method": "notifications/bogus" }),
            std::path::Path::new("knowledge"),
            &cache::BundleCache::new(),
        );
        assert!(response.is_none());
    }

    #[test]
    fn tools_list_includes_search_and_graph_tools() {
        let response = handle_message(
            &json!({ "jsonrpc": "2.0", "id": 1, "method": "tools/list" }),
            std::path::Path::new("knowledge"),
            &cache::BundleCache::new(),
        )
        .unwrap();
        let names: Vec<&str> = response["result"]["tools"]
            .as_array()
            .unwrap()
            .iter()
            .map(|t| t["name"].as_str().unwrap())
            .collect();
        assert!(names.contains(&"search"));
        assert!(names.contains(&"explore"));
        assert!(names.contains(&"coverage"));
        assert!(names.contains(&"graph"));
        assert!(names.contains(&"search_semantic"));
        // The consolidated `graph` tool replaces what used to be one
        // MCP tool per relation (graph_callers, graph_api, ...) -- assert
        // those names are gone, not just that `graph` is present, so a
        // regression that reintroduces the old sprawl is caught here.
        assert!(!names.contains(&"graph_callers"));
        assert!(!names.contains(&"graph_api"));
    }

    #[test]
    fn graph_tool_schema_lists_every_relation_and_requires_relation_only() {
        let response = handle_message(
            &json!({ "jsonrpc": "2.0", "id": 1, "method": "tools/list" }),
            std::path::Path::new("knowledge"),
            &cache::BundleCache::new(),
        )
        .unwrap();
        let graph_tool = response["result"]["tools"]
            .as_array()
            .unwrap()
            .iter()
            .find(|t| t["name"] == "graph")
            .unwrap();
        let relations: Vec<&str> = graph_tool["inputSchema"]["properties"]["relation"]["enum"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap())
            .collect();
        for expected in [
            "callers",
            "callees",
            "path",
            "explain",
            "api",
            "cycles",
            "modules",
            "isolated",
            "stats",
            "layers",
            "domains",
            "communities",
            "patterns",
            "features",
        ] {
            assert!(
                relations.contains(&expected),
                "missing relation `{expected}`"
            );
        }
        assert_eq!(graph_tool["inputSchema"]["required"], json!(["relation"]));
    }

    #[test]
    fn tools_call_runs_search_against_the_bundle() {
        let dir = tempfile::tempdir().unwrap();
        write(
            dir.path(),
            "functions/verify_token.md",
            "---\ntype: Rust Function\ntitle: verify_token\nresource: src/main.rs#L1\n---\n\nbody\n",
        );

        let response = handle_message(
            &json!({
                "jsonrpc": "2.0",
                "id": 7,
                "method": "tools/call",
                "params": { "name": "search", "arguments": { "query": "verify_token" } },
            }),
            dir.path(),
            &cache::BundleCache::new(),
        )
        .unwrap();

        assert_eq!(response["id"], 7);
        assert_eq!(response["result"]["isError"], false);
        let text = response["result"]["content"][0]["text"].as_str().unwrap();
        assert!(text.contains("verify_token"));
    }

    #[test]
    fn tools_call_reports_missing_bundle_as_a_tool_error_not_a_protocol_error() {
        let response = handle_message(
            &json!({
                "jsonrpc": "2.0",
                "id": 1,
                "method": "tools/call",
                "params": { "name": "graph", "arguments": { "relation": "api" } },
            }),
            std::path::Path::new("/nonexistent/knowledge-bundle"),
            &cache::BundleCache::new(),
        )
        .unwrap();

        assert!(response.get("error").is_none());
        assert_eq!(response["result"]["isError"], true);
        let text = response["result"]["content"][0]["text"].as_str().unwrap();
        assert!(text.contains("okf-rs generate"));
    }
}
