//! LSP-backed disambiguation of ambiguous call candidates — see
//! [`crate::analyze_with_cache_lsp`] for the algorithm this plugs into.

use crate::ResolvedEdge;
use okf_core::Project;
use okf_parser::{Confidence, Language, Location};
use okf_tree_sitter::CallCandidate;
use std::collections::HashMap;
use std::time::Duration;

/// Small per-query retry budget applied to *every* ambiguous-call lookup,
/// not just the first one for a given language. `okf_lsp::LspClient::start`
/// already pays the expensive, workspace-wide wait once, up front (see
/// [`okf_lsp::LspClient::wait_until_ready`]), so this only needs to cover
/// residual, per-lookup lag that a workspace-level readiness signal can't
/// see — e.g. one crate among many still finishing its own cross-crate
/// index after the server overall reports ready. That's not hypothetical:
/// it's the exact shape of the one real disagreement found comparing two
/// `rust-analyzer` versions against this project's own source (see
/// `benchmarks/resolver-stability/README.md`) — a cross-crate call whose
/// answer depended on load timing, not on which version was asked.
///
/// This replaces an earlier "retry only the first query per language, then
/// never again" heuristic that used whether *any* prior query for that
/// language had already succeeded as a proxy for "the server is warmed
/// up." That proxy was itself a source of nondeterminism: a fast,
/// intra-crate first query could mark the whole client warmed up while a
/// later, slower cross-crate query right behind it got zero retry budget
/// — the same race being guarded against, just moved one layer up.
///
/// Deliberately small (one retry, one short sleep): unlike the old
/// first-query budget, this one is now paid by *every* call this
/// ambiguous, including ones the server can never resolve at all (e.g. a
/// call dispatched through a trait or generic parameter) — a project with
/// many such calls would otherwise pay the full budget's sleep on each one
/// forever, not just during startup. `LspClient::start`'s own
/// `wait_until_ready` already closes most of the readiness race up front;
/// this only needs to hedge against what that workspace-level wait can't
/// see, not re-absorb a real indexing delay on every lookup.
const DEFINITION_RETRIES: u32 = 2;
const DEFINITION_RETRY_DELAY: Duration = Duration::from_millis(300);

/// Deliberately forces a target crate's own lazy analysis (def-map
/// lowering, type inference — whatever a salsa-backed server like
/// rust-analyzer defers to the first query that actually touches a given
/// crate, rather than doing it during the workspace-wide indexing pass
/// `okf_lsp::LspClient::wait_until_ready` waits on) to happen *before* the
/// real, measured resolution pass in [`resolve_ambiguous_calls`], instead
/// of leaving it to chance which crate a stress test's first real query
/// happens to land in.
///
/// Exists so `okf-rs generate --check-determinism-repeats --warm-crate
/// <name>` can test the demand-driven-analysis hypothesis directly,
/// isolated from the CPU-contention one both share a stress-test run
/// otherwise conflates — see
/// `docs/feedback/2026-08-rust-analyzer-salsa-readiness-review.md` and
/// `benchmarks/resolver-stability/README.md`.
#[derive(Debug, Clone)]
pub struct CrateWarmup {
    /// A crate directory name under `crates/` (e.g. `okf-graph`) —
    /// matched against each ambiguous call's relative path by the prefix
    /// `crates/{crate_name}/`.
    pub crate_name: String,
    /// How many throwaway `textDocument/definition` queries into the
    /// target crate to send before the real resolution pass starts.
    pub queries: usize,
}

/// Selects up to `warmup.queries` entries from `ambiguous` whose file lies
/// under `warmup.crate_name`, in the same order `ambiguous` already has —
/// the set [`resolve_ambiguous_calls`] fires throwaway queries at to warm
/// that crate up. Pulled out as a pure function so the selection logic is
/// unit-testable without spawning a real language server.
fn warmup_targets<'a>(
    ambiguous: &[&'a (CallCandidate, Language, String)],
    warmup: &CrateWarmup,
) -> Vec<&'a (CallCandidate, Language, String)> {
    let prefix = format!("crates/{}/", warmup.crate_name);
    ambiguous
        .iter()
        .filter(|(_, _, relative_path)| relative_path.starts_with(&prefix))
        .take(warmup.queries)
        .copied()
        .collect()
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
///
/// `warmup`, when set, is applied once — right after the *first* client
/// for a language starts — by firing its throwaway queries (see
/// [`CrateWarmup`]) before the real per-call loop below reaches any of
/// them for real. A no-op for a project with only one language client,
/// beyond that one warm-up pass.
pub(crate) fn resolve_ambiguous_calls(
    project: &Project,
    ambiguous: &[&(CallCandidate, Language, String)],
    file_sources: &HashMap<String, String>,
    id_to_location: &HashMap<&str, &Location>,
    candidates_by_name: &HashMap<&str, Vec<&str>>,
    resolved_edges: &mut Vec<ResolvedEdge>,
    warmup: Option<&CrateWarmup>,
) {
    let mut clients: HashMap<Language, Option<okf_lsp::LspClient>> = HashMap::new();

    for (call, language, relative_path) in ambiguous {
        let is_first_use = !clients.contains_key(language);
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

        let Some(client) = client_slot.as_mut() else {
            continue;
        };

        if is_first_use {
            if let Some(warmup) = warmup {
                for (warm_call, _, warm_path) in warmup_targets(ambiguous, warmup) {
                    let Some(source) = file_sources.get(warm_path.as_str()) else {
                        continue;
                    };
                    let Ok(uri) = client.ensure_open(warm_path, source) else {
                        continue;
                    };
                    // Best-effort, single attempt, result discarded: this
                    // exists purely to force the server's own lazy
                    // analysis of this crate before the measured queries
                    // below, not to resolve anything itself.
                    let _ = client.definition(
                        &uri,
                        warm_call.call_site.line,
                        warm_call.call_site.character,
                    );
                }
            }
        }

        let Some(source) = file_sources.get(relative_path.as_str()) else {
            continue;
        };
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
                let resolved_by = okf_lsp::server_command(*language)
                    .map(|(name, _)| name.to_string())
                    .unwrap_or_else(|| format!("{language}"));
                resolved_edges.push(ResolvedEdge {
                    caller: call.caller_id.clone(),
                    callee: (*callee_id).to_string(),
                    resolved_by,
                    confidence: Confidence::Semantic,
                    resolver_version: client.server_version().map(str::to_string),
                });
            }
        }
    }

    for client in clients.into_values().flatten() {
        client.shutdown();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn candidate(caller_id: &str, path: &str) -> (CallCandidate, Language, String) {
        (
            CallCandidate {
                caller_id: caller_id.to_string(),
                callee_name: "get".to_string(),
                call_site: okf_tree_sitter::CallSite {
                    line: 0,
                    character: 0,
                },
            },
            Language::Rust,
            path.to_string(),
        )
    }

    #[test]
    fn warmup_targets_only_selects_the_named_crate() {
        let a = candidate("functions/a", "crates/okf-graph/src/lib.rs");
        let b = candidate("functions/b", "crates/okf-lsp/src/lib.rs");
        let c = candidate("functions/c", "crates/okf-graph/src/other.rs");
        let ambiguous: Vec<&(CallCandidate, Language, String)> = vec![&a, &b, &c];
        let warmup = CrateWarmup {
            crate_name: "okf-graph".to_string(),
            queries: 10,
        };

        let targets = warmup_targets(&ambiguous, &warmup);

        assert_eq!(targets.len(), 2);
        assert!(targets
            .iter()
            .all(|(_, _, path)| path.starts_with("crates/okf-graph/")));
    }

    #[test]
    fn warmup_targets_respects_the_query_budget() {
        let a = candidate("functions/a", "crates/okf-graph/src/a.rs");
        let b = candidate("functions/b", "crates/okf-graph/src/b.rs");
        let ambiguous: Vec<&(CallCandidate, Language, String)> = vec![&a, &b];
        let warmup = CrateWarmup {
            crate_name: "okf-graph".to_string(),
            queries: 1,
        };

        let targets = warmup_targets(&ambiguous, &warmup);

        assert_eq!(targets.len(), 1);
    }

    #[test]
    fn warmup_targets_does_not_match_a_crate_name_that_is_only_a_prefix() {
        // "okf-graph" must not match "okf-graph-extra"'s files -- a naive
        // substring match would.
        let a = candidate("functions/a", "crates/okf-graph-extra/src/lib.rs");
        let ambiguous: Vec<&(CallCandidate, Language, String)> = vec![&a];
        let warmup = CrateWarmup {
            crate_name: "okf-graph".to_string(),
            queries: 10,
        };

        let targets = warmup_targets(&ambiguous, &warmup);

        assert!(targets.is_empty());
    }
}
