//! LSP-backed disambiguation of ambiguous call candidates — see
//! [`crate::analyze_with_cache_lsp`] for the algorithm this plugs into.

use okf_core::Project;
use okf_parser::{Language, Location};
use okf_tree_sitter::CallCandidate;
use std::collections::HashMap;
use std::time::Duration;

/// A freshly started language server needs time to index a project
/// before `textDocument/definition` returns anything useful — this bounds
/// how long the *first* query for each language waits for that (10s
/// total, in half-second steps). Once a language's server has returned at
/// least one real answer, [`ClientState::warmed_up`] is set and later
/// calls stop paying that retry budget, so a call the server genuinely
/// can't resolve (as opposed to one asked before indexing finished) costs
/// one quick attempt, not a repeated ~10s stall.
const DEFINITION_RETRIES: u32 = 20;
const DEFINITION_RETRY_DELAY: Duration = Duration::from_millis(500);

struct ClientState {
    client: Option<okf_lsp::LspClient>,
    warmed_up: bool,
}

/// Attempts to resolve each of `ambiguous` (calls already known to match
/// more than one candidate by name) via that call's own language server,
/// appending any edge it can precisely confirm to `resolved_edges`.
/// Best-effort throughout: a language with no available server, a call
/// whose position the server can't answer for, or an answer that doesn't
/// land inside any of the known candidates' own locations, is simply
/// skipped — never guessed.
///
/// One `okf_lsp::LspClient` is started per distinct language actually
/// needed (lazily, on first use) and shut down once every ambiguous call
/// has been attempted. `file_sources` must already hold every file in
/// `ambiguous` (keyed by relative path) — reusing the source text the
/// caller already read once, rather than this function reading each file
/// from disk again itself.
pub(crate) fn resolve_ambiguous_calls(
    project: &Project,
    ambiguous: &[&(CallCandidate, Language, String)],
    file_sources: &HashMap<String, String>,
    id_to_location: &HashMap<&str, &Location>,
    candidates_by_name: &HashMap<&str, Vec<&str>>,
    resolved_edges: &mut Vec<(String, String)>,
) {
    let mut clients: HashMap<Language, ClientState> = HashMap::new();

    for (call, language, relative_path) in ambiguous {
        let state = clients.entry(*language).or_insert_with(|| ClientState {
            client: match okf_lsp::LspClient::start(*language, &project.root) {
                Ok(client) => client,
                Err(e) => {
                    eprintln!(
                        "warning: failed to start a language server for {language}, \
                         skipping LSP disambiguation for it: {e:#}"
                    );
                    None
                }
            },
            warmed_up: false,
        });

        let retries = if state.warmed_up {
            1
        } else {
            DEFINITION_RETRIES
        };
        let Some(client) = state.client.as_mut() else {
            continue;
        };
        let Some(source) = file_sources.get(relative_path.as_str()) else {
            continue;
        };
        let Ok(uri) = client.ensure_open(relative_path, source) else {
            continue;
        };

        let mut locations = Vec::new();
        let mut got_result = false;
        for attempt in 0..retries {
            match client.definition(&uri, call.call_site.line, call.call_site.character) {
                Ok(found) if !found.is_empty() => {
                    locations = found;
                    got_result = true;
                    break;
                }
                Ok(_) if attempt + 1 < retries => {
                    std::thread::sleep(DEFINITION_RETRY_DELAY);
                }
                _ => break,
            }
        }
        if got_result {
            state.warmed_up = true;
        }
        if locations.is_empty() {
            continue;
        }

        let Some(candidates) = candidates_by_name.get(call.callee_name.as_str()) else {
            continue;
        };
        let matched = candidates.iter().find(|id| {
            id_to_location.get(**id).is_some_and(|loc| {
                locations.iter().any(|(file, line)| {
                    let line_1_based = *line as usize + 1;
                    loc.file == *file
                        && line_1_based >= loc.start_line
                        && line_1_based <= loc.end_line
                })
            })
        });
        if let Some(callee_id) = matched {
            if **callee_id != call.caller_id {
                resolved_edges.push((call.caller_id.clone(), (*callee_id).to_string()));
            }
        }
    }

    for state in clients.into_values() {
        if let Some(client) = state.client {
            client.shutdown();
        }
    }
}
