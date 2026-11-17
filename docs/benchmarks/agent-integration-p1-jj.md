# Agent integration — P1 measurement on a larger repository (Jujutsu)

Second run of the P1 exploration benchmark, on a repository where grep-based navigation should be expensive enough for the spec's 3× target (§9.1) to be reachable. The first run, on okf-rs itself, is in [`agent-integration-p1.md`](agent-integration-p1.md).

**Status:** run on 2026-11-17. **Verdict: fail.** Files opened: 0.87 of baseline (criterion ≤ 0.33); adherence 4/5.

## Target and run conditions

| Field | Value |
|---|---|
| Target | [Jujutsu](https://github.com/jj-vcs/jj) at `4351061bb` (2026-11-14), 456 Rust files across ~10 crates (okf-rs: 55) |
| Bundle | 9,522 concepts from 463 files, `okf-rs generate .` (no `--enrich`, no `--lsp`) |
| okf-rs | `84f2d88` (includes Rust test tagging) |
| Agent / model | Claude Code 2.1.326, headless; `claude-opus-5-5` for all 10 runs |
| Tools, settings, MCP | Same as the first run: `Read`, `Grep`, `Glob`, `Skill`; no user settings; `okf` MCP only in the treatment |
| Isolation | `git archive` exports (no history); every `.claude/` folder removed; the target has no `CLAUDE.md`/`AGENTS.md` |
| Harness | `examples/agent-workflow/bench/setup.sh . <jj> 4351061bb <dir>`, `run-all.sh tasks/jj.tsv` |
| Runs | 1 per task and variant (n = 1), in parallel |

## Tasks and ground truth

Fixed in [`bench/tasks/jj.tsv`](../../examples/agent-workflow/bench/tasks/jj.tsv) before the first run.

| # | Question | Commit(s) | Where the answer lives (HEAD) | Key facts for grading |
|---|---|---|---|---|
| J1 | What does jj do on Windows when replacing a file fails because another process has the target file open? | `27a61a1e0` | `local-util/src/file_util.rs`, `local-util/src/backoff.rs` | `persist_temp_file` retries the rename with `BackoffIterator` for ~2 s when the error is transient (Windows only: `PermissionDenied` or `ERROR_SHARING_VIOLATION`); otherwise returns the error |
| J2 | How does jj detect that a Git remote uses a different object format (SHA-1 vs. SHA-256) than the local repository? | `7300fa598`, `77d7362f6`, `41bca8441` | `lib/src/git_subprocess.rs` | Parses the first stderr line of the failed `git` fetch/push for known messages → `GitSubprocessError::ObjectFormatMismatch` |
| J3 | How can a user resolve a conflict by editing the conflict markers in their own text editor, and how does jj read the result back? | `055817606` | `cli/src/merge_tools/mod.rs` | The `:editor` merge tool: materializes markers into a temp file, opens the configured editor, parses the saved markers back |
| J4 | How does jj limit how many backend requests are in flight when it reads or writes many trees at once? | `a83d44023`, `31824c2dd` | `core/src/tree_walk.rs`, `core/src/tree_rewriter.rs` | Bounded future queues (`PrioritizedFutures`, `LimitedFutures`) capped at `Store::concurrency()` |
| J5 | How does jj's diff output choose the source line, such as the enclosing function's signature, that it shows as context for a hunk? | `987c084b8` and earlier | `cli/src/source_symbol.rs`, `cli/src/diff_util.rs` | Language from the file name, per-language symbol patterns (comments and indented lines skipped, generic GNU-diff rule as fallback), nearest preceding symbol line |

## Results

"Files" = distinct files opened with `Read`. "Source chars" = characters returned to the agent by `Read` plus `Grep` (Grep's content mode returns source lines too, which "files" doesn't see). "MCP chars" = characters returned by the `okf` tools.

| Task | Variant | Files | Source chars | MCP chars | Skill first | Source-first | Turns | Tokens | Cost | Correct |
|---|---|---|---|---|---|---|---|---|---|---|
| J1 | baseline | 2 | 21,458 | — | n/a | n/a | 6 | 55,550 | $0.167 | yes |
| J1 | treatment | 2 | 18,806 | 0 | **no** | **yes** | 5 | 56,984 | $0.154 | yes |
| J2 | baseline | 3 | 17,860 | — | n/a | n/a | 7 | 58,105 | $0.164 | yes |
| J2 | treatment | 3 | 14,839 | 6,424 | yes | no | 13 | 113,045 | $0.204 | yes |
| J3 | baseline | 3 | 33,032 | — | n/a | n/a | 9 | 121,944 | $0.244 | partial |
| J3 | treatment | 0 | 10,857 | 11,434 | yes | no | 13 | 115,922 | $0.203 | yes |
| J4 | baseline | 5 | 28,478 | — | n/a | n/a | 12 | 99,038 | $0.242 | yes |
| J4 | treatment | 6 | 30,776 | 3,594 | yes | no | 16 | 140,490 | $0.260 | yes |
| J5 | baseline | 2 | 44,130 | — | n/a | n/a | 7 | 100,353 | $0.244 | yes |
| J5 | treatment | 2 | 42,955 | 18,686 | yes | no | 13 | 173,365 | $0.337 | yes |
| **Total** | baseline | **15** | **144,958** | — | | | 41 | **434,990** | **$1.06** | 4.5/5 |
| **Total** | treatment | **13** | **118,233** | **40,138** | 4/5 | 1/5 | 60 | **599,806** | **$1.16** | 5/5 |

- Files, treatment ÷ baseline: **0.87** (criterion ≤ 0.33).
- Source chars: 0.82; source + MCP chars: **1.09** — the treatment consumed more text than the baseline.
- Tokens +38%, cost +9%, turns +46%.

Grading notes: J3's question allows two readings. The baseline answered with jj's general mechanism (edit the markers in the working copy; jj re-parses them on the next snapshot), which is valid but misses the `:editor` tool the ground-truth commit added; graded partial. Every other answer names the key facts above.

## What the transcripts show

1. **Grep is already near-optimal for "how does X work" questions.** Both variants locate the right file in one or two calls and read the relevant range with `offset`/`limit`, not whole files. The baseline needs 2–5 files per task on a 456-file repository; there is little navigation cost for the graph to remove.
2. **The graph names the code but not its location.** `explore` returns the id, signature, callers, callees, and blast radius, but no file or line range. After every `explore`, the treatment grepped for the function it had just been given (e.g. J3: `explore` on `edit_conflict_markers_in_text_editor`, then `Grep "fn edit_conflict_markers_in_text_editor" -A 70`). The graph adds calls without removing any.
3. **`explore` responses are large on a large repository.** Blast-radius lists of 50+ ids made up most of the 40k MCP characters (J5 alone: 18.7k).
4. **Name-based call resolution misses edges.** In J5, `explore` reported 0 callers for `SourceSymbolScanner::find` (a common method name, left unresolved by Tree-sitter's name matching). The agent noticed, said so in its answer, and fell back to grep. `generate --lsp` exists for exactly this case and wasn't used here.
5. **The skill doesn't always trigger.** J1's question carried precise keywords (Windows, file in use); the agent grepped straight away without loading `okf-explore` or calling the graph.

Across both repositories the pattern is the opposite of the spec's hypothesis: the graph helped on the small repository (source + MCP characters −33%, [first run](agent-integration-p1.md)) and not on the larger one.

## Options (decision needed; not taken here)

1. **Fix what the transcripts point to, then re-run this exact task set:** add the concept's location (file and line range) to `explore`; cap blast-radius lists in `explore` (count plus the first N); optionally run the treatment bundle with `--lsp`. The tasks file is fixed, so the re-run is directly comparable.
2. **Re-scope P1.** UC-1 mechanism questions are grep's best case. The graph's distinctive answers are structural — transitive callers, blast radius, cycles — which grep can only reconstruct slowly. Gate on UC-2 (impact, P2's recall metric) instead of files read for UC-1, and record the change in the spec's decisions log.
3. **Stop at adherence.** Keep the skill and protocol for consistency, without a cost claim.

Per spec §9.1, P2 does not start until one of these is chosen.
