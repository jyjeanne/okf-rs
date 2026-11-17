# Agent integration — P1 structural benchmark (Jujutsu)

P1 gate after the 2026-11-17 re-scope (spec §9, decision D-9): whether the `okf` graph gives a coding agent complete, correct answers to **structural** questions — who calls X, what reaches X, the call path between two functions, recursion — that grep can only reconstruct slowly, and at what cost. The earlier "how does X work" runs, which motivated the re-scope, are in [`agent-integration-p1.md`](agent-integration-p1.md) and [`agent-integration-p1-jj.md`](agent-integration-p1-jj.md).

**Status:** pre-registered. Tasks, ground truth, and scoring below were fixed and committed before the first run.

## Target

[Jujutsu](https://github.com/jj-vcs/jj) at `4351061bb`, 456 Rust files; the same isolated copies and harness as the earlier Jujutsu runs (`setup.sh`, `run-all.sh tasks/jj-structural.tsv`). The bundle is generated without `--lsp` (the default configuration), so unresolved method calls are part of what is measured.

## Ground truth method

Ground truth never uses okf-rs output. For each target:

1. Grep the whole source tree for the target's call sites (`name(`, `.name(`, `::name(`).
2. Map each call site to its enclosing function with a text-only helper (nearest preceding `fn`, nearest `impl`/`trait` header above it), then review every result by reading the code (receiver types for method calls, closures, trait impls).
3. For transitive questions, repeat step 1–2 for each caller, level by level, up to the stop condition (a `cmd_*` command handler).

The graph's `cycles` output was used only to *find a candidate* for S5; its two members and entry point were then verified by reading the code.

## Tasks and ground truth

| # | Question (verbatim) | Ground truth |
|---|---|---|
| S1 | List every function in jj that calls `persist_temp_file` directly. | Required (4): `DefaultIndexStore::associate_index_with_operation` (`default-index/src/store.rs`), `TreeState::save` and `CheckoutState::save` (`lib/src/local_working_copy.rs`), `SimpleWorkspaceStore::write_store` (`lib/src/simple_workspace_store.rs`). Optional test: `test_persist_temp_file_target_open_briefly`. |
| S2 | List every function in jj that calls `Store::concurrency()`. | Required (22): `cmd_file_search`, `cmd_log`, `compute_operation_commits_diff` (cli); `materialized_diff_stream`, `copies_tree_diff_stream`, `MergedTree::conflicts_matching`, `MergedTree::entries_matching`, `MergedTree::diff_internal`, `MergedTreeBuilder::write_merged_trees`, `commits` (the `RevsetStreamExt` blanket impl), `Tree::entries_matching`, `TreeMerger::merge`, `TreeMerger::enqueue_tree_read`, `TreeMerger::enqueue_file_merge`, `rewrite_trees` (core); `DefaultIndexStore::build_index_at_operation` (default-index); `process_commits` (annotate), `walk_predecessors`, `import_refs_inner`, `TreeState::update`, `MutableRepo::index_commits`, `find_duplicate_divergent_commits` (lib). Optional test: `test_annotate_merge_does_not_load_ancestors_of_unused_side`. Not a caller: `Store::concurrency` itself (it calls `Backend::concurrency`). |
| S3 | Which `jj` commands can end up calling `parse_object_format_mismatch`, and through which call chain does each one reach it? | Required commands (4): `jj git fetch`: `cmd_git_fetch` → `GitFetch::fetch` → `GitSubprocessContext::spawn_fetch` → `parse_git_fetch_output` → target. `jj git clone`: `cmd_git_clone` → `fetch_new_remote` → `GitFetch::fetch` → … `jj git push`: `cmd_git_push` → `push_refs` → `push_updates` → `GitSubprocessContext::spawn_push` → `parse_git_push_output` → target. `jj gerrit upload`: `cmd_gerrit_upload` → `push_updates` → … |
| S4 | What chain of function calls leads from the command handler of `jj resolve` to `edit_conflict_markers_in_text_editor`? | `cmd_resolve` → `MergeEditor::edit_files` → `edit_conflict_markers_in_text_editor`. `edit_files` passes a closure containing the call to `resolve_files_one_by_one`, which runs it per file; naming that is correct but not required. |
| S5 | When jj snapshots the working copy, which functions call each other recursively to walk the directory tree, and which function starts that recursion? | Cycle members (2): `FileSnapshotter::visit_directory` ↔ `FileSnapshotter::process_dir_entry` (`lib/src/local_working_copy.rs`). Started by `TreeState::snapshot`. |

## Scoring (fixed before the run)

- **S1, S2, S3 (sets):** recall = required items named ÷ required items; precision = correct items ÷ items the answer claims as callers (or commands). Optional tests count neither way. Score = F1.
- **S3 chains:** recorded per command (correct / partly correct / missing), not part of the score.
- **S4 (path):** 1 if the three required hops appear in order with no wrong hop, else 0.
- **S5:** F1 over the two cycle members, × 0.5, plus 0.5 if the entry point is right.
- **Task score** in [0, 1]; **accuracy** = mean over the five tasks, per variant.
- Cost metrics as before (spec §9.2): tokens, cost, turns, characters read per tool.

**P1 exit criterion (spec §9.1 after D-9):** treatment accuracy ≥ 0.9, **and** either treatment accuracy ≥ baseline accuracy + 0.15, **or** treatment tokens ≤ 0.67 × baseline tokens with accuracy ≥ baseline − 0.05. Adherence (skill used first, graph before source) is recorded but not gated.

## Results

Not run yet.
