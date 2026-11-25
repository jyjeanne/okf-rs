# Agent integration — P2 modification measurement

The P2 gate (spec §9.1): on 3 real modifications, blast-radius recall ≥ 0.9 and 0 unpredicted broken tests (definitions in spec §9.2, `A` as decided in D-12). Tasks are replays of merged commits from this repository's own history (spec §9.3).

**Status:** draft, parked on 2026-11-25 (branch `wip/p2.3-measurement`) while the existing changes are stabilized for a release. This is **not yet the pre-registration**: that is the commit that fills in the tasks and ground truth below, made before any okf-rs command runs on a selected pre-commit tree. No okf-rs command has run on one so far.

## Discipline

- **The selection rules below were written before any candidate commit was examined.** Candidates are then taken in a fixed order; nothing is chosen by hand.
- **Ground truth never comes from okf-rs.** `A` comes from `git diff`, a text-only helper (`examples/agent-workflow/bench/p2/diff-functions.js`: it blanks comments and strings, then matches braces around `fn` items and `impl`/`trait`/`mod` blocks), the compiler and test runner on this repository's own code, and reading the code. No bundle is generated and no okf-rs query runs on a pre-commit tree before this document is committed.
- **Frozen build:** okf-rs at the commit that records this pre-registration (crates identical to `09f79e3`), skill pack 1.4. Unchanged until the run is recorded.
- Building and testing the pre-commit trees is allowed: this is the project's own code (the third-party restriction in §9.3 doesn't apply).

## Selection rules

**Pool:** non-merge commits reachable from `main` at `72105ae`, in `git rev-list --no-merges` order (newest first). Only Rust files under `crates/` count; files under any `fixtures/` directory are test data and are ignored.

**A commit is eligible when all of these hold** (checked with git and the text helper only):

1. **It changes existing behavior:** at least one hunk that isn't whitespace or comments changes a non-test function that exists before the commit.
2. **It comes with a test:** it adds or changes at least one test function (`#[test]` or a `…::test` attribute).
3. **It fits one session:** at most 200 changed code lines (added + removed, ignoring whitespace, blank and comment lines) outside test code, in at most 6 non-test Rust files. Test code is anything in a `tests/` directory, a `tests.rs` file, a `#[cfg(test)]` module, or a function with a test attribute.
4. **Its parent builds:** `cargo test --workspace --locked --no-run` succeeds on the pre-commit tree with today's toolchain. Checked only for commits that pass rules 1–3, in order.

**Take** the first 3 eligible commits. A commit that fails rule 4 is recorded with the reason and the next one is taken.

**Target:** the existing non-test function with the most changed code lines in the reference diff (removed lines inside it plus added lines in the hunks that map to it); ties go to the first in diff order. The task statement names it, in the form a developer would use ("change `X` so that …").

## Ground truth

For each selected commit `C` with parent `C^`:

- **`A_edit`:** existing functions and methods (test or not) whose lines `git diff -U0 -w C^ C` changes, after dropping hunks whose every changed line is blank or a comment. A changed line belongs to the innermost `fn` whose span (from the `fn` keyword to its closing brace, or to `;` for a declaration without a body) contains it; a pure insertion belongs to the function it lands strictly inside. A deleted function counts (its lines are changed). Every mapping is checked by reading the code.
- **Failing tests:** apply only the commit's test-code hunks to `C^` and run `cargo test --workspace --locked`. A test fails if it fails, or if it doesn't compile because its own body uses something `C^` lacks (the compiler's error locations decide which test). Tests that already fail on `C^` without the hunks are listed and left out.
- **`A_test`:** the existing functions and methods each failing test calls directly in its body (plain, path, and method calls, including inside macro arguments), found by reading the test. Standard-library and third-party calls, closures, and macros themselves are left out.
- **`A` = `A_edit` ∪ `A_test`.**
- **Listed, not scored:** functions the commit adds, and changed items that are not functions (structs, enums, constants, `use` and `mod` lines, attributes).
- **Broken tests:** apply only the commit's non-test hunks to `C^`, keep the old tests, and run `cargo test --workspace --locked`. Every test that fails, or doesn't compile because the source changed under it, is a broken test. Tests that already fail on `C^` are left out. A failing test is re-run once by itself before it's counted, so a flaky failure (seen on Windows: file locks in e2e tests) needs two failures to count.

## Predicted set and scoring

- **`P`** = {target} ∪ `affected_concepts` from `okf-rs graph plan-change <target id> --format json`, run on a bundle that the frozen binary generates from `C^`. The target id is looked up with `okf-rs search`, and its file must match the target's file.
- **Matching:** a concept in `P` matches a function in `A` when both have the same file, the same name, and overlapping line ranges in `C^`. The match is computed by a script and every pair is checked by hand.
- **Recall** per task = |P ∩ A| / |A|. **Recall across the 3 tasks** = the mean of the per-task recalls (the D-10 rule: a mean over task-runs).
- **Unpredicted broken tests** per task: broken tests that aren't in plan-change's `tests` field. Spec §9.2 says "tests that fail after the change". Here that means the reference change, because §9.2 also takes `A` from the reference rather than from the agent's edit. The agent sessions report their own failures separately.
- **Pass** if recall across the 3 tasks is ≥ 0.9 and the total of unpredicted broken tests is 0. Otherwise fail. Per spec, a recall below 0.9 points first at the test heuristic (D-3).
- **Reported alongside, not gated:** per-task recall split into `A_edit` and `A_test`, the listed-not-scored items, plan-change's `limits` and `tests` fields, and the spec-literal recall from before D-12 (`okf-rs impact C^ C`: `added`, `changed`, `removed`), computed after the run.
- The "before/after graph for every refactoring task" clause applies only if a selected commit is a refactoring (no behavior change). Rule 2 makes that unlikely; if none is selected, the clause is recorded as not exercised.

## Agent sessions (adherence, not gated)

Per the plan (P2.3: "record adherence"), each task is also replayed once by the developer agent:

- A fresh git repository holding the `C^` tree as a single commit (no history to read the answer from). Every `.claude/` folder from the old tree is removed; the pack's four skills, three agents, and `.claude/settings.json` are installed; the `okf` MCP server points at the tree; the bundle is generated by the frozen binary and stays untracked (`knowledge/` is in `.gitignore`); `cargo test --workspace --no-run` is run once beforehand so the first test run fits the Bash timeout.
- `claude -p --agent okf-developer`, model `claude-opus-5-5`, permission mode `dontAsk`. Allowed tools: `Read`, `Grep`, `Glob`, `Skill`, `Edit`, `Write`, the `okf` MCP tools, and `Bash` for `cargo`, `okf-rs`, and `git`. User settings are not loaded. The prompt is the task statement, verbatim.
- When a turn ends before any file was edited (the agent stopped for plan approval or anything else), the operator resumes the session with exactly `Approved. Implement it.`, at most twice. Nothing else is said. The read-only filters `head`, `tail`, and `grep` are allowed in Bash as well, so piped test output isn't refused.
- Recorded per session: OKF-DP-001 and OKF-DP-003 per §7.2; the agent's own `P` (every plan-change call before its first edit) and its recall against `A`; whether the commit's tests pass on the agent's result where they compile; tests failing at the end; tokens and cost. One session per task; nothing re-run except a session that exits without a result (re-run once and noted).

## Tasks

*(To be filled in before the pre-registration commit.)*

**Where the work stopped (2026-11-25):**

- **Selection, rules 1–3** (`diff-functions.js` over the pool): the first three eligible commits are `8df80e8` (three `/code-review` fixes in the cold-crate probe, plus de-duplicated ambiguity detection; target `probe_cold_crates` in `crates/okf-analyzer/src/lib.rs`), `c0986c6` (decode only invalid byte runs as Windows-1254; target `read_source_lossy`), and `3604041` (decode non-UTF-8 files as Windows-1254; target `read_source_lossy`). Rule 4 (the parent builds) is not checked yet.
- **`A_edit` checked by reading** for `8df80e8` and `3604041`: the helper's mappings are right. Two notes for the record: in `8df80e8`, the change to `standalone_binary_cold_crate_probe_reports_one_pair_per_crate_with_an_ambiguous_call` (e2e) is a line re-wrap that `-w` doesn't ignore, so the rule keeps it in `A_edit`; the doc-comment hunk above `lsp::probe_cold_crates` that adds `struct ProbeSkips` is a non-function item.
- **Ground-truth test runs: to redo.** The first attempt ran the `pre`, tests-only, and source-only trees (`split-patch.js`) from separate `git archive` exports sharing one `CARGO_TARGET_DIR`. Exported files carry the commit's mtime, so cargo reused the first tree's binaries for the other two and the results are invalid. Run them in one checkout switched with `git checkout`/`git apply`, so changed files get fresh mtimes, with its own target directory.
- Still to do after that: failing and broken tests per task, `A_test` by reading, the task statements, the pre-registration commit, then `setup-p2.sh`/`run-p2.sh` and `score-p2.js`.
