//! LSP-backed disambiguation of ambiguous call candidates — see
//! [`crate::analyze_with_cache_lsp`] for the algorithm this plugs into.

use okf_core::Project;
use okf_parser::{Language, Location};
use okf_tree_sitter::CallCandidate;
use std::collections::HashMap;
use std::fs;
use std::time::Duration;

/// A freshly started language server needs time to index a project before
/// `textDocument/definition` returns anything useful — these bound how
/// long a single query waits for that (10s total, in half-second steps),
/// not how long disambiguation runs overall (each subsequent query on an
/// already-indexed server typically returns immediately).
const DEFINITION_RETRIES: u32 = 20;
const DEFINITION_RETRY_DELAY: Duration = Duration::from_millis(500);

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
/// has been attempted.
pub(crate) fn resolve_ambiguous_calls(
    project: &Project,
    ambiguous: &[&(CallCandidate, Language, String)],
    id_to_location: &HashMap<&str, &Location>,
    candidates_by_name: &HashMap<&str, Vec<&str>>,
    resolved_edges: &mut Vec<(String, String)>,
) {
    let mut clients: HashMap<Language, Option<okf_lsp::LspClient>> = HashMap::new();
    let mut sources: HashMap<&str, String> = HashMap::new();

    for (call, language, relative_path) in ambiguous {
        let client_slot =
            clients.entry(*language).or_insert_with(|| {
                match okf_lsp::LspClient::start(*language, &project.root) {
                    Ok(client) => client,
                    Err(e) => {
                        eprintln!(
                            "warning: failed to start a language server for {language}, \
                         skipping LSP disambiguation for it: {e:#}"
                        );
                        None
                    }
                }
            });
        let Some(client) = client_slot else { continue };

        let source = sources.entry(relative_path.as_str()).or_insert_with(|| {
            fs::read_to_string(project.root.join(relative_path)).unwrap_or_default()
        });
        let Ok(uri) = client.ensure_open(relative_path, source) else {
            continue;
        };

        let mut locations = Vec::new();
        for attempt in 0..DEFINITION_RETRIES {
            match client.definition(&uri, call.call_site.line, call.call_site.character) {
                Ok(found) if !found.is_empty() => {
                    locations = found;
                    break;
                }
                Ok(_) if attempt + 1 < DEFINITION_RETRIES => {
                    std::thread::sleep(DEFINITION_RETRY_DELAY);
                }
                _ => break,
            }
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

    for client in clients.into_values().flatten() {
        client.shutdown();
    }
}
