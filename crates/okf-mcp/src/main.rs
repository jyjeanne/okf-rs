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
//! The server is bound to a single project at startup (its argv[1], or
//! the current directory), the same way `okf-rs search`/`validate`/
//! `graph` resolve a bundle: an explicit bundle path wins, otherwise
//! `okf.toml`'s `output` under the project root, otherwise `knowledge`.
//! Each tool call re-reads the bundle fresh (via `okf_parser::read_bundle`
//! or `okf_search::SearchIndex::build`), so it always reflects the latest
//! `okf-rs generate` run without needing a restart.

mod tools;

use anyhow::Result;
use serde_json::{json, Value};
use std::io::{self, BufRead, Write};
use std::path::PathBuf;

const PROTOCOL_VERSION: &str = "2024-11-05";

fn main() -> Result<()> {
    let project_root = std::env::args()
        .nth(1)
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."));
    let project_root = project_root
        .canonicalize()
        .unwrap_or_else(|_| project_root.clone());
    let bundle = okf_core::config::resolve_bundle(&project_root, None);

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
        if let Some(response) = handle_message(&request, &bundle) {
            writeln!(stdout, "{}", serde_json::to_string(&response)?)?;
            stdout.flush()?;
        }
    }
    Ok(())
}

/// Dispatches one JSON-RPC message, returning the response to write (or
/// `None` for notifications, which never get one).
fn handle_message(request: &Value, bundle: &std::path::Path) -> Option<Value> {
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
            match tools::call(name, &arguments, bundle) {
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
        );
        assert!(response.is_none());
    }

    #[test]
    fn unknown_method_on_a_request_errors_but_unknown_notification_is_silent() {
        let response = handle_message(
            &json!({ "jsonrpc": "2.0", "id": 1, "method": "bogus" }),
            std::path::Path::new("knowledge"),
        )
        .unwrap();
        assert_eq!(response["error"]["code"], -32601);

        let response = handle_message(
            &json!({ "jsonrpc": "2.0", "method": "notifications/bogus" }),
            std::path::Path::new("knowledge"),
        );
        assert!(response.is_none());
    }

    #[test]
    fn tools_list_includes_search_and_graph_tools() {
        let response = handle_message(
            &json!({ "jsonrpc": "2.0", "id": 1, "method": "tools/list" }),
            std::path::Path::new("knowledge"),
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
        assert!(names.contains(&"graph_callers"));
        assert!(names.contains(&"graph_callees"));
        assert!(names.contains(&"graph_api"));
        assert!(names.contains(&"graph_cycles"));
        assert!(names.contains(&"graph_modules"));
        assert!(names.contains(&"graph_isolated"));
        assert!(names.contains(&"graph_stats"));
        assert!(names.contains(&"graph_path"));
        assert!(names.contains(&"graph_communities"));
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
                "params": { "name": "graph_api", "arguments": {} },
            }),
            std::path::Path::new("/nonexistent/knowledge-bundle"),
        )
        .unwrap();

        assert!(response.get("error").is_none());
        assert_eq!(response["result"]["isError"], true);
        let text = response["result"]["content"][0]["text"].as_str().unwrap();
        assert!(text.contains("okf-rs generate"));
    }
}
