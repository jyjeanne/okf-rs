//! The live-endpoint half of Phase E (`docs/improvement-plan-provenance-diff.md`):
//! actually calling a model to measure specialized-vs-consolidated MCP
//! tool-*selection* accuracy, instead of only setting up to measure it.
//!
//! [`crate::tool_selection_benchmark`] shipped everything that's checkable
//! without a model: the fixed question set, the reconstructed pre-0.3.0
//! specialized tool names, the scoring function, and a fixture bundle with
//! known-correct answers. This module is the part that was deliberately
//! deferred — wiring a real OpenAI-compatible `/chat/completions` endpoint
//! (with function calling, not just plain text — see below) so those
//! questions get asked of an actual model and scored for real.
//!
//! Configured entirely through environment variables — [`LiveConfig::from_env`]
//! — mirroring `--enrich`'s `OKF_ENRICH_BASE_URL`/`OKF_ENRICH_MODEL`/
//! `OKF_ENRICH_API_KEY` pattern (`crates/okf-enrich/src/lib.rs`,
//! `tools::enrich_config_from_env`), which is itself how this exact server
//! already resolves `search_semantic`'s embedding endpoint — an MCP tool
//! call has no equivalent of a CLI flag, and `--benchmark-tool-selection`
//! (see `main.rs`) matches that same "just a project root, config comes
//! from the environment" shape. Deliberately a **separate** set of
//! variables (`OKF_BENCHMARK_MODEL_*`, not `OKF_ENRICH_*`): the model
//! worth measuring for tool-selection accuracy need not be the model this
//! server would otherwise use for description enrichment or semantic
//! search, and reusing one config for both would make it impossible to
//! point them at different endpoints.
//!
//! Why this isn't just [`okf_enrich::EnrichClient`]: that client's
//! `complete()` sends a bare system+user chat completion with no `tools`/
//! `tool_choice` — sufficient for "summarize this function," but it never
//! asks the model to *choose among tool schemas*, which is the entire
//! thing this benchmark measures. A real tool-selection measurement has
//! to present the same kind of structured tool schema a real MCP client
//! would (the actual `graph` tool's JSON Schema for the consolidated
//! design, the reconstructed 13 `graph_*` schemas for the specialized
//! design — see [`consolidated_tool_schema`]/[`specialized_tool_schemas`])
//! and read back which one the model picked from `choices[0].message.tool_calls`,
//! not from free-text content. So this module has its own small
//! [`ToolCallingClient`], not a reuse of `okf-enrich`'s.
//!
//! Explicitly **not** reachable from the stdio JSON-RPC server: this is
//! wired up only behind `okf-mcp`'s `--benchmark-tool-selection` one-shot
//! CLI flag (see `main.rs`), the same "diagnostic side door, not a
//! protocol method" shape `--benchmark` already uses. Never run by
//! `cargo test --workspace` — a live model call is not offline,
//! deterministic, or free, unlike every other test in this codebase (see
//! `tool_selection_benchmark`'s own doc comment on why that harness stops
//! short of this). This module's own tests instead run against a
//! hand-rolled mock HTTP server (`mod tests`), exactly the pattern
//! `okf_enrich::test_support` already established, to keep the request/
//! response *parsing* covered without a real endpoint.

use crate::cache::BundleCache;
use crate::tool_selection_benchmark::{
    fixture_bundle, questions, scores_correctly, specialized_tool_name,
};
use crate::tools;
use anyhow::{anyhow, Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::fmt::Write as _;
use std::path::Path;
use std::time::{Duration, Instant};

/// Env var for the OpenAI-compatible endpoint's base URL, e.g.
/// `http://localhost:11434/v1` (Ollama) or `https://api.openai.com/v1`.
pub const BASE_URL_VAR: &str = "OKF_BENCHMARK_MODEL_BASE_URL";
/// Env var for the model name as the endpoint expects it.
pub const MODEL_VAR: &str = "OKF_BENCHMARK_MODEL";
/// Env var for an optional bearer API key; unset is fine for a local
/// endpoint that doesn't require one.
pub const API_KEY_VAR: &str = "OKF_BENCHMARK_MODEL_API_KEY";

/// How to reach the model under benchmark. See the module docs for why
/// this is a separate set of variables from `okf-enrich`'s.
#[derive(Debug, Clone)]
pub struct LiveConfig {
    pub base_url: String,
    pub model: String,
    pub api_key: Option<String>,
}

impl LiveConfig {
    pub fn from_env() -> Result<Self> {
        let base_url = std::env::var(BASE_URL_VAR).map_err(|_| {
            anyhow!(
                "--benchmark-tool-selection requires the {BASE_URL_VAR} environment variable \
                 (an OpenAI-compatible endpoint's base URL, e.g. http://localhost:11434/v1)"
            )
        })?;
        let model = std::env::var(MODEL_VAR).map_err(|_| {
            anyhow!("--benchmark-tool-selection requires the {MODEL_VAR} environment variable")
        })?;
        let api_key = std::env::var(API_KEY_VAR).ok();
        Ok(LiveConfig {
            base_url,
            model,
            api_key,
        })
    }
}

#[derive(Serialize)]
struct ChatRequest<'a> {
    model: &'a str,
    messages: Vec<ChatMessage<'a>>,
    tools: &'a [Value],
    tool_choice: &'a str,
    temperature: f32,
}

#[derive(Serialize)]
struct ChatMessage<'a> {
    role: &'a str,
    content: &'a str,
}

#[derive(Deserialize, Default)]
struct Usage {
    #[serde(default)]
    prompt_tokens: u64,
    #[serde(default)]
    completion_tokens: u64,
}

#[derive(Deserialize)]
struct ChatResponse {
    choices: Vec<ResponseChoice>,
    #[serde(default)]
    usage: Option<Usage>,
}

#[derive(Deserialize)]
struct ResponseChoice {
    message: ResponseMessage,
}

#[derive(Deserialize, Default)]
struct ResponseMessage {
    #[serde(default)]
    tool_calls: Vec<ToolCall>,
}

#[derive(Deserialize)]
struct ToolCall {
    function: FunctionCall,
}

#[derive(Deserialize)]
struct FunctionCall {
    name: String,
    /// The OpenAI-compatible wire format sends this as a JSON-encoded
    /// *string*, not a nested object — every provider this crate targets
    /// (OpenAI, Ollama, LM Studio) follows that shape.
    arguments: String,
}

/// One model response to a `choose_tool` call: which function it picked,
/// the parsed arguments, and the accounting this benchmark reports.
#[derive(Debug)]
pub struct ToolCallOutcome {
    pub tool_name: String,
    pub arguments: Value,
    pub prompt_tokens: u64,
    pub completion_tokens: u64,
    pub latency: Duration,
}

/// A client bound to one [`LiveConfig`], reused across every question in
/// one benchmark run.
pub struct ToolCallingClient {
    config: LiveConfig,
    agent: ureq::Agent,
}

impl ToolCallingClient {
    pub fn new(config: LiveConfig) -> Self {
        let agent = ureq::AgentBuilder::new()
            .timeout(Duration::from_secs(60))
            .build();
        ToolCallingClient { config, agent }
    }

    /// Sends `prompt` as a single user message alongside `tools` (each an
    /// OpenAI-compatible `{"type": "function", "function": {...}}` schema
    /// — see [`consolidated_tool_schema`]/[`specialized_tool_schemas`]),
    /// and returns which one the model called plus its arguments.
    /// `tool_choice: "auto"` rather than `"required"`: not every
    /// OpenAI-compatible endpoint this crate targets (Ollama, LM Studio)
    /// supports forcing a call, and a model that declines to call
    /// anything is itself a real, scoreable outcome (an error here, which
    /// callers count as a wrong answer) rather than something to paper
    /// over by only testing against providers that support forcing it.
    pub fn choose_tool(&self, prompt: &str, tools: &[Value]) -> Result<ToolCallOutcome> {
        let url = format!(
            "{}/chat/completions",
            self.config.base_url.trim_end_matches('/')
        );
        let request = ChatRequest {
            model: &self.config.model,
            messages: vec![ChatMessage {
                role: "user",
                content: prompt,
            }],
            tools,
            tool_choice: "auto",
            temperature: 0.0,
        };

        let mut req = self.agent.post(&url);
        if let Some(key) = &self.config.api_key {
            req = req.set("Authorization", &format!("Bearer {key}"));
        }

        let start = Instant::now();
        let response = req.send_json(&request).map_err(|e| match e {
            ureq::Error::Status(code, response) => {
                let body = response.into_string().unwrap_or_default();
                anyhow!("tool-selection endpoint {url} returned HTTP {code}: {body}")
            }
            ureq::Error::Transport(t) => {
                anyhow!("failed to reach tool-selection endpoint {url}: {t}")
            }
        })?;
        let latency = start.elapsed();

        let parsed: ChatResponse = response
            .into_json()
            .with_context(|| format!("malformed response from tool-selection endpoint {url}"))?;
        let usage = parsed.usage.unwrap_or_default();
        let choice = parsed
            .choices
            .into_iter()
            .next()
            .ok_or_else(|| anyhow!("tool-selection endpoint {url} returned no choices"))?;
        let tool_call = choice
            .message
            .tool_calls
            .into_iter()
            .next()
            .ok_or_else(|| anyhow!("model at {url} did not call any tool for prompt {prompt:?}"))?;
        let arguments: Value = if tool_call.function.arguments.trim().is_empty() {
            json!({})
        } else {
            serde_json::from_str(&tool_call.function.arguments).with_context(|| {
                format!(
                    "malformed tool-call arguments from {url}: {}",
                    tool_call.function.arguments
                )
            })?
        };

        Ok(ToolCallOutcome {
            tool_name: tool_call.function.name,
            arguments,
            prompt_tokens: usage.prompt_tokens,
            completion_tokens: usage.completion_tokens,
            latency,
        })
    }
}

/// The consolidated design's one tool schema, taken directly from the
/// real `graph` tool's own `tools/list` entry — not a hand-copied
/// approximation that could drift from what the server actually
/// advertises.
fn consolidated_tool_schema() -> Value {
    let graph_tool = tools::list()
        .into_iter()
        .find(|t| t["name"] == "graph")
        .expect("the graph tool is always registered");
    json!({
        "type": "function",
        "function": {
            "name": graph_tool["name"],
            "description": graph_tool["description"],
            "parameters": graph_tool["inputSchema"],
        }
    })
}

/// The specialized design's 13 reconstructed pre-0.3.0 `graph_*` tool
/// schemas — one per relation `graph` exposes today except `explain`,
/// which postdates that surface (see
/// [`crate::tool_selection_benchmark::specialized_tool_name`]).
/// Descriptions are lifted from the consolidated tool's own per-relation
/// bullet points (`tools::list`'s `graph` entry) rather than invented, so
/// a specialized-design "loss" can't be an artifact of a worse
/// description than the consolidated tool gets.
fn specialized_tool_schemas() -> Vec<Value> {
    let descriptions: &[(&str, &str)] = &[
        ("callers", "Concepts that directly call the given concept id."),
        ("callees", "Concepts the given concept id directly calls."),
        ("path", "Shortest call path between two concept ids."),
        (
            "api",
            "The project's public API surface (public functions, methods, and types).",
        ),
        (
            "cycles",
            "Groups of concepts that call each other in a cycle (direct or mutual recursion).",
        ),
        ("modules", "Cross-module call dependency edges."),
        (
            "isolated",
            "Concepts with no Calls/CalledBy edge in either direction -- candidates for dead code or unresolved calls.",
        ),
        (
            "stats",
            "Concept-kind breakdown, relationship edge counts by kind, and connected components of the Calls/CalledBy graph.",
        ),
        (
            "layers",
            "Each package's layer in the package dependency graph (layer 0 = depends on no other package in the bundle).",
        ),
        (
            "domains",
            "Clusters of packages that depend on each other, directly or transitively.",
        ),
        (
            "communities",
            "Package communities from modularity-optimization detection -- finer-grained than domains.",
        ),
        (
            "patterns",
            "Design patterns (Builder, Singleton, Factory, Visitor) detected via structural/naming heuristics.",
        ),
        (
            "features",
            "REST endpoints, database models, and event-flow participants detected via naming heuristics.",
        ),
    ];

    descriptions
        .iter()
        .map(|(relation, description)| {
            let parameters = match *relation {
                "callers" | "callees" => json!({
                    "type": "object",
                    "properties": { "id": { "type": "string", "description": "Concept id" } },
                    "required": ["id"],
                }),
                "path" => json!({
                    "type": "object",
                    "properties": {
                        "from": { "type": "string", "description": "Starting concept id" },
                        "to": { "type": "string", "description": "Target concept id" },
                    },
                    "required": ["from", "to"],
                }),
                _ => json!({ "type": "object", "properties": {} }),
            };
            json!({
                "type": "function",
                "function": {
                    "name": format!("graph_{relation}"),
                    "description": description,
                    "parameters": parameters,
                }
            })
        })
        .collect()
}

/// Dispatches a specialized-design tool call (`graph_<relation>`) through
/// the one real `graph` implementation `tools::call` already has — there
/// is no separate pre-0.3.0 code path left to call into, since 0.3.0
/// replaced it in place rather than keeping both around. Reinjecting
/// `relation` from the tool name is exactly the inverse of what
/// consolidation did to the schema, so this is a faithful reconstruction
/// of "what would have happened," not a shortcut around it.
fn call_via_relation(
    tool_name: &str,
    arguments: &Value,
    bundle: &Path,
    cache: &BundleCache,
) -> Result<String> {
    let relation = tool_name
        .strip_prefix("graph_")
        .ok_or_else(|| anyhow!("unexpected specialized tool name `{tool_name}`"))?;
    let mut args = arguments.clone();
    match &mut args {
        Value::Object(map) => {
            map.insert("relation".to_string(), Value::String(relation.to_string()));
        }
        _ => args = json!({ "relation": relation }),
    }
    tools::call("graph", &args, bundle, cache)
}

/// One question's scored outcome within one design.
pub struct QuestionOutcome {
    pub prompt: &'static str,
    pub expected: String,
    pub chosen: Option<String>,
    pub tool_selection_correct: bool,
    pub final_answer_correct: bool,
    pub prompt_tokens: u64,
    pub completion_tokens: u64,
    pub latency: Duration,
    pub error: Option<String>,
}

/// All of one design's (consolidated or specialized) question outcomes.
pub struct DesignReport {
    pub design: &'static str,
    pub outcomes: Vec<QuestionOutcome>,
}

impl DesignReport {
    pub fn correct_selections(&self) -> usize {
        self.outcomes
            .iter()
            .filter(|o| o.tool_selection_correct)
            .count()
    }

    pub fn correct_final_answers(&self) -> usize {
        self.outcomes
            .iter()
            .filter(|o| o.final_answer_correct)
            .count()
    }

    pub fn selection_accuracy(&self) -> f64 {
        if self.outcomes.is_empty() {
            return 0.0;
        }
        self.correct_selections() as f64 / self.outcomes.len() as f64
    }

    pub fn final_answer_accuracy(&self) -> f64 {
        if self.outcomes.is_empty() {
            return 0.0;
        }
        self.correct_final_answers() as f64 / self.outcomes.len() as f64
    }

    pub fn total_tokens(&self) -> u64 {
        self.outcomes
            .iter()
            .map(|o| o.prompt_tokens + o.completion_tokens)
            .sum()
    }

    pub fn total_latency(&self) -> Duration {
        self.outcomes.iter().map(|o| o.latency).sum()
    }
}

/// Both designs' reports from one benchmark run against one model.
pub struct LiveReport {
    pub model: String,
    pub consolidated: DesignReport,
    pub specialized: DesignReport,
}

impl LiveReport {
    pub fn render(&self) -> String {
        let mut out = String::new();
        let _ = writeln!(out, "okf-mcp tool-selection benchmark (Phase E, live)");
        let _ = writeln!(out, "=================================================");
        let _ = writeln!(out);
        let _ = writeln!(out, "Model: {}", self.model);
        let _ = writeln!(
            out,
            "Sample size: {} questions (consolidated), {} questions (specialized -- \
             `explain` excluded, no pre-0.3.0 counterpart to compare against)",
            self.consolidated.outcomes.len(),
            self.specialized.outcomes.len(),
        );
        let _ = writeln!(
            out,
            "This measures one model's behavior on one fixed fixture bundle -- not a \
             universal claim about \"LLMs in general.\""
        );
        let _ = writeln!(out);

        for design in [&self.consolidated, &self.specialized] {
            let _ = writeln!(out, "-- {} design --", design.design);
            let _ = writeln!(
                out,
                "  tool/relation-selection accuracy: {}/{} ({:.0}%)",
                design.correct_selections(),
                design.outcomes.len(),
                design.selection_accuracy() * 100.0,
            );
            let _ = writeln!(
                out,
                "  final-answer accuracy:            {}/{} ({:.0}%)",
                design.correct_final_answers(),
                design.outcomes.len(),
                design.final_answer_accuracy() * 100.0,
            );
            let _ = writeln!(
                out,
                "  total tokens (prompt+completion): {}",
                design.total_tokens()
            );
            let _ = writeln!(
                out,
                "  total latency:                    {:.2}s",
                design.total_latency().as_secs_f64()
            );
            for o in &design.outcomes {
                if let Some(err) = &o.error {
                    let _ = writeln!(out, "    [ERROR] {:?}: {err}", o.prompt);
                } else if !o.tool_selection_correct {
                    let _ = writeln!(
                        out,
                        "    [WRONG] {:?}: expected `{}`, got {:?}",
                        o.prompt, o.expected, o.chosen
                    );
                }
            }
            let _ = writeln!(out);
        }

        out
    }
}

fn run_consolidated(
    client: &ToolCallingClient,
    bundle: &Path,
    cache: &BundleCache,
) -> DesignReport {
    let schema = [consolidated_tool_schema()];
    let outcomes = questions()
        .into_iter()
        .map(|q| match client.choose_tool(q.prompt, &schema) {
            Ok(outcome) => {
                let chosen_relation = outcome
                    .arguments
                    .get("relation")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                let selection_correct =
                    outcome.tool_name == "graph" && scores_correctly(&chosen_relation, q.relation);
                let final_correct = selection_correct
                    && tools::call("graph", &outcome.arguments, bundle, cache)
                        .map(|r| r.contains(q.expected_substring))
                        .unwrap_or(false);
                QuestionOutcome {
                    prompt: q.prompt,
                    expected: q.relation.to_string(),
                    chosen: Some(chosen_relation),
                    tool_selection_correct: selection_correct,
                    final_answer_correct: final_correct,
                    prompt_tokens: outcome.prompt_tokens,
                    completion_tokens: outcome.completion_tokens,
                    latency: outcome.latency,
                    error: None,
                }
            }
            Err(e) => QuestionOutcome {
                prompt: q.prompt,
                expected: q.relation.to_string(),
                chosen: None,
                tool_selection_correct: false,
                final_answer_correct: false,
                prompt_tokens: 0,
                completion_tokens: 0,
                latency: Duration::ZERO,
                error: Some(e.to_string()),
            },
        })
        .collect();
    DesignReport {
        design: "consolidated",
        outcomes,
    }
}

fn run_specialized(client: &ToolCallingClient, bundle: &Path, cache: &BundleCache) -> DesignReport {
    let schema = specialized_tool_schemas();
    let outcomes = questions()
        .into_iter()
        .filter_map(|q| {
            let expected_tool = specialized_tool_name(q.relation)?;
            Some(match client.choose_tool(q.prompt, &schema) {
                Ok(outcome) => {
                    let selection_correct = scores_correctly(&outcome.tool_name, &expected_tool);
                    let final_correct = selection_correct
                        && call_via_relation(&outcome.tool_name, &outcome.arguments, bundle, cache)
                            .map(|r| r.contains(q.expected_substring))
                            .unwrap_or(false);
                    QuestionOutcome {
                        prompt: q.prompt,
                        expected: expected_tool,
                        chosen: Some(outcome.tool_name),
                        tool_selection_correct: selection_correct,
                        final_answer_correct: final_correct,
                        prompt_tokens: outcome.prompt_tokens,
                        completion_tokens: outcome.completion_tokens,
                        latency: outcome.latency,
                        error: None,
                    }
                }
                Err(e) => QuestionOutcome {
                    prompt: q.prompt,
                    expected: expected_tool,
                    chosen: None,
                    tool_selection_correct: false,
                    final_answer_correct: false,
                    prompt_tokens: 0,
                    completion_tokens: 0,
                    latency: Duration::ZERO,
                    error: Some(e.to_string()),
                },
            })
        })
        .collect();
    DesignReport {
        design: "specialized",
        outcomes,
    }
}

/// Runs the full live benchmark (both designs, all questions) against
/// `config`, on the shared fixture bundle. The one entry point
/// `main.rs`'s `--benchmark-tool-selection` calls.
pub fn run(config: &LiveConfig) -> LiveReport {
    let client = ToolCallingClient::new(config.clone());
    let dir = fixture_bundle();
    let cache = BundleCache::new();

    let consolidated = run_consolidated(&client, dir.path(), &cache);
    let specialized = run_specialized(&client, dir.path(), &cache);

    LiveReport {
        model: config.model.clone(),
        consolidated,
        specialized,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{BufRead, BufReader, Write};
    use std::net::TcpListener;

    /// A minimal `TcpListener`-based mock `/chat/completions` endpoint —
    /// the same hand-rolled-server pattern `okf_enrich::test_support`
    /// uses, adapted to reply with a tool call instead of plain content
    /// (which that crate's client never sends). `reply_body` is the raw
    /// JSON response body to return for every request.
    struct MockServer {
        base_url: String,
    }

    fn start_mock_server(reply_body: &'static str, status_line: &'static str) -> MockServer {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();

        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { break };
                let mut reader = BufReader::new(stream.try_clone().unwrap());
                let mut content_length = 0usize;
                loop {
                    let mut line = String::new();
                    if reader.read_line(&mut line).unwrap_or(0) == 0 {
                        break;
                    }
                    let trimmed = line.trim_end();
                    if trimmed.is_empty() {
                        break;
                    }
                    if let Some(value) = trimmed.strip_prefix("Content-Length: ") {
                        content_length = value.trim().parse().unwrap_or(0);
                    }
                }
                let mut body = vec![0u8; content_length];
                std::io::Read::read_exact(&mut reader, &mut body).unwrap();

                let response = format!(
                    "HTTP/1.1 {status_line}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                    reply_body.len(),
                    reply_body
                );
                let _ = stream.write_all(response.as_bytes());
            }
        });

        MockServer {
            base_url: format!("http://127.0.0.1:{port}/v1"),
        }
    }

    fn client_for(server: &MockServer) -> ToolCallingClient {
        ToolCallingClient::new(LiveConfig {
            base_url: server.base_url.clone(),
            model: "test-model".to_string(),
            api_key: None,
        })
    }

    #[test]
    fn choose_tool_parses_the_function_name_arguments_and_usage() {
        let server = start_mock_server(
            r#"{"choices":[{"message":{"tool_calls":[{"function":{"name":"graph","arguments":"{\"relation\":\"callers\",\"id\":\"functions/pkg-b/bar\"}"}}]}}],"usage":{"prompt_tokens":42,"completion_tokens":7}}"#,
            "200 OK",
        );
        let client = client_for(&server);
        let outcome = client
            .choose_tool("Who calls bar?", &[consolidated_tool_schema()])
            .unwrap();
        assert_eq!(outcome.tool_name, "graph");
        assert_eq!(outcome.arguments["relation"], "callers");
        assert_eq!(outcome.arguments["id"], "functions/pkg-b/bar");
        assert_eq!(outcome.prompt_tokens, 42);
        assert_eq!(outcome.completion_tokens, 7);
    }

    #[test]
    fn choose_tool_errors_clearly_when_the_model_calls_no_tool() {
        let server = start_mock_server(r#"{"choices":[{"message":{}}]}"#, "200 OK");
        let client = client_for(&server);
        let err = client
            .choose_tool("Who calls bar?", &[consolidated_tool_schema()])
            .unwrap_err();
        assert!(err.to_string().contains("did not call any tool"));
    }

    #[test]
    fn choose_tool_surfaces_an_http_error_status_and_body() {
        let server = start_mock_server(r#"{"error":"rate limited"}"#, "429 Too Many Requests");
        let client = client_for(&server);
        let err = client
            .choose_tool("Who calls bar?", &[consolidated_tool_schema()])
            .unwrap_err();
        assert!(err.to_string().contains("429"));
        assert!(err.to_string().contains("rate limited"));
    }

    #[test]
    fn choose_tool_errors_clearly_on_malformed_tool_call_arguments() {
        let server = start_mock_server(
            r#"{"choices":[{"message":{"tool_calls":[{"function":{"name":"graph","arguments":"not json"}}]}}]}"#,
            "200 OK",
        );
        let client = client_for(&server);
        let err = client
            .choose_tool("Who calls bar?", &[consolidated_tool_schema()])
            .unwrap_err();
        assert!(err.to_string().contains("malformed tool-call arguments"));
    }

    #[test]
    fn live_config_from_env_requires_base_url_and_model() {
        std::env::remove_var(BASE_URL_VAR);
        std::env::remove_var(MODEL_VAR);
        let err = LiveConfig::from_env().unwrap_err();
        assert!(err.to_string().contains(BASE_URL_VAR));
    }

    #[test]
    fn consolidated_tool_schema_matches_the_live_graph_tool() {
        let schema = consolidated_tool_schema();
        assert_eq!(schema["function"]["name"], "graph");
        assert!(schema["function"]["parameters"]["properties"]["relation"].is_object());
    }

    #[test]
    fn specialized_tool_schemas_cover_every_reconstructed_tool_name() {
        let schemas = specialized_tool_schemas();
        assert_eq!(schemas.len(), 13);
        let names: Vec<&str> = schemas
            .iter()
            .map(|s| s["function"]["name"].as_str().unwrap())
            .collect();
        assert!(names.contains(&"graph_callers"));
        assert!(names.contains(&"graph_patterns"));
        assert!(!names.contains(&"graph_explain"));
    }

    #[test]
    fn call_via_relation_dispatches_through_the_real_graph_tool() {
        let dir = fixture_bundle();
        let cache = BundleCache::new();
        let response = call_via_relation(
            "graph_callers",
            &json!({ "id": "functions/pkg-b/bar" }),
            dir.path(),
            &cache,
        )
        .unwrap();
        assert!(response.contains("functions/pkg-a/foo"));
    }

    /// The full offline-mockable path end to end: a mock endpoint that
    /// always answers correctly for the consolidated design (returns the
    /// literal `relation`/args each question expects) scores 100% on
    /// both selection and final-answer accuracy — proving the scoring
    /// and dispatch plumbing in [`run_consolidated`] is wired correctly,
    /// independent of any real model's behavior.
    #[test]
    fn run_consolidated_against_a_mock_that_always_answers_correctly_scores_perfectly() {
        let dir = fixture_bundle();
        let cache = BundleCache::new();
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { break };
                let mut reader = BufReader::new(stream.try_clone().unwrap());
                let mut content_length = 0usize;
                loop {
                    let mut line = String::new();
                    if reader.read_line(&mut line).unwrap_or(0) == 0 {
                        break;
                    }
                    let trimmed = line.trim_end();
                    if trimmed.is_empty() {
                        break;
                    }
                    if let Some(value) = trimmed.strip_prefix("Content-Length: ") {
                        content_length = value.trim().parse().unwrap_or(0);
                    }
                }
                let mut raw_body = vec![0u8; content_length];
                std::io::Read::read_exact(&mut reader, &mut raw_body).unwrap();
                let request: Value = serde_json::from_slice(&raw_body).unwrap();
                let prompt = request["messages"][0]["content"].as_str().unwrap();
                let question = questions()
                    .into_iter()
                    .find(|q| q.prompt == prompt)
                    .unwrap();
                let args = question.args.to_json(question.relation);
                let args_str = serde_json::to_string(&args).unwrap().replace('"', "\\\"");
                let payload = format!(
                    r#"{{"choices":[{{"message":{{"tool_calls":[{{"function":{{"name":"graph","arguments":"{args_str}"}}}}]}}}}]}}"#
                );
                let response = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                    payload.len(),
                    payload
                );
                let _ = stream.write_all(response.as_bytes());
            }
        });

        let client = ToolCallingClient::new(LiveConfig {
            base_url: format!("http://127.0.0.1:{port}/v1"),
            model: "test-model".to_string(),
            api_key: None,
        });
        let report = run_consolidated(&client, dir.path(), &cache);
        assert_eq!(report.correct_selections(), report.outcomes.len());
        assert_eq!(report.correct_final_answers(), report.outcomes.len());
    }

    #[test]
    fn live_report_render_includes_the_model_and_both_designs() {
        let report = LiveReport {
            model: "test-model".to_string(),
            consolidated: DesignReport {
                design: "consolidated",
                outcomes: vec![QuestionOutcome {
                    prompt: "Who calls bar?",
                    expected: "callers".to_string(),
                    chosen: Some("callers".to_string()),
                    tool_selection_correct: true,
                    final_answer_correct: true,
                    prompt_tokens: 10,
                    completion_tokens: 2,
                    latency: Duration::from_millis(50),
                    error: None,
                }],
            },
            specialized: DesignReport {
                design: "specialized",
                outcomes: vec![QuestionOutcome {
                    prompt: "Who calls bar?",
                    expected: "graph_callers".to_string(),
                    chosen: Some("graph_callees".to_string()),
                    tool_selection_correct: false,
                    final_answer_correct: false,
                    prompt_tokens: 10,
                    completion_tokens: 2,
                    latency: Duration::from_millis(50),
                    error: None,
                }],
            },
        };
        let text = report.render();
        assert!(text.contains("test-model"));
        assert!(text.contains("consolidated"));
        assert!(text.contains("specialized"));
        assert!(text.contains("[WRONG]"));
        assert!(text.contains("100%"));
        assert!(text.contains("0%"));
    }
}
