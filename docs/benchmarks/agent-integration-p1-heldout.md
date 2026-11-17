# Agent integration — held-out structural benchmark (Helix)

A fresh, held-out check of the P1 structural result ([`agent-integration-p1-structural.md`](agent-integration-p1-structural.md)). That result passed after receiver-type resolution, but the fix was built after seeing which two Jujutsu tasks the graph got wrong, so those tasks couldn't show whether the fix generalizes. This set uses a different repository and new questions, chosen without looking at the graph.

**Status:** pre-registered in `500d975` (tasks, ground truth, scoring, and gate committed before any okf-rs command ran on Helix); run 2026-11-17. **Verdict: fail.** Accuracy 1.0 for both variants; treatment tokens **1.59×** baseline (per run 1.48–1.71), cost 1.02×. The Jujutsu pass does not generalize to this repository. See [Results](#results).

## Held-out discipline

- **Different repository:** [Helix](https://github.com/helix-editor/helix) at `49986e66` (2026-11-16): 251 Rust files, 108k lines, a text editor built from many small command functions and macros (`current!`, `view_mut!`). Not used anywhere in developing okf-rs.
- **No graph during selection.** No bundle was generated and no okf-rs query ran on Helix before this document was committed. Candidates were found with grep and a text-only helper (`enclosing.js`, nearest `fn`/`impl` by scanning), and a text-only mutual-recursion finder for H5; every answer was checked by reading the code.
- **Selection criteria, decided before looking at candidates:** one direct-caller control (a free function with 3–8 production callers, one definition); one method whose name is defined on two or more types, with 8–30 call sites (the most frequent such name was taken); one transitive question to registered commands with a single, unambiguous path into the target; one call path of four hops; one mutual recursion found by the text helper and confirmed by reading.
- **Frozen build:** okf-rs at `3629723` (crates identical to `b1b3d3a`), skill pack 1.3, unchanged after this point.
- Building or running Helix was avoided (third-party build scripts); nothing from it was executed.

## Tasks and ground truth

| # | Question (verbatim) | Ground truth |
|---|---|---|
| H1 | List every function in Helix that calls `rope_is_line_ending` directly. | Required (3): `move_prev_paragraph`, `move_next_paragraph` (`helix-core/src/movement.rs`), `textobject_paragraph` (`helix-core/src/textobject.rs`). |
| H2 | List every function in Helix that calls `View::ensure_cursor_in_view`. | Required (18): `Application::handle_config_events` (application.rs); in `commands/typed.rs`: `reload`, `reload_all`, `abort_goto_line_number_preview`, `update_goto_line_number_preview`, `sort`, `reflow`, `reset_diff_change`, `read`; in `commands.rs`: `command_palette`, `make_format_callback`, `replace_selections_with_register`, `shell`; `EditorView::handle_event` (ui/editor.rs, two call sites); `raw_regex_prompt` (ui/mod.rs); in `helix-view/src/editor.rs`: `Editor::_refresh`, `Editor::replace_document_in_view`, `Editor::ensure_cursor_in_view`. **Not callers:** `EditorView::handle_mouse_event` and `Editor::focus` (they call `Editor::ensure_cursor_in_view`, with a view id); `ensure_cursor_in_view_center` is a different method. |
| H3 | Which typable commands (`:` commands) in Helix can end up calling `Editor::close_document`, and through which call chain does each one reach it? | Required commands (8), each calling `buffer_close_by_ids_impl`, the only caller of `Editor::close_document`: `:buffer-close` (`buffer_close`), `:buffer-close!` (`force_buffer_close`), `:buffer-close-others` (`buffer_close_others`), `:buffer-close-others!` (`force_buffer_close_others`), `:buffer-close-all` (`buffer_close_all`), `:buffer-close-all!` (`force_buffer_close_all`), `:write-buffer-close` (`write_buffer_close`), `:write-buffer-close!` (`force_write_buffer_close`). Either the `:` name or the function name counts. |
| H4 | What chain of function calls leads from the `goto_definition` command to `jump_to_position`? | `goto_definition` → `goto_single_impl` → `goto_impl` (in the callback run on the language server's response) → `jump_to_location` (the single-location case) → `jump_to_position`. `ls.goto_definition(..)` inside the command is the LSP client's method of the same name, not part of the path. |
| H5 | When Helix elaborates a parsed snippet, which functions call each other recursively, and which function starts that recursion? | Cycle members (2): `Snippet::elaborate` ↔ `Snippet::elaborate_placeholder` (`helix-core/src/snippets/elaborate.rs`); `elaborate` also calls itself for a variable's default. Started by `Snippet::new` (reached from `Snippet::parse`). **Not members:** `Snippet::elaborate_choice` and `Snippet::elaborate_transform` call `TabstopIdx::elaborate`, a different function of the same name. |

## Scoring (identical to the Jujutsu set)

- **H1, H2, H3 (sets):** recall = required items named ÷ required items; precision = correct items ÷ items claimed as callers (or commands). Score = F1.
- **H3 chains:** recorded, not scored.
- **H4 (path):** 1 if the five required functions appear in order with no wrong hop, else 0.
- **H5:** F1 over the two cycle members × 0.5, plus 0.5 if `Snippet::new` is named as the function that starts the recursion (naming `Snippet::parse` as the public way in as well is fine). Claiming `elaborate_choice`, `elaborate_transform`, or `TabstopIdx::elaborate` as members counts against precision.
- **Accuracy** = mean task score per variant over all task-runs.

## Gate and repetition (identical to D-9 and D-10)

- n = 3 runs of the full 10 sessions (5 tasks × baseline/treatment), launched one after another; same harness, isolation, model (`claude-opus-5-5`), and skill pack 1.3.
- Accuracy = mean over all 15 task-runs per variant; token ratio = total treatment tokens ÷ total baseline tokens over the same task-runs.
- **Pass** if treatment accuracy ≥ 0.9, and either accuracy ≥ baseline + 0.15, or token ratio ≤ 0.67 with treatment accuracy ≥ baseline − 0.05.
- No exclusions: a session that exits non-zero or produces no result is re-run once and noted; if it fails again, that task-run scores 0 and its tokens count.
- Reported alongside: per-run ratios, cost, characters read, adherence, and — after the run — how the graph alone answers each task.

## Results

Run 2026-11-17 on okf-rs `500d975` (crates identical to `b1b3d3a`), skill pack 1.3, `claude-opus-5-5`, 3 runs × 10 sessions. All 30 sessions exited cleanly; nothing was re-run or excluded. Bundle: 5,511 concepts from 299 files.

### Accuracy

All 30 answers were read and graded: **every task-run scores 1.0 in both variants.** H1: the 3 callers. H2: exactly the 18 required functions; answers that mention `Editor::focus` and `EditorView::handle_mouse_event` list them separately as indirect callers through the `Editor` wrapper (accurate, not claimed as direct callers), and `ensure_cursor_in_view_center` only as excluded. H3: the 8 commands. H4: the five-function chain; `ls.goto_definition` appears only as the request the closure sends. H5: the `elaborate` ↔ `elaborate_placeholder` cycle, `Snippet::new` as the entry point, `elaborate_choice` and `elaborate_transform` explicitly excluded. Chains (H3) were recorded, not scored; a few list `buffer_gather_*_impl` as if it were a hop.

### Tokens and cost

| Task | Baseline tokens | Treatment tokens | Ratio |
|---|---|---|---|
| H1 direct callers (control) | 91,766 | 177,546 | 1.93 |
| H2 method callers | 242,835 | 263,548 | 1.09 |
| H3 commands reaching a function | 201,478 | 302,437 | 1.50 |
| H4 call path | 110,087 | 246,503 | 2.24 |
| H5 recursion | 105,692 | 208,044 | 1.97 |
| **Total (3 runs)** | **751,858** | **1,198,078** | **1.593** |

| Run | Baseline tokens | Treatment tokens | Ratio | Baseline cost | Treatment cost |
|---|---|---|---|---|---|
| 1 | 231,894 | 395,365 | 1.705 | $0.75 | $0.84 |
| 2 | 229,697 | 372,187 | 1.620 | $0.60 | $0.54 |
| 3 | 290,267 | 430,526 | 1.483 | $0.62 | $0.63 |
| **All** | **751,858** | **1,198,078** | **1.593** | **$1.97** | **$2.01** (1.02) |

- Source characters (Read + Grep): 273,923 baseline vs 211,885 treatment, plus 42,666 from MCP responses. Turns: 87 baseline vs 127 treatment.
- Adherence: skill loaded first on 8/15 treatment task-runs; a source file was read before the first `explore` call on 4/15 (all on H2 and H3).

### Verdict against the pre-registered rule

- Treatment accuracy ≥ 0.9: yes (1.000).
- Accuracy ≥ baseline + 0.15: **no** (1.000 vs 1.000).
- Token ratio ≤ 0.67 at accuracy ≥ baseline − 0.05: **no** (1.593).
- **Held-out gate: fail.**

### What the graph alone answers (checked after the run, as pre-registered)

| Task | Graph alone | Notes |
|---|---|---|
| H1 | correct (3 callers) | |
| H2 | **2 of 18 callers** | Most `view` receivers come from `current!` / `view_mut!` macros (`let (view, doc) = current!(cx.editor);`), which syntax-only receiver typing can't see through. Every treatment answer noticed and fell back to grep. |
| H3 | correct (all 8 commands via `plan-change`) | |
| H4 | correct (`graph path` gives the exact chain) | |
| H5 | correct pair | The self-call `Snippet::elaborate` → itself is missing: the analyzer never records a function calling itself (see findings). |

### Findings

1. **The cost advantage depends on how expensive grep is.** On Jujutsu the baseline needed 300k tokens for the transitive question, and the graph cut that to a third. On Helix the answers are local (mostly in one file, `commands/typed.rs`), the baseline answers in 4–8 turns at ~30–70k tokens, and the graph route's fixed overhead — loading the skill, `search`, `explore`/`graph`, then a confirming grep — costs more than the whole baseline. The graph was right on 4 of 5 tasks and still lost on cost on all five.
2. **Agents verify graph answers with grep.** Most treatment answers say the graph and a text search "agree". That double-checking is reasonable given the graph's known gaps, but it means the graph adds cost rather than replacing search on easy questions.
3. **Receiver typing doesn't see through macros.** It generalized to Helix's plain parameters, fields, and chains (H1, H3, H4, H5 are graph-correct), but not to macro-bound values, which Helix uses for most `view`/`doc` access. The H2 graph answer was 2 of 18.
4. **Self-calls are never recorded.** `split_ambiguous_calls` skips any edge from a function to itself, so self-recursion never appears as a `Calls` edge or as a cycle in a real bundle, contradicting spec §4.1.4's cycle definition (which counts self-recursion) and `plan-change`'s self-recursion handling (exercised only by in-memory tests).
5. **One model, two repositories, ten structural questions.** Taken together, the two pre-registered structural sets point to a narrower claim than P1 made: the graph lowers agent cost where answering by grep is expensive (cross-crate transitive reach, widely shared method names), and raises it where the answer is local.

### Options (decision needed; not taken here)

1. **Accept the fail.** P1's structural-cost claim does not hold in general; record the narrower finding and decide P2 on its own merits.
2. **Route by question type.** Change the skill so the graph is used for transitive and cross-module questions and grep for local lookups, then test that routing on a *new* pre-registered held-out set (a third repository) mixing both kinds. Re-running this Helix set after tuning would no longer be held-out.
3. **Fix the two concrete defects regardless of P1:** record self-call edges (a correctness bug against the spec), and decide whether to attempt common macro patterns for receiver typing (`current!`-style tuples) or document the limit.
