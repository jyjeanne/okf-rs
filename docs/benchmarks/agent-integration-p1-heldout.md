# Agent integration — held-out structural benchmark (Helix)

A fresh, held-out check of the P1 structural result ([`agent-integration-p1-structural.md`](agent-integration-p1-structural.md)). That result passed after receiver-type resolution, but the fix was built after seeing which two Jujutsu tasks the graph got wrong, so those tasks couldn't show whether the fix generalizes. This set uses a different repository and new questions, chosen without looking at the graph.

**Status:** pre-registered. Tasks, ground truth, scoring, and the gate were committed before any okf-rs command ran on Helix.

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

Not run yet.
