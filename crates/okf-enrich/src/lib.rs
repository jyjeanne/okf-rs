//! Optional AI enrichment: fills in a concept's missing `description`
//! (function/method summaries, module/package descriptions), and
//! suggests missing relationship links between semantically close
//! concepts (see [`links`]) — via any OpenAI-compatible chat completions
//! endpoint: [Ollama](https://ollama.com), LM Studio, LocalAI,
//! [Crustly](https://github.com/jyjeanne/crustly), or a cloud provider.
//! Never a hard dependency on one vendor: this crate only ever speaks
//! the one `POST {base_url}/chat/completions` shape every one of those
//! already implements.
//!
//! Entirely optional and additive, the same way `--lsp` is in
//! `okf-analyzer`: nothing in `okf-rs generate` requires this crate, and
//! a concept that already has a description (human-written, or carried
//! forward from a previous `--enrich` run — see [`enrich_missing_descriptions`])
//! is never re-queried or overwritten.

mod links;

pub use links::{suggest_missing_links, SuggestedLink};

use anyhow::{anyhow, bail, Context, Result};
use okf_parser::{Concept, ConceptKind};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::time::Duration;

/// How to reach an OpenAI-compatible chat completions endpoint. `api_key`
/// is optional since a local server (Ollama, LM Studio, LocalAI) doesn't
/// need one; a cloud provider's key is sent as a bearer token when
/// present.
#[derive(Debug, Clone)]
pub struct EnrichConfig {
    /// Everything up to (not including) `/chat/completions`, e.g.
    /// `http://localhost:11434/v1` or `https://api.openai.com/v1`.
    pub base_url: String,
    /// Model name as the endpoint expects it, e.g. `llama3.1` or
    /// `gpt-4o-mini`.
    pub model: String,
    /// Sent as `Authorization: Bearer <key>` when present; omitted
    /// entirely otherwise.
    pub api_key: Option<String>,
}

/// A client bound to one [`EnrichConfig`], reused across every concept
/// enriched in a single `okf-rs generate --enrich` run.
pub struct EnrichClient {
    config: EnrichConfig,
    agent: ureq::Agent,
}

#[derive(Serialize)]
struct ChatRequest<'a> {
    model: &'a str,
    messages: Vec<ChatMessage<'a>>,
    temperature: f32,
}

#[derive(Serialize)]
struct ChatMessage<'a> {
    role: &'a str,
    content: &'a str,
}

#[derive(Deserialize)]
struct ChatResponse {
    choices: Vec<ChatChoice>,
}

#[derive(Deserialize)]
struct ChatChoice {
    message: ChatResponseMessage,
}

#[derive(Deserialize)]
struct ChatResponseMessage {
    content: String,
}

impl EnrichClient {
    /// Builds a client. Doesn't itself make any network call — nothing
    /// is reachability-checked until the first [`EnrichClient::complete`].
    pub fn new(config: EnrichConfig) -> Self {
        let agent = ureq::AgentBuilder::new()
            .timeout(Duration::from_secs(60))
            .build();
        EnrichClient { config, agent }
    }

    /// Sends one chat completion request (`system` + `user` messages, a
    /// single response expected) and returns the model's reply text,
    /// trimmed. Low temperature (`0.2`): a concise, literal description
    /// is wanted here, not creative variation.
    pub fn complete(&self, system: &str, user: &str) -> Result<String> {
        let url = format!(
            "{}/chat/completions",
            self.config.base_url.trim_end_matches('/')
        );
        let request = ChatRequest {
            model: &self.config.model,
            messages: vec![
                ChatMessage {
                    role: "system",
                    content: system,
                },
                ChatMessage {
                    role: "user",
                    content: user,
                },
            ],
            temperature: 0.2,
        };

        let mut req = self.agent.post(&url);
        if let Some(key) = &self.config.api_key {
            req = req.set("Authorization", &format!("Bearer {key}"));
        }

        let response = req.send_json(&request).map_err(|e| match e {
            ureq::Error::Status(code, response) => {
                let body = response.into_string().unwrap_or_default();
                anyhow!("enrichment endpoint {url} returned HTTP {code}: {body}")
            }
            ureq::Error::Transport(t) => {
                anyhow!("failed to reach enrichment endpoint {url}: {t}")
            }
        })?;

        let parsed: ChatResponse = response
            .into_json()
            .with_context(|| format!("malformed response from enrichment endpoint {url}"))?;
        let content = parsed
            .choices
            .into_iter()
            .next()
            .ok_or_else(|| anyhow!("enrichment endpoint {url} returned no choices"))?
            .message
            .content;
        let content = content.trim();
        if content.is_empty() {
            bail!("enrichment endpoint {url} returned an empty completion");
        }
        Ok(content.to_string())
    }
}

const SYSTEM_PROMPT: &str = "You are a technical writer producing a single-sentence, factual description for a piece of source code, to be stored as documentation. Reply with only that one sentence — no preamble, no markdown, no quotes around it.";

/// The kinds [`enrich_missing_descriptions`] fills in. Deliberately
/// narrower than every [`ConceptKind`]: functions/methods and modules/
/// packages are what the roadmap names ("function summaries, module
/// descriptions") and cover the overwhelming majority of undocumented
/// concepts in practice. Types (`Struct`/`Enum`/`Class`/...) aren't
/// enriched yet — see the crate root docs' known-limitations note.
fn is_enrichable(kind: ConceptKind) -> bool {
    matches!(
        kind,
        ConceptKind::Function | ConceptKind::Method | ConceptKind::Module | ConceptKind::Package
    )
}

fn prompt_for(concept: &Concept) -> String {
    match concept.kind {
        ConceptKind::Function | ConceptKind::Method => format!(
            "Describe what this {} function/method does, in one sentence, for developer documentation.\nName: {}\nSignature: {}",
            concept.language.display_name(),
            concept.qualified_name,
            concept.signature.as_deref().unwrap_or("(signature unknown)"),
        ),
        ConceptKind::Module => format!(
            "Describe the purpose of this {} module, in one sentence, for developer documentation.\nModule: {}\nSource file: {}",
            concept.language.display_name(),
            concept.qualified_name,
            concept.location.file,
        ),
        ConceptKind::Package => format!(
            "Describe the purpose of this {} package, in one sentence, for developer documentation.\nPackage: {}",
            concept.language.display_name(),
            concept.qualified_name,
        ),
        other => format!(
            "Describe this {} {:?} named `{}`, in one sentence, for developer documentation.",
            concept.language.display_name(),
            other,
            concept.qualified_name
        ),
    }
}

/// How many concepts [`enrich_missing_descriptions`] touched: generated
/// fresh via the endpoint, versus reused from `previous` without a
/// network call at all.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct EnrichStats {
    /// Descriptions generated via a fresh call to the endpoint.
    pub generated: usize,
    /// Descriptions carried forward from `previous` (a prior bundle's
    /// non-empty description for the same concept id) without a call.
    pub reused: usize,
}

/// Fills in `description` for every concept in `concepts` that's missing
/// one (see [`is_enrichable`] for which kinds are eligible) — mutating in
/// place, and never overwriting a description a concept already has,
/// whether human-written or from an earlier enrichment run.
///
/// `previous` is the concept set from a bundle previously written to the
/// same output directory (an empty slice if there isn't one yet, e.g.
/// `okf_parser::read_bundle` returning `Vec::new()` for a path that
/// doesn't exist). Since `okf-rs generate` re-derives every concept from
/// source on every run — descriptions never survive that re-derivation on
/// their own, unlike the tree-sitter extraction its own cache speeds up
/// — matching a concept's id against `previous`'s own already-enriched
/// (or hand-written) description lets a second `--enrich` run skip a
/// network call entirely for anything already described, rather than
/// re-querying the whole bundle every time.
pub fn enrich_missing_descriptions(
    client: &EnrichClient,
    concepts: &mut [Concept],
    previous: &[Concept],
) -> Result<EnrichStats> {
    let previous_by_id: HashMap<&str, &str> = previous
        .iter()
        .filter_map(|c| {
            c.description
                .as_deref()
                .filter(|d| !d.trim().is_empty())
                .map(|d| (c.id.as_str(), d))
        })
        .collect();

    let mut stats = EnrichStats::default();
    for concept in concepts.iter_mut() {
        if !is_enrichable(concept.kind) {
            continue;
        }
        if concept
            .description
            .as_deref()
            .is_some_and(|d| !d.trim().is_empty())
        {
            continue;
        }

        if let Some(&reused) = previous_by_id.get(concept.id.as_str()) {
            concept.description = Some(reused.to_string());
            stats.reused += 1;
            continue;
        }

        let description = client
            .complete(SYSTEM_PROMPT, &prompt_for(concept))
            .with_context(|| format!("failed to enrich `{}`", concept.id))?;
        concept.description = Some(description);
        stats.generated += 1;
    }

    Ok(stats)
}

/// A minimal, single-endpoint OpenAI-compatible mock server for tests
/// (this crate's own, and `links`'s): reads one HTTP/1.1 request,
/// ignores everything about it except that it arrived, and replies with
/// a canned `chat.completions`-shaped JSON body. Runs on a background
/// thread per accepted connection so a test can make several requests
/// against one client without needing to know how many in advance.
#[cfg(test)]
pub(crate) mod test_support {
    use crate::{EnrichClient, EnrichConfig};
    use std::io::{BufRead, BufReader, Write};
    use std::net::TcpListener;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;

    pub(crate) struct MockServer {
        pub(crate) base_url: String,
        pub(crate) request_count: Arc<AtomicUsize>,
    }

    pub(crate) fn start_mock_server(reply_content: &'static str) -> MockServer {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let request_count = Arc::new(AtomicUsize::new(0));
        let counter = Arc::clone(&request_count);

        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { break };
                counter.fetch_add(1, Ordering::SeqCst);
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

                let payload = format!(
                    r#"{{"choices":[{{"message":{{"content":"{reply_content}"}}}}]}}"#
                );
                let response = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                    payload.len(),
                    payload
                );
                let _ = stream.write_all(response.as_bytes());
            }
        });

        MockServer {
            base_url: format!("http://127.0.0.1:{port}/v1"),
            request_count,
        }
    }

    pub(crate) fn client_for(server: &MockServer) -> EnrichClient {
        EnrichClient::new(EnrichConfig {
            base_url: server.base_url.clone(),
            model: "test-model".to_string(),
            api_key: None,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{client_for, start_mock_server};
    use okf_parser::{Language, Location};
    use std::sync::atomic::Ordering;

    fn function_concept(id: &str) -> Concept {
        Concept {
            id: id.to_string(),
            kind: ConceptKind::Function,
            language: Language::Rust,
            name: id.to_string(),
            qualified_name: id.to_string(),
            description: None,
            location: Location {
                file: "src/lib.rs".to_string(),
                start_line: 1,
                end_line: 1,
            },
            signature: Some("fn f()".to_string()),
            tags: Vec::new(),
            is_public: true,
            generated_at: None,
            relationships: Vec::new(),
        }
    }

    #[test]
    fn complete_parses_a_canned_response() {
        let server = start_mock_server("a concise summary");
        let client = client_for(&server);
        let text = client.complete("system", "user").unwrap();
        assert_eq!(text, "a concise summary");
        assert_eq!(server.request_count.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn complete_errors_clearly_when_the_endpoint_is_unreachable() {
        let client = EnrichClient::new(EnrichConfig {
            base_url: "http://127.0.0.1:1".to_string(),
            model: "m".to_string(),
            api_key: None,
        });
        let err = client.complete("s", "u").unwrap_err();
        assert!(err.to_string().contains("failed to reach"));
    }

    #[test]
    fn enrich_missing_descriptions_fills_in_a_function_without_one() {
        let server = start_mock_server("verifies a token");
        let client = client_for(&server);
        let mut concepts = vec![function_concept("functions/f")];

        let stats = enrich_missing_descriptions(&client, &mut concepts, &[]).unwrap();
        assert_eq!(stats, EnrichStats { generated: 1, reused: 0 });
        assert_eq!(concepts[0].description.as_deref(), Some("verifies a token"));
    }

    #[test]
    fn enrich_missing_descriptions_never_overwrites_an_existing_one() {
        let server = start_mock_server("should not be used");
        let client = client_for(&server);
        let mut concept = function_concept("functions/f");
        concept.description = Some("hand-written description".to_string());
        let mut concepts = vec![concept];

        let stats = enrich_missing_descriptions(&client, &mut concepts, &[]).unwrap();
        assert_eq!(stats, EnrichStats { generated: 0, reused: 0 });
        assert_eq!(
            concepts[0].description.as_deref(),
            Some("hand-written description")
        );
        assert_eq!(server.request_count.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn enrich_missing_descriptions_reuses_a_previous_bundles_description_without_a_call() {
        let server = start_mock_server("should not be used");
        let client = client_for(&server);
        let mut concepts = vec![function_concept("functions/f")];
        let mut previous = function_concept("functions/f");
        previous.description = Some("from a prior --enrich run".to_string());

        let stats =
            enrich_missing_descriptions(&client, &mut concepts, &[previous]).unwrap();
        assert_eq!(stats, EnrichStats { generated: 0, reused: 1 });
        assert_eq!(
            concepts[0].description.as_deref(),
            Some("from a prior --enrich run")
        );
        assert_eq!(server.request_count.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn enrich_missing_descriptions_skips_kinds_outside_the_enrichable_set() {
        let server = start_mock_server("should not be used");
        let client = client_for(&server);
        let mut concept = function_concept("structs/s");
        concept.kind = ConceptKind::Struct;
        let mut concepts = vec![concept];

        let stats = enrich_missing_descriptions(&client, &mut concepts, &[]).unwrap();
        assert_eq!(stats, EnrichStats { generated: 0, reused: 0 });
        assert_eq!(concepts[0].description, None);
        assert_eq!(server.request_count.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn enrich_missing_descriptions_mixes_reused_and_generated_in_one_run() {
        let server = start_mock_server("freshly generated");
        let client = client_for(&server);
        let mut previous = function_concept("functions/has_prior");
        previous.description = Some("carried forward".to_string());
        let mut concepts = vec![
            function_concept("functions/has_prior"),
            function_concept("functions/new"),
        ];

        let stats =
            enrich_missing_descriptions(&client, &mut concepts, &[previous]).unwrap();
        assert_eq!(stats, EnrichStats { generated: 1, reused: 1 });
        assert_eq!(
            concepts[0].description.as_deref(),
            Some("carried forward")
        );
        assert_eq!(
            concepts[1].description.as_deref(),
            Some("freshly generated")
        );
        assert_eq!(server.request_count.load(Ordering::SeqCst), 1);
    }
}
