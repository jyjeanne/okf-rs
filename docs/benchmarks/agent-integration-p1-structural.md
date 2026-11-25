# Agent integration — P1 structural benchmark (Jujutsu)

P1 gate after the 2026-11-17 re-scope (spec §9, decision D-9): whether the `okf` graph gives a coding agent complete, correct answers to **structural** questions — who calls X, what reaches X, the call path between two functions, recursion — that grep can only reconstruct slowly, and at what cost. The earlier "how does X work" runs, which motivated the re-scope, are in [`agent-integration-p1.md`](agent-integration-p1.md) and [`agent-integration-p1-jj.md`](agent-integration-p1-jj.md).

**Status:** pre-registered in `881b196`, run once on 2026-11-17: **fail, narrowly** (accuracy 1.0 for both, tokens 0.77×; bar ≤ 0.67×) — see [Results](#results). After receiver-type resolution (`b1b3d3a`), re-run three times under a rule pre-registered in `c021f98`: **pass** — accuracy 1.0 for both variants, token ratio 0.525 (per run 0.513–0.541), cost 0.37×. See [Re-run results](#re-run-results-3-runs-pre-registered-rule) and its limits.

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

Run 2026-11-17: okf-rs `881b196` (crates identical to `4e85e4c`), skill pack 1.3, `claude-opus-5-5`, Claude Code 2.1.326, n = 1 per task and variant, all 10 in parallel. Same isolation as the earlier Jujutsu runs.

### Accuracy (graded against the ground truth above)

| Task | Baseline | Treatment | Notes |
|---|---|---|---|
| S1 | 1.0 | 1.0 | Both list the 4 required callers and the test. The treatment also rejected a wrong edge in the bundle (`RunWorkspace::persist` reaches `persist_temp_file` only through `TreeState::save`). |
| S2 | 1.0 | 1.0 | Both list exactly the 22 required functions and the optional test, nothing extra. Both summaries miscount (baseline says "26", treatment "24"; each table has 23). The graph had no callers; the treatment said so and grepped. |
| S3 | 1.0 | 1.0 | Both name all 4 commands with correct chains, including `jj gerrit upload`. The graph's `plan-change` covered only the push side (`GitFetch::fetch` has no resolved callers: `fetch` is a widely shared method name); the treatment noticed the gap and grepped the fetch side. |
| S4 | 1 | 1 | Both give `cmd_resolve` → `MergeEditor::edit_files` → (closure via `resolve_files_one_by_one`) → target. |
| S5 | 1.0 | 1.0 | Both name the `visit_directory` ↔ `process_dir_entry` cycle and `TreeState::snapshot` as the entry point. |
| **Accuracy** | **1.0** | **1.0** | |

### Cost

| Task | Variant | Files | Source chars (Read + Grep) | MCP chars | Turns | Tokens | Cost |
|---|---|---|---|---|---|---|---|
| S1 | baseline | 3 | 11,227 | — | 8 | 59,137 | $0.134 |
| S1 | treatment | 0 | 6,815 | 664 | 5 | 51,069 | $0.095 |
| S2 | baseline | 4 | 59,150 | — | 11 | 169,150 | $0.401 |
| S2 | treatment | 0 | 27,823 | 766 | 10 | 182,115 | $0.286 |
| S3 | baseline | 5 | 71,554 | — | 14 | 312,737 | $0.431 |
| S3 | treatment | 0 | 1,710 | 9,728 | 14 | 141,743 | $0.173 |
| S4 | baseline | 2 | 14,992 | — | 6 | 69,622 | $0.145 |
| S4 | treatment | 1 | 2,801 | 1,780 | 9 | 64,427 | $0.115 |
| S5 | baseline | 1 | 16,716 | — | 5 | 50,700 | $0.135 |
| S5 | treatment | 1 | 2,645 | 5,087 | 9 | 66,957 | $0.123 |
| **Total** | baseline | **15** | **173,639** | — | 44 | **661,346** | **$1.25** |
| **Total** | treatment | **2** | **41,794** | **18,025** | 47 | **506,311** | **$0.79** |

- Tokens, treatment ÷ baseline: **0.77** (pre-registered bar: ≤ 0.67). Cost: 0.64. Source characters: 0.24; with MCP output: 0.34. Files: 15 → 2.
- Adherence: the treatment queried the graph before reading any source on 5/5 tasks; it loaded the `okf-explore` skill on 3/5 (S1 and S2 went straight to `search` and `graph callers`).

### Verdict against the pre-registered criterion

- Treatment accuracy ≥ 0.9: **yes** (1.0).
- Accuracy ≥ baseline + 0.15: **no** (1.0 vs 1.0).
- Tokens ≤ 0.67 × baseline at accuracy ≥ baseline − 0.05: **no** (0.77).
- **P1 gate: fail.**

### What the run shows

1. **The strong baseline is accurate on structural questions too.** With grep alone it answered all five correctly, including the transitive question and the non-obvious `jj gerrit upload` path. On these tasks the graph's value is cost, not correctness.
2. **The savings are concentrated where the graph is built to help.** On the transitive question (S3) the treatment used 0.45× the tokens and 0.40× the cost, reading 1.7k characters of source against 72k. On direct-caller and path questions (S1, S4) it was slightly cheaper; on the recursion question (S5) slightly more expensive; on S2, where the graph had no data, about even.
3. **Name-matched call resolution is the main gap.** It missed every caller of `Store::concurrency()` (S2) and of `GitFetch::fetch` (S3), both methods with widely shared names. The skill's checking step worked in both cases, including S3, where the graph returned a plausible but incomplete answer: the agent noticed the missing link and grepped for it.
4. **One sample.** Earlier Jujutsu baseline runs varied by about ±20% between runs, so a 0.77 token ratio is not clearly separated from a value at or under 0.67. The cost ratio (0.64) is further from 1, but cost is not the pre-registered metric.

### Options (decision needed; not taken here)

1. **Accept the fail as pre-registered.** P2 does not start; the graph's demonstrated value is cheaper transitive answers, not better answers.
2. **Repeat the same pre-registered run** (same tasks, ground truth, and scoring; ~$2 per run) to measure the ratio with n = 3 before deciding. This doesn't move the bar, but the spec never stated n, so deciding to average runs is itself a decision to record.
3. **Close the name-resolution gap, then re-run:** better receiver-aware resolution in the Rust extractor, or `--lsp` on repositories it is safe to build. S2 and half of S3 would then come from the graph instead of grep.

Changing the criterion or the task weighting after seeing these results is not an option this document can take: a new criterion needs a new pre-registered task set.

**Decision (2026-11-17, project owner):** option 3 — fix name resolution in the Rust extractor using the receiver's type, then repeat the run three times with the averaging rule written down before running.

## Re-run after receiver-type resolution — pre-registration

Committed before any of these runs.

**What changes:** only okf-rs — build `b1b3d3a`, which resolves Rust method calls by their receiver's type (spec §14). On the target, the graph now lists all 23 callers of `Store::concurrency()` (S2) and the callers of `GitFetch::fetch`, so `plan-change` reaches all four commands of S3. Checked before pre-registering, by running the graph queries directly, not by running any agent session.

**What stays the same:** target (`4351061bb`), the five questions, ground truth, scoring rules, and gate thresholds above; skill pack 1.3; model `claude-opus-5-5`; harness and isolation; one fresh setup (bundle regenerated with `b1b3d3a`) shared by the three runs.

**Repetition and averaging rule:**

1. **n = 3 runs.** Each run is the full set of 10 sessions (5 tasks × baseline/treatment), launched in parallel; the three runs are launched one after another.
2. **Accuracy** per variant = mean task score over all 15 task-runs (5 tasks × 3 runs), each scored as above.
3. **Token ratio** = total treatment tokens over the 15 task-runs ÷ total baseline tokens over the same 15 (ratio of totals, not mean of per-run ratios).
4. **Gate** (thresholds unchanged): treatment accuracy ≥ 0.9, **and** either treatment accuracy ≥ baseline accuracy + 0.15, or token ratio ≤ 0.67 with treatment accuracy ≥ baseline accuracy − 0.05.
5. **No exclusions.** A session that exits non-zero or produces no result is re-run once and the re-run noted; if it fails again, that task-run scores 0 and its recorded tokens count.
6. **Reported alongside, not gated:** per-run token ratios and their spread, cost ratio, characters read, adherence.
7. The first run above (`881b196`, before the fix) is not part of the average: it used a different build.

## Re-run results (3 runs, pre-registered rule)

Run 2026-11-17 after `c021f98` (rule) on build `c021f98` (crates identical to `b1b3d3a`). All 30 sessions exited cleanly; no re-runs were needed, nothing was excluded.

### Accuracy

All 30 answers were read and graded against the ground truth above: **every task-run scores 1.0 in both variants.** Every S1, S2, and S3 answer lists exactly the required items (plus the optional tests), with no extra claims; every S4 chain is `cmd_resolve` → `MergeEditor::edit_files` → (closure via `resolve_files_one_by_one`) → target; every S5 answer names the `visit_directory` ↔ `process_dir_entry` cycle and `TreeState::snapshot`. Not scored but noted: some summaries miscount their own tables (S2 baselines say "24" or "21" over a 23-row table), and the treatment's S2 answers swap the line numbers of `MergedTree::conflicts_matching` and `MergedTree::entries_matching`.

### Tokens and cost, per task (sum of 3 runs)

| Task | Baseline tokens | Treatment tokens | Ratio |
|---|---|---|---|
| S1 direct callers (control) | 167,592 | 138,043 | 0.82 |
| S2 method callers | 569,732 | 165,995 | **0.29** |
| S3 transitive reach | 911,074 | 282,647 | **0.31** |
| S4 call path | 160,012 | 238,226 | 1.49 |
| S5 recursion | 208,706 | 234,351 | 1.12 |
| **Total** | **2,017,116** | **1,059,262** | **0.525** |

| Run | Baseline tokens | Treatment tokens | Ratio | Baseline cost | Treatment cost |
|---|---|---|---|---|---|
| 1 | 723,618 | 377,002 | 0.521 | $1.26 | $0.61 |
| 2 | 667,507 | 361,033 | 0.541 | $1.11 | $0.34 |
| 3 | 625,991 | 321,227 | 0.513 | $1.11 | $0.35 |
| **All** | **2,017,116** | **1,059,262** | **0.525** | **$3.49** | **$1.31** (0.37) |

- Source characters read (Read + Grep): 543,621 baseline vs 41,655 treatment, plus 71,856 from MCP responses (0.21 in total). Files opened: 34 vs 6. Turns: 138 vs 111.
- Adherence: the treatment queried the graph before reading any source on 15/15 task-runs; it loaded the `okf-explore` skill first on 8/15 (the others went straight to the `okf` tools).

### Verdict against the pre-registered rule

- Treatment accuracy ≥ 0.9: **yes** (1.000).
- Accuracy ≥ baseline + 0.15: no (1.000 vs 1.000).
- Token ratio ≤ 0.67 at accuracy ≥ baseline − 0.05: **yes** (0.525; per run 0.513–0.541).
- **P1 gate: pass.**

### What changed since the failed run

The difference is concentrated in the two tasks the graph got wrong before the fix. S2 went from treatment tokens 182,115 (one run, graph had no callers, agent grepped) to 55,332 per run on average; S3 from 141,743 to 94,216. With the callers resolved, the treatment answers S2 from one `graph callers` call. S4 and S5 stay costlier with the graph than without (1.49 and 1.12): for a short path or a two-function cycle, grep plus one read is cheaper than the extra graph round trips.

### Limits of this result

- **Not a held-out test.** The receiver-type fix was built after the first run showed which two tasks the graph got wrong, and S2 and S3 are exactly those tasks. The fix is general — 27 random added and 27 random removed edges on Jujutsu all checked out by hand, and it changes resolution on okf-rs too — but these five questions can't show that on their own. A fresh pre-registered structural task set, on this or another repository, would.
- One model (`claude-opus-5-5`), one repository, five tasks, three runs.
- Accuracy ties at 1.0: the graph's value here is cost, not correctness. A strong agent with grep answered every structural question right.
