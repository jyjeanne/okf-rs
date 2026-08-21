//! LSP-backed disambiguation of ambiguous call candidates — see
//! [`crate::analyze_with_cache_lsp`] for the algorithm this plugs into.

use crate::ResolvedEdge;
use okf_core::Project;
use okf_parser::{Confidence, Language, Location};
use okf_tree_sitter::CallCandidate;
use std::collections::{HashMap, HashSet};
use std::time::{Duration, Instant};

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
/// under `warmup.crate_name` *and* whose language is `language` (the
/// client about to fire the warmup queries), in the same order
/// `ambiguous` already has — the set [`resolve_ambiguous_calls`] fires
/// throwaway queries at to warm that crate up. Pulled out as a pure
/// function so the selection logic is unit-testable without spawning a
/// real language server.
///
/// The `language` filter matters in a multi-language project: without it,
/// warmup fires through whichever language's `LspClient` happens to start
/// first, sending another language's files (matched by path alone) to a
/// server that can't make sense of them — e.g. a Python client asked to
/// open and query a Rust file from `crates/okf-graph/`, tagged
/// `languageId: "python"` by `ensure_open`. Filtering by `language` here
/// keeps warmup scoped to the client it's actually running through.
fn warmup_targets<'a>(
    ambiguous: &[&'a (CallCandidate, Language, String)],
    language: Language,
    warmup: &CrateWarmup,
) -> Vec<&'a (CallCandidate, Language, String)> {
    ambiguous
        .iter()
        .filter(|(_, entry_language, relative_path)| {
            *entry_language == language
                && crate_name_from_path(relative_path).is_some_and(|name| name == warmup.crate_name)
        })
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
                for (warm_call, _, warm_path) in warmup_targets(ambiguous, *language, warmup) {
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

/// One crate's cold-vs-warm probe result: its genuinely first
/// `textDocument/definition` query in the process (cache priming
/// disabled, so this is demand-driven-lowering-cold rather than warm by
/// luck of the indexing order) compared against an immediate repeat of
/// the exact same query, which should hit the by-then-computed, memoized
/// result if the crate's own lowering is what the cold attempt paid for.
/// See [`probe_cold_crates`].
#[derive(Debug, Clone)]
pub struct CrateProbeResult {
    /// A crate directory name under `crates/` (e.g. `okf-graph`).
    pub crate_name: String,
    /// Whether the cold (first-ever) query into this crate came back
    /// with no candidate locations at all.
    pub cold_empty: bool,
    /// How long the cold query took to answer.
    pub cold_elapsed: Duration,
    /// Whether the immediate-repeat (warm) query came back empty.
    pub warm_empty: bool,
    /// How long the warm query took to answer.
    pub warm_elapsed: Duration,
}

/// Probes each crate's genuinely first `textDocument/definition` query
/// against an immediate repeat of the same query, one probe per distinct
/// crate found in `ambiguous`, all within a single process with
/// `rust-analyzer`'s cache priming turned off (see
/// [`okf_lsp::disable_rust_analyzer_cache_priming`]) so demand-driven
/// lowering is back in force and a crate's first touch is reliably cold
/// rather than warm by luck of however much of the crate graph priming
/// already walked before the probe started.
///
/// Turns "wait for a lucky flip once every several `--check-determinism`
/// runs" into "one cold-first measurement per crate, every run" — see
/// `docs/feedback/2026-08-rust-analyzer-salsa-readiness-review.md`.
/// Diagnostic-only: never resolves anything into `resolved_edges`, purely
/// measurement. One `okf_lsp::LspClient` per distinct language, same as
/// [`resolve_ambiguous_calls`]; probes run in `ambiguous`'s own order, one
/// per crate, at that crate's first appearance.
///
/// Why one crate's cold-vs-warm probe never happened, tallied by
/// [`probe_cold_crates`] instead of collapsed into one generic "skipped"
/// count. Distinguishing these matters to the caller (`okf-rs
/// cold-crate-probe`): reporting every skip as "no language server
/// available" would misattribute a skip actually caused by unreadable
/// source text or a rejected `textDocument/didOpen` to a cause the user
/// never actually hit.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ProbeSkips {
    /// The language server for that crate's language never started at all
    /// (not installed, failed to spawn, ...) — see the `eprintln!`
    /// warning [`probe_cold_crates`] prints when this happens.
    pub no_server: usize,
    /// The server started, but the probe itself couldn't run for an
    /// unrelated reason: the source text wasn't available, or the server
    /// rejected opening that file.
    pub other: usize,
}

impl ProbeSkips {
    /// Total crates skipped, regardless of reason.
    pub fn total(&self) -> usize {
        self.no_server + self.other
    }
}

/// Returns the probe results alongside [`ProbeSkips`], so a caller can
/// tell "no ambiguous calls existed at all" apart from "some existed but
/// couldn't be probed" — and, within the latter, a genuinely missing
/// language server apart from every other reason a probe might not run.
pub(crate) fn probe_cold_crates(
    project: &Project,
    ambiguous: &[&(CallCandidate, Language, String)],
    file_sources: &HashMap<String, String>,
) -> (Vec<CrateProbeResult>, ProbeSkips) {
    let targets = probe_targets(ambiguous);

    let mut results = Vec::new();
    let mut skips = ProbeSkips::default();
    let mut clients: HashMap<Language, Option<okf_lsp::LspClient>> = HashMap::new();

    for (crate_name, (call, language, relative_path)) in targets {
        let client_slot = clients.entry(*language).or_insert_with(|| {
            match okf_lsp::LspClient::start_with_init_options(
                *language,
                &project.root,
                Some(okf_lsp::disable_rust_analyzer_cache_priming()),
            ) {
                Ok(client) => client,
                Err(e) => {
                    eprintln!(
                        "warning: failed to start a language server for {language} \
                     (cache priming disabled), skipping the cold-crate probe for it: {e:#}"
                    );
                    None
                }
            }
        });
        let Some(client) = client_slot.as_mut() else {
            skips.no_server += 1;
            continue;
        };
        let Some(source) = file_sources.get(relative_path.as_str()) else {
            skips.other += 1;
            continue;
        };
        let Ok(uri) = client.ensure_open(relative_path, source) else {
            skips.other += 1;
            continue;
        };

        let cold_start = Instant::now();
        let cold = client.definition(&uri, call.call_site.line, call.call_site.character);
        let cold_elapsed = cold_start.elapsed();

        let warm_start = Instant::now();
        let warm = client.definition(&uri, call.call_site.line, call.call_site.character);
        let warm_elapsed = warm_start.elapsed();

        results.push(CrateProbeResult {
            crate_name,
            cold_empty: cold.is_err() || cold.is_ok_and(|v| v.is_empty()),
            cold_elapsed,
            warm_empty: warm.is_err() || warm.is_ok_and(|v| v.is_empty()),
            warm_elapsed,
        });
    }

    for client in clients.into_values().flatten() {
        client.shutdown();
    }

    (results, skips)
}

/// Selects one entry from `ambiguous` per distinct crate — its first
/// appearance in `ambiguous`'s own order, paired with that crate's own
/// name (computed once here rather than re-parsed later) — the set
/// [`probe_cold_crates`] fires its cold-then-warm query pair at. Pulled
/// out as a pure function for the same reason [`warmup_targets`] is:
/// unit-testable without spawning a real language server. Entries outside
/// `crates/` (see [`crate_name_from_path`]) are skipped, not grouped into
/// one bucket.
fn probe_targets<'a>(
    ambiguous: &[&'a (CallCandidate, Language, String)],
) -> Vec<(String, &'a (CallCandidate, Language, String))> {
    let mut seen_crates: HashSet<String> = HashSet::new();
    ambiguous
        .iter()
        .filter_map(|entry| {
            let (_, _, relative_path) = entry;
            let name = crate_name_from_path(relative_path)?;
            seen_crates.insert(name.clone()).then_some((name, *entry))
        })
        .collect()
}

/// Extracts a crate directory name from a `crates/{name}/...`-shaped
/// relative path, or `None` for anything outside `crates/` (a
/// single-package project with no `crates/` layout, say). Shared by
/// [`warmup_targets`], [`probe_targets`], and [`probe_cold_crates`].
fn crate_name_from_path(relative_path: &str) -> Option<String> {
    relative_path
        .strip_prefix("crates/")?
        .split('/')
        .next()
        .filter(|s| !s.is_empty())
        .map(str::to_string)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn candidate(caller_id: &str, path: &str) -> (CallCandidate, Language, String) {
        candidate_lang(caller_id, path, Language::Rust)
    }

    fn candidate_lang(
        caller_id: &str,
        path: &str,
        language: Language,
    ) -> (CallCandidate, Language, String) {
        (
            CallCandidate {
                caller_id: caller_id.to_string(),
                callee_name: "get".to_string(),
                call_site: okf_tree_sitter::CallSite {
                    line: 0,
                    character: 0,
                },
            },
            language,
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

        let targets = warmup_targets(&ambiguous, Language::Rust, &warmup);

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

        let targets = warmup_targets(&ambiguous, Language::Rust, &warmup);

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

        let targets = warmup_targets(&ambiguous, Language::Rust, &warmup);

        assert!(targets.is_empty());
    }

    #[test]
    fn warmup_targets_does_not_fire_through_a_different_languages_client() {
        // Regression test for a real bug /code-review found: without the
        // language filter, a Python file matching the target crate's path
        // would be selected and warmed up through whichever client
        // started first, even a Rust one -- see this function's own docs
        // for the failure this guards against.
        let rust_file =
            candidate_lang("functions/a", "crates/okf-graph/src/lib.rs", Language::Rust);
        let python_file = candidate_lang(
            "functions/b",
            "crates/okf-graph/src/other.py",
            Language::Python,
        );
        let ambiguous: Vec<&(CallCandidate, Language, String)> = vec![&python_file, &rust_file];
        let warmup = CrateWarmup {
            crate_name: "okf-graph".to_string(),
            queries: 10,
        };

        // A Python client starting first (it's the first entry in
        // `ambiguous`) must not pick up the Rust file just because the
        // path matches -- only the Rust file matches `Language::Rust`.
        let rust_targets = warmup_targets(&ambiguous, Language::Rust, &warmup);
        assert_eq!(rust_targets.len(), 1);
        assert_eq!(rust_targets[0].0.caller_id, "functions/a");

        let python_targets = warmup_targets(&ambiguous, Language::Python, &warmup);
        assert_eq!(python_targets.len(), 1);
        assert_eq!(python_targets[0].0.caller_id, "functions/b");
    }

    #[test]
    fn crate_name_from_path_extracts_the_crate_directory_name() {
        assert_eq!(
            crate_name_from_path("crates/okf-graph/src/lib.rs"),
            Some("okf-graph".to_string())
        );
        assert_eq!(
            crate_name_from_path("crates/okf-graph/src/nested/mod.rs"),
            Some("okf-graph".to_string())
        );
    }

    #[test]
    fn crate_name_from_path_is_none_outside_crates() {
        assert_eq!(crate_name_from_path("src/lib.rs"), None);
        assert_eq!(crate_name_from_path("crates/"), None);
        assert_eq!(crate_name_from_path("crates"), None);
    }

    #[test]
    fn probe_targets_picks_one_entry_per_distinct_crate_at_its_first_appearance() {
        let a1 = candidate("functions/a1", "crates/okf-graph/src/a.rs");
        let a2 = candidate("functions/a2", "crates/okf-graph/src/b.rs");
        let b1 = candidate("functions/b1", "crates/okf-core/src/lib.rs");
        let ambiguous: Vec<&(CallCandidate, Language, String)> = vec![&a1, &a2, &b1];

        let targets = probe_targets(&ambiguous);

        assert_eq!(targets.len(), 2);
        assert_eq!(targets[0].0, "okf-graph");
        assert_eq!(targets[0].1 .0.caller_id, "functions/a1");
        assert_eq!(targets[1].0, "okf-core");
        assert_eq!(targets[1].1 .0.caller_id, "functions/b1");
    }

    #[test]
    fn probe_targets_skips_entries_outside_crates() {
        let outside = candidate("functions/c", "src/lib.rs");
        let ambiguous: Vec<&(CallCandidate, Language, String)> = vec![&outside];

        let targets = probe_targets(&ambiguous);

        assert!(targets.is_empty());
    }
}
