# Implementation plan — Coding agent integration

**Source:** `docs/OKF_Agent_Integration_Spec_EN_v1.md` (v1.1, 2026-11-17)
**Status:** P0 complete. P1.1–P1.6 implemented on branch `feat/agent-integration-p1` (2026-11-17). P1.7 run twice on 2026-11-17. On okf-rs: adherence 5/5, file ratio 0.46 (fail; floor 0.38 on that repo). Re-run on Jujutsu (456 Rust files): adherence 4/5, file ratio **0.87**, source + MCP characters +9% (fail). After the `explore` fix (locations, capped lists; `4e85e4c`), Jujutsu run 2: adherence 5/5, Grep output −59%, but file ratio 1.23 and tokens 2.3× baseline (fail): graph round trips add turns. See `docs/benchmarks/agent-integration-p1.md` and `docs/benchmarks/agent-integration-p1-jj.md`. **Decision (2026-11-17, project owner): P1 re-scoped to structural questions (spec D-9)**; the new gate is P1.8. P1.8 ran once and failed narrowly on its token criterion (0.77× vs ≤ 0.67×, accuracy tied at 1.0). After receiver-type resolution, the pre-registered 3-run re-measurement **passed** (tokens 0.525×, accuracy 1.0 for both). The pre-registered **held-out** set on a different repository (Helix) then **failed** (tokens 1.59×, accuracy 1.0 for both): the cost advantage holds where grep is expensive (Jujutsu's cross-crate questions) and reverses where answers are local (`docs/benchmarks/agent-integration-p1-heldout.md`). **Decision (D-11): the narrower claim is accepted** — the graph saves agent cost on questions that are expensive to answer by search, not on local lookups — and P1 is closed on that basis. The self-call bug found on Helix is fixed. **Next: P2**, decided on its own merits; its modification tasks (blast radius of a change) are the cross-module kind where the graph paid off.

### P1 progress

| Item | Status | Where |
|---|---|---|
| P1.1 `plan-change` core | Done — 18 unit tests | `crates/okf-query/src/plan_change.rs` |
| P1.2 config keys | Done — `[plan_change]`, `[agents]`; invalid values warn and keep defaults | `crates/okf-core/src/config.rs` |
| P1.3 CLI | Done — `okf-rs graph plan-change <id> [bundle] [--project] [--format json\|text]`; e2e golden test | `crates/okf-cli/src/main.rs`, `tests/fixtures/plan-change/authenticate.json` |
| P1.4 MCP | Done — `graph` relation `plan-change`, config read once at server start | `crates/okf-mcp/src/{main,tools}.rs` |
| P1.5 directive | Done — opt-in; output unchanged when off; e2e test through `init` | `crates/okf-generator/src/agents.rs` |
| P1.6 pack | Done — `okf-explore` Skill, README with setup and the 5 fixed tasks | `examples/agent-workflow/` |
| P1.8 structural gate (D-9, D-10) | **Jujutsu: pass; held-out Helix: fail.** Jujutsu after receiver-type resolution (`b1b3d3a`), 3 runs: accuracy 1.0 both, tokens 0.525×. Held-out Helix (pre-registered `500d975`), 3 runs: accuracy 1.0 both, tokens 1.59×. Graph right on 4/5 Helix tasks but its fixed overhead exceeds grep's cost on local questions; macro-bound receivers unresolved; self-calls never recorded | `bench/tasks/jj-structural.tsv`, `docs/benchmarks/agent-integration-p1-structural.md` |
| P1.7 measurement | **Run three times — fail.** okf-rs: 13 → 6 files (0.46), adherence 5/5. Jujutsu run 1: 15 → 13 files (0.87), adherence 4/5. Jujutsu run 2 (after the `explore` fix): 13 → 16 files (1.23), adherence 5/5, tokens 2.3× | `docs/benchmarks/agent-integration-p1.md`, `docs/benchmarks/agent-integration-p1-jj.md`, harness in `examples/agent-workflow/bench/` |

### P2 progress

| Item | Status | Where |
|---|---|---|
| Prerequisite: stale files | Done — `generate`/`watch` remove files an earlier run wrote for concepts that no longer exist (`d6a1bd7`) | `crates/okf-generator/src/lib.rs` |
| P2.1 skills | Done — `okf-impact`, `okf-refactor`, `okf-graph-validation` (pack 1.4) | `examples/agent-workflow/.claude/skills/` |
| P2.2 agents | Done for Claude Code, verified in real sessions (architect: question and refactor plan; reviewer; developer under `--agent`, both protocol steps). OpenCode files follow the V2 docs, not run (OpenCode not installed). Findings: `Bash(<pattern>)` in a subagent's `tools` doesn't limit the shell, so read-only Bash comes from `.claude/settings.json` allow rules; O-5 answered from docs; O-6 closed (`diff` = `impact` = `review` worktree mechanism) | `examples/agent-workflow/.claude/agents/`, `.opencode/agents/`, spec §6.3, §6.5 |
| P2.3 measurement | Not started. Two graph limits found in P2.2 bear on it: `diff`/`impact` don't see body-only edits (so `A` can't come from `impact` alone), and Rust calls inside macro arguments are not extracted (callers and tests reached through `assert_eq!`/`format!` are missing from `P`) | — |

**Unplanned fix in P1.5.** `okf-rs init` overwrote `okf.toml` with only its `output` line, dropping `[diff]` and (now) `[agents]` on every re-run, which would have made the opt-in directive disappear. `config::write_default` now replaces only the top-level `output` line of an existing file.

**Finding that affected P2 (test heuristic) — resolved before P2.** Running `plan-change` on this repository's own bundle (`functions/crates/okf-graph/src/Graph/transitive_callers`) showed Rust unit tests in `#[cfg(test)]` modules carrying no marker: `tests` was empty, `no_test_reaches_target` fired as a false alarm, and the unrecognized tests took 4 of the 5 `recommended_inspection` slots. Fixed by (1) the Rust extractor tagging `#[test]`-style functions and everything inside `#[cfg(test)]` modules with `test` (`crates/okf-tree-sitter/src/rust.rs`), (2) a fourth heuristic rule, `tag`, and (3) inspection rules 3 and 4 skipping tests. Re-measured: 36 of the 51 affected concepts are recognized as tests, the remaining 15 are all production code, and the inspection list holds only production callers (spec §4.1.5).
**Scope:** P0–P2 in detail; P3 and P4 listed for ordering only.

Each phase ships as its own branch and pull request, following the existing workflow (`chore/...`, `feat/...`), with `cargo fmt --all`, `cargo clippy --workspace --all-targets -- -D warnings`, and `cargo test --workspace --locked` passing before merge.

---

## P0 — Spec fixes

Goal: the spec is accepted as the contract for P1.

1. Apply the findings of the independent spec review — done 2026-11-17 (spec §14 lists the changes; the review's blockers were the OKF-DP-003 ordering, the non-existent `okf-mcp serve` command, and the architect's shell allowlist, all fixed).
2. Resolve open items O-3 (directive opt-in) and O-4 (config keys) explicitly, or record them as deferred.
3. Confirm D-1 to D-6 (spec §13). Default for each is the one written in the spec.

**Done when:** the spec has no `[PLANNED]` tag on anything P1 will not deliver, and its open-items table has no "Open" rows for P1 work.

---

## P1 — Foundations

Goal: `plan-change` works end to end; the `okf-explore` Skill and the protocol directive exist; the exploration exit criterion is measured.

### P1.1 — Core query: `plan-change`

**Where:** new module `crates/okf-query/src/plan_change.rs`, re-exported from `okf-query`'s `lib.rs`. Follow the existing pattern: a `plan_change(bundle, id)` wrapper that loads concepts, and `plan_change_from_concepts(concepts, id)` that does the work (the same split `explore` uses, so MCP can reuse the loaded-bundle cache).

**Reuse, do not reimplement:**
- `okf_graph::Graph::build`, `get`, `callers`, `callees`, `transitive_callers(id, None)`, `owning_module`, `cycles()`, `okf_graph::is_public_api`.
- `okf_graph::is_structural` for the 4.1.7 error case.
- Ranked search (`search_ranked`) only to build the candidate list in the "id not found" error. It is not used to resolve the target.

**Output:** a `PlanChange` struct with `serde::Serialize`, rendered with `serde_json` (both already dependencies of `okf-query`). Schema tag `"okf-plan-change/1"`. Text rendering for `--format text` is a plain function over the same struct.

**Tests (unit, in the module):**
- Each field of §4.1.4 on a small fixture graph built in the test.
- Each risk signal in §4.1.6 fires on its trigger and not otherwise; signal order is fixed.
- `recommended_inspection`: rule order, dedup by `ref`, cap of 5.
- Cycle: target in a 2-concept cycle lists the other member in `cycles`.
- Tests heuristic (§4.1.5): one fixture per rule (path segment, file-name pattern, name prefix), plus a negative case.
- Errors: unknown id (with candidates), structural id.
- Determinism: two calls return identical JSON.

**Done when:** `cargo test -p okf-query plan_change` passes.

### P1.2 — Configuration keys

**Where:** `crates/okf-core/src/config.rs`. The current `Config` struct has `output` and `diff` only. Add:

```toml
[plan_change]
large_blast_radius = 10        # default 10 (spec §4.1.6 rule 3)
boundary_terms = []            # default empty; rule 6 never fires when empty

[agents]
protocol = false               # default false; see P1.5 (spec O-3)
```

Unknown keys: keep the current behavior of `okf.toml` parsing (check how `diff` handles unknown keys before adding these, and match it).

**Tests:** defaults when the section is absent; values parsed; invalid types produce the existing config error style.

### P1.3 — CLI subcommand

**Where:** `crates/okf-cli/src/main.rs`. The `graph` subcommands are a `Subcommand` enum (line ~416, `Callers { … }` at ~419, dispatched at ~1550). Add `PlanChange { id, bundle, format }` and dispatch to `okf_query::plan_change`.

- `--format json` (default) or `--format text`.
- Exit codes: 0 on success; 1 on the errors in spec §4.1.7.
- Flags: `--project` and `--output` as the other `graph` subcommands accept them. Confirm the exact names in `crates/okf-cli/src/main.rs` before implementing (spec §4.1.2).
- Output key order and two-space pretty-printing are fixed by spec §4.1.3, because the golden-file test depends on them.

**Tests:** e2e in `crates/okf-cli/tests/e2e.rs`: run `generate` on a fixture project, then `graph plan-change <id> --format json`; compare against a golden file in `tests/fixtures/plan-change/`. Add one error-case test (unknown id).

### P1.4 — MCP relation

**Where:** `crates/okf-mcp/src/tools.rs`.
- Add `"plan-change"` to the `relation` enum in the `graph` tool schema (line ~72).
- Add a match arm in `graph_relation` (around lines 143–175) that calls `plan_change_from_concepts(&cache.get_or_load(bundle)?, id)`.
- Keep the schema change to one enum value. Do not add a tool (spec D-1).

**Tests:** existing MCP tests in the crate cover the `graph` tool; add one case for `plan-change` with a fixture bundle. Check the tool-selection benchmark in `benchmarks/mcp-tool-selection/` still runs.

### P1.5 — Protocol directive (opt-in)

**Where:** `crates/okf-generator/src/agents.rs`, `render_section`. Add the directive text from spec §7.1 inside the managed block, emitted only when `[agents] protocol = true`.

Because the managed block is regenerated by `okf-rs init`, the directive is written by the generator, not hand-edited into `AGENTS.md`.

**Tests:** `render_section` output with the flag off (byte-identical to today's output, so no existing user sees a change) and on (contains the directive, ≤ 12 lines). Add a regression test that the existing `agents.rs` tests still pass unchanged.

**Dependency:** P1.2 for the config key.

### P1.6 — `okf-explore` Skill and the example pack

**Where:** `examples/agent-workflow/`:
- `.claude/skills/okf-explore/SKILL.md`, the spec §5.3 text, updated to MCP names (§4.3) and with `metadata.okf_pack: "1.1"`.
- `README.md`: how to copy the pack, and the task list from spec §9.3.
- `CLAUDE.md` / `AGENTS.md` are not added here; the pack documents the directive (P1.5) and the user enables it.

**Check before writing:** the spec's `okf-explore` examples must be valid in Claude Code. The skill's `description` must trigger on the intended requests. Verify by running the skill on one exploration question.

### P1.7 — Measurement of P1 exit criterion

**Procedure (spec §9.2–9.3):**
- Pick the 5 exploration tasks from this repository's history. Record them in `examples/agent-workflow/README.md` before the first run.
- For each task, run twice: baseline (no okf-mcp, no Skill, grep allowed) and treatment (okf-mcp + `okf-explore` Skill).
- Record per run: distinct source files read, tokens, whether the Skill was loaded, and adherence checks §7.2 (source-first violation).
- Store results in `docs/benchmarks/agent-integration-p1.md` (new; one table per run).

**Exit criterion:** the spec §9.1 P1 row — the Skill loads and is followed without prompting on all 5 tasks, and treatment reads ≤ 1/3 the distinct source files of baseline.

If the criterion fails, record the result and stop; do not start P2 (spec §9.1 gating).

### P1 risks

| Risk | Mitigation |
|---|---|
| Skill does not trigger automatically | Adjust the `description` (it is the trigger), measure, don't add prompting |
| Test heuristic gives empty `tests` on this repo | Expected for some layouts; the `limits` field says so. Check the output on this repo before the baseline run |
| `plan-change` slows MCP for large bundles | Reuses the existing bundle cache; time `graph plan-change` on the repo's own bundle in P1.3 |
| Directive changes generated `AGENTS.md` for everyone | Default off; the byte-identical test in P1.5 proves it |

---

## P2 — Impact and refactor

Goal: the modification loop works end to end, with a measured blast-radius recall.

### P2.1 — Skills

- `okf-impact`: explore → `plan-change` → plan. Output format fixed by the Skill (direct, indirect, API, tests, risks), so the result is comparable across runs.
- `okf-refactor`: modules → communities → cycles → path → plan → implement → regenerate → `okf-rs diff` between two git refs. The Skill must **commit or use refs**: `okf-rs diff` compares refs, not the working tree. Document this as step 1.
- `okf-graph-validation`: `generate --check-fresh` → `generate` → `validate` → `diff` → affected tests.

Each Skill: body under 1,500 words, numbered steps, MCP names from §4.3.

### P2.2 — Agent definitions

Written from spec §6.5 (verified 2026-11-17). Two formats:
- Claude Code: `.claude/agents/okf-architect.md`, `okf-developer.md`, `okf-reviewer.md`. The architect's `tools` list names `Read, Grep, Glob` and the `mcp__okf__*` tools explicitly (spec §6.5).
- OpenCode: `.opencode/agents/` with the current `permissions` array form.

**Architect MCP access in Claude Code.** The explicit tool list in spec §6.5 is used, because wildcard syntax in `tools` is unverified. Confirm in a real session that the listed MCP tools are available to the subagent before the file is committed.

**Architect shell access (OpenCode and Claude Code).** Read-only `okf-rs` subcommands plus `git diff` and `git log`, as in spec §6.3. `generate`, `init`, and `diff` are not allowed (O-6).

**Verification before commit:** run each agent once in a real session. Check the unverified items in §6.5 (SKILL.md MCP naming, Claude Code wildcard `tools`, `shell` glob syntax, the OpenCode `allowed-tools`-equivalent question), and spec O-5 and O-6. Record results in the spec.

### P2.3 — Modification measurement

- 3 modification tasks, replayed from this repository's merged commits (spec §9.3), reverted in a scratch worktree.
- Procedure per task: run `plan-change` on the target before any edit and record `P` = target ∪ `affected_concepts`; also record the `tests` field and the `limits` field; implement; run the tests. Compute `A` from the **reference** commit, not from the agent's edit: the `added`, `changed` and `removed` ids of `okf-rs impact` between the pre- and post-commit refs, plus the concepts directly called by tests that fail on the unmodified code. Recall = |P ∩ A| / |A| (spec §9.2). Count unpredicted broken tests separately.
- Record blast-radius recall, unpredicted broken tests, and adherence (OKF-DP-001/003).

**Exit criterion (spec §9.1):** recall ≥ 0.9 and 0 unpredicted broken tests across the 3 tasks. If recall < 0.9, the test heuristic (spec D-3) is the first suspect: fix it and re-measure before changing anything else.

### P2 risks

| Risk | Mitigation |
|---|---|
| `impact` and `plan-change` disagree on the same concept | Both use the same `Graph` primitives; add a test that compares them on the fixture |
| Agent file frontmatter wrong for the installed version | Verify in a session (P2.2); pin the tested versions in the README |
| Refactor Skill's before/after snapshot is dirty | Step 1 of the Skill requires a commit or ref |
| A modification task has |A| = 0 | Excluded from recall and reported separately; if fewer than 3 valid tasks remain, extend the task pool (spec §9.3). Never pass the gate on fewer than 3 |

---

## P3 — Review and debug (outline only)

- Skills `okf-review` (git diff → `okf-rs review` → blast radius → AI review) and `okf-debug` (explore → paths → cycles → sources → hypotheses).
- Exit criterion: a real PR review mentions every concept in the graph diff.

## P4 — Feature and docs (outline only)

Deferred. Start only if P1–P3 meet their exit criteria (spec §9.1).

---

## Dependencies

```
P0 ─► P1.1 ─► P1.3 ─► P1.4
        │
        └──► P1.2 ─► P1.5
P1.6 (needs P1.1 for the MCP names to exist)
P1.7 (needs P1.1–P1.6)
P1 gate ─► P2.1 ─► P2.2 ─► P2.3
```

P1.1 is the critical path. Everything else in P1 is small once it exists.

## Verification commands (every PR)

```sh
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace --locked
cargo audit
```

Phase-specific: `cargo test -p okf-query plan_change`, `cargo test -p okf-cli --test e2e plan_change`, `cargo test -p okf-generator`.

## Open items carried into P1

- Spec O-3 (directive default) — default: opt-in.
- Spec O-4 — config keys: implemented in P1.2.
- Spec O-2 — closed: the invocation is `okf-mcp <project-root>` (there is no `serve`). Use it in the pack README. The `claude mcp add` syntax is still unverified.
- Spec §6.5 unverified items — P2.2.
- Spec O-5 (shell pattern syntax in OpenCode permissions) — P2.2.
- Spec O-6 (whether `okf-rs diff` writes outside the project) — decides whether the architect may run `diff`; default: not allowed.
