# okf-rs — Coding Agent Integration (Claude Code / OpenCode)
## Specification v1.1 — "OKF as an architectural knowledge layer"

**Project:** okf-rs (github.com/jyjeanne/okf-rs)
**Status:** Specification v1.1 — reviewed 2026-11-17 (findings applied); awaiting approval of the implementation plan
**Date:** 2026-11-17
**Supersedes:** v1 (2026-10-03). Changes from v1 are listed in §14.

> **Notation convention:** items tagged `[PLANNED]` describe capabilities that do
> not yet exist in okf-rs at the time of writing. Everything else refers to
> features already implemented. Items tagged `[DECIDED]` record a default chosen
> in v1.1 that can be revisited with a documented reason (§13). Items tagged
> *Unverified* were not confirmed against documentation or code and must be
> tested before being relied on.

---

# 1. Context and problem

okf-rs transforms a codebase into a knowledge graph in the OKF format, exposed via MCP.
Its MCP server provides `search`, `search_ranked`, `search_semantic`, `explore`, `coverage`, and a consolidated `graph` tool (relations `callers`, `callees`, `path`, `explain`, `api`, `cycles`, `modules`, `isolated`, `stats`, `layers`, `domains`, `communities`, `patterns`, `features`). The CLI additionally provides `review`, `impact`, and `diff`, which compare two git refs and are **not** MCP tools. `okf-rs init` generates the AGENTS.md / CLAUDE.md integration points.

**The risk to avoid:** positioning okf-rs as "one more MCP tool the agent can call." In that case it becomes an optional tool the agent forgets about as soon as the task gets urgent. The project owner reports this behavior (the agent falls back to grep/find/cat under time pressure); it is not yet measured (§9).

**The goal:** make OKF the *mandatory, structuring* knowledge layer between the repository and the agents, via MCP (data) + Skills (method) + Agents (roles) + deterministic Workflows (sequence).

**Non-goal of v1.1:** making agents follow the protocol perfectly. The goal is measurable adherence, and failure to follow the protocol is recorded, not hidden (§9).

---

# 2. Positioning

> okf-rs = an architecture and context layer for coding agents.
> LSP provides semantic understanding of symbols; OKF provides semantic understanding of architecture.

The central differentiator: **the default OKF graph is deterministic, Git-native, human-readable Markdown, generated without an LLM.** It can be versioned, reviewed, and diffed like code. The LLM never guesses the architecture — it queries OKF, then reasons over the result.

The determinism guarantee holds for `okf-rs generate` without `--enrich`. `generate --enrich` writes LLM-produced descriptions into the bundle; those are outside the guarantee and are not used by `plan-change`.

**Two change-analysis operations, two questions** `[DECIDED]`:

| Operation | Input | Question | Used before or after a change |
|---|---|---|---|
| `plan-change` `[PLANNED]` | one concept id, current bundle | "If I change this concept, what is at risk?" | before (planning) |
| `impact` / `review` (existing, CLI only) | two git refs | "What did this change touch, and how far does it reach?" | after (review, validation) |

`plan-change` does not replace `impact`. It answers the same question before the code exists, from the same graph primitives.

---

# 3. Target architecture — 4 layers

```
┌─────────────────────────────┐
│       Human developer       │
└──────────────┬──────────────┘
               │  task / intent
┌──────────────▼──────────────┐
│   Coding agent (Claude Code │
│   / OpenCode)               │
└──────────────┬──────────────┘
               │
┌──────────────▼──────────────┐
│  L2 — Skills (method)       │  WHEN and HOW to use OKF
│  explore / impact / refactor│  → SKILL.md, loaded on demand
├─────────────────────────────┤
│  L3 — Agents (role)         │  WHO does what
│  Architect / Dev / Reviewer │  → permissions, model, tools
├─────────────────────────────┤
│  L4 — Workflows (sequence)  │  WHAT happens in which order
│  explore→impact→plan→build… │  → deterministic OKF-DP protocol
└──────────────┬──────────────┘
               │  MCP queries
┌──────────────▼──────────────┐
│  L1 — okf-mcp (truth)       │  WHAT IS TRUE
│  explore / graph / search / │  → structured data
│  coverage                   │
└──────────────┬──────────────┘
               │
┌──────────────▼──────────────┐
│   OKF bundle (versioned)    │
│   concepts / relations /    │
│   callers / callees /       │
│   modules / blast radius    │
└──────────────┬──────────────┘
               │
┌──────────────▼──────────────┐
│        Source code          │
└─────────────────────────────┘
```

**Separation of responsibilities:**

| Layer | Question | Content |
|---|---|---|
| MCP | What is true? | callers, callees, blast radius, dependencies, change plan |
| Skill | How should I investigate? | step-by-step procedure per task type |
| Agent | Who am I? | role, permissions, model, reasoning depth |
| Workflow | In which order? | deterministic explore→impact→plan→… sequence |

**Founding principle: _Graph first, source second._** The agent queries the graph before reading a single file, then reads only relevant sources.

---

# 4. L1 — okf-mcp: interface contracts

A principle already in place and to be preserved: **consolidate** rather than multiply tools. An agent with fewer tool schemas in its permanent context is an agent that reasons better. Each MCP tool's JSON Schema is sent with every turn, so a new tool costs tokens on every session.

### 4.0 Exposure decision `[DECIDED]`

`plan-change` is exposed as a **new relation of the existing `graph` tool**, not as a new MCP tool. It is also available as `okf-rs graph plan-change <id>` on the CLI.

Reason: `graph` already holds every per-topology query; adding one relation adds no schema. Rejected alternative: a dedicated `plan_change` tool (one more permanent schema for one operation).

### 4.1 New operation: `plan-change` `[PLANNED]`

#### 4.1.1 Purpose

Turn the graph primitives behind `explore` and `okf-rs impact` into one structured plan for a single concept, so an agent can reason over one object instead of composing several calls.

#### 4.1.2 Input

| Parameter | Required | Meaning |
|---|---|---|
| `id` | yes | Exact concept id, e.g. `functions/src/auth/verify_token` (ids are shown by `search`) |
| `bundle` (positional) and `--project` (CLI only) | no | Bundle directory and project root, resolved as for the other `graph` subcommands (`okf.toml`'s `output` under `--project`, else `knowledge`). The MCP form has no bundle parameter: the MCP server is started with a project root and resolves the bundle from it |

**Resolution rule `[DECIDED]`:** `id` must be an exact concept id. Free text is **not** resolved (unlike `explore`). If `id` is unknown, the operation fails with the error in §4.1.7 and lists up to 5 candidates from ranked search. Rationale: a change plan for the wrong concept is worse than no plan, so the operation never guesses.

#### 4.1.3 Output schema

JSON, schema-versioned, pretty-printed with two-space indentation, keys in the order shown below. The CLI prints it with `--format json` (default) or `--format text`; MCP returns JSON. The text format's layout is defined by the implementation and is not part of the golden-file contract.

```json
{
  "schema": "okf-plan-change/1",
  "target": {
    "id": "functions/src/auth/authenticate",
    "kind": "Function",
    "module": "modules/src/auth",
    "is_public_api": true,
    "in_cycle": false
  },
  "direct_callers": ["functions/src/auth/login"],
  "indirect_callers": ["functions/src/auth/login_accepts_a_known_user", "functions/src/auth/test_login", "functions/src/oauth/handle_callback", "functions/tests/login/login_flow"],
  "affected_concepts": ["functions/src/auth/login", "functions/src/auth/login_accepts_a_known_user", "functions/src/auth/test_login", "functions/src/oauth/handle_callback", "functions/tests/login/login_flow"],
  "callees": ["functions/src/auth/verify_password"],
  "public_api": ["functions/src/auth/authenticate", "functions/src/auth/login", "functions/src/oauth/handle_callback"],
  "cycles": [],
  "modules": ["modules/src/auth", "modules/src/oauth", "modules/tests/login"],
  "tests": ["functions/src/auth/login_accepts_a_known_user", "functions/src/auth/test_login", "functions/tests/login/login_flow"],
  "risk_signals": [
    {"signal": "public_api", "detail": "target is public API"},
    {"signal": "module_boundary", "detail": "affects 2 module(s) besides the target's own"}
  ],
  "recommended_inspection": [
    {"ref": "functions/src/auth/login", "reason": "direct caller that is itself public API"},
    {"ref": "functions/src/oauth/handle_callback", "reason": "indirect caller in another module"}
  ],
  "limits": [
    "tests: heuristic (rules: path_segment, symbol_pattern, tag)",
    "tests reached only through mocks, dynamic dispatch, or separate binaries are not Calls edges and are not listed",
    "test functions declared inline in source files are recognized only when the extractor tags them (Rust: #[test]-style attributes and #[cfg(test)] modules) or their name or file matches a rule"
  ]
}
```

This is the real output for the e2e fixture (golden file `tests/fixtures/plan-change/authenticate.json`, written by `write_plan_change_fixture` in `crates/okf-cli/tests/e2e.rs`), with arrays compacted here for reading; the CLI and MCP print serde_json's pretty form, one array element per line. In the fixture, `login` (public) is the only direct caller of `authenticate`; `test_login` (`symbol_pattern`), the `#[test]` function `login_accepts_a_known_user` inside `#[cfg(test)] mod tests` (`tag`), `handle_callback` (module `src/oauth`), and `tests/login.rs`'s `login_flow` (`path_segment`) call `login`. `login` is listed once in `recommended_inspection`, under the first rule that selects it (§4.1.6 rule 1); `login_flow` is in another module but is a test, so rule 3 skips it. Modules are per source file in okf-rs, so each file is its own module.

#### 4.1.4 Field definitions (deterministic)

**Ordering:** every list is sorted by concept id, byte-wise, and deduplicated. The implementation sorts explicitly: `Graph::callers` and `Graph::callees` return relationship order, not sorted order; only `transitive_callers` is already sorted, and the implementation must not rely on that.

**No model input:** no field is inferred by a model.

| Field | Definition | Source |
|---|---|---|
| `target` | Resolved concept: `id`, `kind` (concept kind, e.g. `Function`, `Method`), `module` (owning module id, or `null` if none), `is_public_api`, `in_cycle` (true when the target belongs to a cycle per the definition below) | `Graph::get`, `Graph::owning_module`, `okf_graph::is_public_api`, `Graph::cycles` |
| `direct_callers` | Concepts with a `Calls` edge to the target, excluding the target itself (a self-call is reported by `target.in_cycle`) | `Graph::callers`, sorted |
| `indirect_callers` | `affected_concepts` minus `direct_callers` | derived |
| `affected_concepts` | Full blast radius: every concept that transitively calls the target, excluding the target itself | `Graph::transitive_callers(id, None)`, sorted |
| `callees` | Concepts the target calls, excluding the target itself | `Graph::callees`, sorted |
| `public_api` | Public-API members among `{target} ∪ affected_concepts` | `okf_graph::is_public_api` |
| `cycles` | The other members of the target's cycle, excluding the target. Empty when the target is in no cycle **or** is a self-recursive concept with no other members (see the cycle definition below) | `Graph::cycles()` |
| `modules` | Owning module id of every concept in `{target} ∪ affected_concepts`, excluding concepts with no owning module, deduplicated, sorted | `Graph::owning_module` |
| `tests` | Test concepts in `affected_concepts` (§4.1.5). The target itself is never listed | heuristic |
| `risk_signals` | Rules in §4.1.6, in fixed rule order | derived |
| `recommended_inspection` | At most 5 entries, built per §4.1.6 | derived |

**Cycle definition `[DECIDED]`.** `Graph::cycles()` returns every strongly-connected component that has more than one member, plus every single concept with a self-loop (a self-recursive call). The target is **in a cycle** (`target.in_cycle = true`, and the `cycle_member` rule fires) when the target belongs to one of those components. `cycles` then lists the other members of that component, sorted. A self-recursive target with no other member therefore has `in_cycle = true` and `cycles = []`. This matches `explore`'s "In a call cycle" line, which tests the same membership.

#### 4.1.5 Test detection (heuristic) `[DECIDED]`

The model does not record which concepts are tests. A concept is a **test concept** if it is not structural (§Appendix C) and any of these rules matches. Rule names are the values reported in `limits`.

- **`path_segment`** — any directory component of `location.file`, compared case-insensitively, equals `tests`, `test`, or `__tests__`.
- **`name_pattern`** — the file basename (case-sensitive) matches one of: `test_*.*`, `*_test.*`, `*.test.*`, `*.spec.*`, or its stem ends in `Test` or `Tests` (camel-case, so `FooTest.java` and `FooTests.cs` match and `latest.rs` does not).
- **`symbol_pattern`** — the concept's `name` (case-sensitive) starts with `test_`, or starts with `Test` followed by an uppercase letter (so Go's `TestX` matches and `Tester` does not). Applies to `Function`, `Method`, `Class`, and `Struct` concepts only.
- **`tag`** — the concept's `tags` contain `test`. The Rust extractor (`okf_tree_sitter`) sets this tag on every function or method that carries a test attribute — `#[test]`, any attribute path ending in `::test` (`#[tokio::test]`, `#[async_std::test]`, ...), `#[rstest]`, or `#[test_case(...)]` — or that is declared, at any depth, inside a module marked exactly `#[cfg(test)]` (test helpers included). Not recognized: compound conditions such as `#[cfg(all(test, ...))]`, and a test module kept in its own file (`#[cfg(test)] mod tests;`), whose attribute isn't visible from that file. Other extractors don't set the tag yet.

The target is never a test concept for this purpose. `limits` always holds three fixed entries, in this order:

1. `tests: heuristic (rules: <names>)`, listing in rule order the rules that matched at least one entry of `tests`; or `tests: heuristic (no affected concept matched a test rule)` when `tests` is empty;
2. `tests reached only through mocks, dynamic dispatch, or separate binaries are not Calls edges and are not listed`;
3. `test functions declared inline in source files are recognized only when the extractor tags them (Rust: #[test]-style attributes and #[cfg(test)] modules) or their name or file matches a rule`.

**Measured (2026-11-17).** Before the `tag` rule, Rust unit tests inside `#[cfg(test)] mod tests` blocks appeared in the bundle as ordinary private functions with no marker, so on this repository `tests` was empty for library code, `no_test_reaches_target` fired as a false alarm, and unrecognized tests filled `recommended_inspection`. With the `tag` rule, `plan-change` on `functions/crates/okf-graph/src/Graph/transitive_callers` lists 36 tests among its 51 affected concepts; the other 15 are all production code. A bundle generated before this change keeps untagged extractions in `.okf-cache.json` until the okf-rs version changes (the cache is invalidated per version); regenerate with `--no-cache` to pick the tags up sooner.

#### 4.1.6 Risk signals and inspection rules

**Thresholds and terms** come from `okf.toml` (§P1.2). Defaults: `large_blast_radius = 10`, `boundary_terms = []`. The keys are read by name; an unknown key in `[plan_change]` is ignored silently, as the current config loader does, so a misspelled key falls back to the default without an error.

**Risk signals** are evaluated in this fixed order. Each fires at most once.

| # | Signal | Fires when |
|---|---|---|
| 1 | `public_api` | target `is_public_api` |
| 2 | `cycle_member` | `target.in_cycle` is true |
| 3 | `large_blast_radius` | `affected_concepts` count ≥ `large_blast_radius` (default 10) |
| 4 | `multiple_direct_callers` | `direct_callers` count ≥ 2 |
| 5 | `module_boundary` | `modules` contains at least one module id different from `target.module`. When `target.module` is `null`, any listed module fires the rule |
| 6 | `auth_boundary` | some module id in `modules` has a path segment equal to a term in `boundary_terms`, compared case-insensitively. Only the segments after the leading `modules/` kind segment are compared, and whole segments, not substrings: the term `modules` never matches, and `aut` does not match `auth`. Never fires when `boundary_terms` is empty |
| 7 | `no_test_reaches_target` | `affected_concepts` is non-empty and `tests` is empty |

The `detail` string for each signal is generated from the counts above, using the fixed templates in the implementation; it never contains free text.

**`recommended_inspection`** is built in this order. A concept listed by an earlier rule is not repeated. Output is capped at 5 entries. Rules 3 and 4 skip concepts listed in `tests`: tests have their own field, and on a well-tested codebase they would otherwise fill every slot and push out production callers. Rule 2 still applies to a test, since a test that is the only direct caller is the only usage there is to read.

| Rule | Candidates | Reason string (fixed) |
|---|---|---|
| 1 | every `direct_callers` entry that is itself public API | `direct caller that is itself public API` |
| 2 | the single `direct_callers` entry, when there is exactly one | `sole direct caller` |
| 3 | every `indirect_callers` entry whose module differs from `target.module` | `indirect caller in another module` |
| 4 | remaining `direct_callers` | `direct caller` |

#### 4.1.7 Errors and exit behavior

| Condition | Behavior |
|---|---|
| `id` not found | Exit 1. Message names the id and lists up to 5 ranked candidates. MCP returns an error result with the same text |
| `id` is a structural concept (`Module`, `Package`, `Document`) | Exit 1. Structural concepts have no call edges; the error says so |
| bundle missing | Exit 1 with `no bundle found at <path> — run \`okf-rs generate\` first`, the message every other bundle-reading query already returns (`okf_query::require_bundle`) |
| bundle stale | Not checked by `plan-change` itself. The protocol requires `okf-rs generate --check-fresh` **before** the first edit (OKF-DP-001 precondition, §7) |

#### 4.1.8 Relationship to `explore`

`explore` answers "what is this" (signature, description, neighbours). `plan-change` answers "what does changing it touch". An agent that needs both calls `explore` first (one call) and `plan-change` second. The two share the same graph calls; `plan-change` does not repeat `explore`'s description or signature.

**`explore` output (changed 2026-11-17).** `explore` returns the concept's source location (`Location: file#Lstart-Lend`, the bundle's `resource` format) and the location of each direct caller and callee, so an agent reads exactly those lines instead of searching the file. Its lists are capped: at most 20 direct callers, 20 callees, and 10 blast-radius concepts, ordered production code first and then tests (§4.1.5 detection), each group by id. Counts stay exact, the blast-radius header says how many are tests, and a capped list ends with `+N more` naming the `graph` relation that lists everything (`callers`, `callees`, `plan-change`). Reason: in the Jujutsu benchmark (`docs/benchmarks/agent-integration-p1-jj.md`) the agent grepped for every concept `explore` had just named, and blast-radius lists of 50+ ids made up most of `explore`'s output.

#### 4.1.9 Acceptance tests `[PLANNED]`

- Unit tests in `okf-query` on a fixture graph: each field, each risk rule, ordering, the 5-entry cap.
- Fixture with a 2-concept cycle: `cycles` lists the other member; `cycle_member` fires.
- Fixture with a self-recursive concept: `in_cycle` true, `cycles` empty, `cycle_member` fires.
- Fixture with test files matching each §4.1.5 rule; a fixture without tests fires `no_test_reaches_target`.
- Unknown id and structural id produce the errors in §4.1.7.
- e2e test in `crates/okf-cli/tests/e2e.rs`: `okf-rs generate` on a fixture, then `okf-rs graph plan-change <id> --format json` matches a golden file byte for byte.
- Determinism: two runs produce byte-identical output.

### 4.2 MCP golden rule

The agent should never need to know the bundle's internal structure. Everything goes through okf-mcp. The bundle remains a versioned artifact, not an interface.

### 4.3 Skill-facing MCP naming `[DECIDED]`

Skills and agent prompts refer to MCP operations by their MCP form:

- `explore` → MCP tool `explore`
- `plan-change` → MCP tool `graph` with `relation: "plan-change"`
- Claude Code names MCP tools `mcp__<server>__<tool>` in permission rules (https://code.claude.com/docs/en/permissions). With the server named `okf`, `explore` is `mcp__okf__explore`. Use of that name in skill prose is *Unverified* (§6.5).

Skills describe the operation by its name and parameters, and show the MCP form once. The CLI form (`okf-rs graph …`) is used only in the agent's permitted bash commands, not in skill bodies. This removes the `okf graph relation=module` / `okf-rs graph modules` mixing from v1.

---

# 5. L2 — OKF Agent Skills Pack

## 5.1 Mechanics

- **Claude Code:** a Skill is a folder containing `SKILL.md` (+ optional scripts), auto-discovered from `~/.claude/skills/` (user) or `.claude/skills/` (project). The Skill body is only loaded into context when the task matches its description, so a long Skill costs little until it is used. Frontmatter fields are all optional; `name` defaults to the directory name. `allowed-tools` does **not** restrict a Skill: it pre-approves the listed tools for the turn that invokes it. To remove tools while a Skill is active, use `disallowed-tools`.
- **OpenCode:** agents are Markdown files under `.opencode/agents/` (project) or `~/.config/opencode/agents/` (global), or JSONC under the `agents` key of any OpenCode config file, including `opencode.json`. Frontmatter fields (current schema): `description`, `mode` (`primary|subagent|all`), `model`, `system`, `permissions`, `steps`. The v1 fields `permission`, `maxSteps`, `temperature`, and `prompt` belong to the legacy schema and are no longer documented; sampling is set on the model, not the agent. OpenCode also reads `SKILL.md` files from `.opencode/skills/`, `.claude/skills/` and `.agents/skills/`, so one Skill folder can serve both tools.
- **Portability:** the claim that SKILL.md is an open standard (agentskills.io) is *Unverified* in v1.1 and must be checked before it is cited.

> **Verification status (2026-11-17):** checked against https://code.claude.com/docs/en/skills, https://code.claude.com/docs/en/sub-agents, https://code.claude.com/docs/en/permissions, https://opencode.ai/docs/agents/ and https://opencode.ai/docs/skills/. The Claude Code pages were read only to about 100K characters. Points not confirmed are marked *Unverified* in place.

## 5.2 SKILL.md authoring rules

1. Description: outcome-oriented, imperative, containing trigger keywords.
2. Body < 1,500 words, explicit numbered steps, no prose.
3. One Skill = one exact workflow. Two goals → two Skills.
4. Never project knowledge: only the *method for exploiting the graph*.
5. Reference files by relative path rather than inlining schemas.
6. Name operations as in §4.3. Never give a shell command that re-runs analysis (`generate`) unless the step is explicitly "regenerate".
7. Every step that produces a claim names the MCP call that justifies it.

## 5.3 Complete example — `okf-explore` (foundational Skill, build first)

The shipped version is `examples/agent-workflow/.claude/skills/okf-explore/SKILL.md` (pack 1.3, which adds a step for structural questions, D-9); the example below is the 1.2 form.

```markdown
---
name: okf-explore
description: >
  Explore an unfamiliar or existing part of the codebase through the OKF
  knowledge graph before reading any source file. Use when the user asks
  to explain, understand, locate, or navigate code, symbols, modules,
  or architecture.
metadata:
  okf_pack: "1.2"
---

# OKF Exploration — graph first, source second

## Method

1. Identify candidate concepts from the user request (names, domain terms).
2. MCP `search` with those terms — locate concepts. If results are ambiguous
   or empty, MCP `search_ranked`.
3. MCP `explore` on the chosen concept — location, signature, description,
   callers and callees (each with its location), blast radius, public API
   membership, cycle membership. ONE call.
4. If the question is architecture-level, MCP `graph` with relation `modules`,
   `communities`, or `path` (from, to).
5. NOW read source: only the line ranges the `Location` values point to.
   Do not search for a function the graph has already located.
6. Synthesize the mental model. Never grep the whole repo to "find"
   something the graph already knows.

## Anti-patterns (forbidden)

- Reading files before the first `explore` call.
- Guessing callers/callees from source reading.
- Calling `explore` on more than 2–3 concepts before synthesizing.
```

## 5.4 The 9 Skills of the pack (prioritized)

| # | Skill | Trigger | Workflow core |
|---|---|---|---|
| 1 | `okf-explore` | understand / locate | search → explore → targeted reading |
| 2 | `okf-impact` | "I want to change X" | explore → plan-change → plan |
| 3 | `okf-refactor` | architectural refactor | modules → communities → cycles → path → plan → implement → regen → graph diff |
| 4 | `okf-review` | PR / diff | git diff → review → graph diff → blast radius → AI review |
| 5 | `okf-debug` | bug on existing symbol | explore → paths → cycles → targeted sources → hypotheses |
| 6 | `okf-feature` | new feature | search → extension points → impact → plan → build → regen |
| 7 | `okf-architecture` | docs / vision | modules → communities → cycles → mapping |
| 8 | `okf-graph-validation` | after any modification | validate → graph diff → affected tests (regen before) |
| 9 | `okf-documentation` | API/concept docs | explore → signatures → write |

**v1 scope decision: ship ONLY skills 1, 2, 3** + `okf-graph-validation` (required for the loop). The others wait for proven need (deterministic first principle).

**Phasing tags:** skills 1–3 + 8 → v1/P1–P2 · skills 4–5 → P3 · skills 6–7, 9 → P4 (deferred until value is demonstrated).

## 5.5 Skill distribution `[DECIDED]`

- v1.1 ships the pack as **examples** in `examples/agent-workflow/` (Appendix A). Users copy it into their project.
- `okf-rs init` does **not** install skills or agent files in v1.1. Reason: it would write files outside the managed blocks of `CLAUDE.md`/`AGENTS.md`, and the pack's content is still changing. Revisit after P2 (§13).
- The pack is versioned with okf-rs. Each SKILL.md carries `metadata.okf_pack` (e.g. `"1.1"`), so a copied skill can be matched to the okf-rs version it was written for.

---

# 6. L3 — Specialized agents

## 6.1 Principle

Skills carry knowledge and procedure; Agents carry role: permissions, model, accessible tools, reasoning depth. **Do not create one agent per use case** — start minimal.

## 6.2 v1.1 matrix — 3 agents

| Agent | Mode | Permissions | Role |
|---|---|---|---|
| `okf-architect` | subagent (both tools) | edit: deny; shell: deny except read-only `okf-rs` subcommands and `git diff`/`git log` (§6.3) | Queries the graph, builds the architectural model, produces plans. Never touches code. |
| `okf-developer` | primary (OpenCode) | edit: allow; shell: allow for tests | Executes the approved plan; runs `okf-graph-validation` after every modification. |
| `okf-reviewer` | subagent | edit: deny; shell: read-only as the architect | Compares before/after graphs + diff; produces the review report. |

## 6.3 OpenCode configuration example — `.opencode/agents/okf-architect.md`

```markdown
---
description: >
  Architectural reasoning over the OKF knowledge graph. Invoke for
  exploration, impact analysis, refactoring plans, and architecture
  questions. Read-only — never edits code.
mode: subagent
steps: 20
permissions:
  - { action: edit, resource: "**", effect: deny }
  - { action: shell, resource: "*", effect: deny }
  - { action: shell, resource: "okf-rs search*", effect: allow }
  - { action: shell, resource: "okf-rs explore*", effect: allow }
  - { action: shell, resource: "okf-rs graph*", effect: allow }
  - { action: shell, resource: "okf-rs coverage*", effect: allow }
  - { action: shell, resource: "okf-rs validate*", effect: allow }
  - { action: shell, resource: "okf-rs impact*", effect: allow }
  - { action: shell, resource: "okf-rs review*", effect: allow }
  - { action: shell, resource: "git diff*", effect: allow }
  - { action: shell, resource: "git log*", effect: allow }
  - { action: webfetch, resource: "**", effect: deny }
---

You are the OKF Architect. Your only source of architectural truth is
the OKF MCP server — never infer callers, dependencies, or blast radius
from source reading.

Workflow:
1. Classify the request: understand | change | debug | review.
2. Apply the matching OKF skill (okf-explore, okf-impact, okf-refactor).
3. Produce a structured plan: target concepts, affected surfaces,
   risks, ordered steps.
4. Hand off to the developer agent. You do not implement.
```

The allowlist omits `okf-rs generate` (writes the bundle and `.okf-cache.json`), `okf-rs init` (writes `AGENTS.md`/`CLAUDE.md`), `okf-rs watch`, `okf-rs docs`, and `okf-rs diff`. `diff` is left out until its temporary-worktree behavior is checked against a read-only requirement; `impact` and `review` cover the same question for PRs. The architect therefore cannot run `generate --check-fresh` itself: the developer runs it before the first edit (OKF-DP-001). Shell pattern syntax is *Unverified* beyond the documented `src/**` example for `read`; confirm in P2.

Changes from v1 (schema checked 2026-11-17): `steps` instead of `maxSteps`; `permissions` as an ordered list of `{action, resource, effect}` where the last matching rule wins; `shell` instead of `bash`; `temperature` removed because sampling is set on the model. The binary name is `okf-rs`. v1's allowlist `okf-rs *` allowed `generate` and `init`, which write files; it is replaced.

## 6.4 Claude Code configuration

- MCP server: `okf-mcp` takes the project root as its first argument (default `.`); there is **no `serve` subcommand** (verified in `crates/okf-mcp/src/main.rs:55-66`). Setup: `claude mcp add okf -- okf-mcp <project-root>`. The `claude mcp add` syntax itself is *Unverified* in this spec.
- `.claude/skills/`: the Skills from the pack (`okf-explore`, `okf-impact`, `okf-refactor`, `okf-graph-validation`).
- `.claude/agents/`: Claude Code subagent files, one per agent, format in §6.5.
- `CLAUDE.md` (generated by `okf-rs init`): add the protocol directive (§7.1), when `[agents] protocol = true`.

## 6.5 Agent file formats `[VERIFIED 2026-11-17, partial]`

Field names below were checked against the documentation pages cited. Items not confirmed are listed under *Unverified*. Agent files may be written for P2 from this section; a file that relies on an unverified item must be tested in a real session before it is committed.

**Claude Code subagent** — `.claude/agents/<name>.md`, project scope (https://code.claude.com/docs/en/sub-agents)
- Required frontmatter: `name` (no `:`, max 256 chars) and `description`.
- Optional: `tools` (allowlist; all subagent-available tools if omitted), `disallowedTools` (denylist, applied first), `model` (`sonnet`, `opus`, `haiku`, `fable`, a full model ID, or `inherit`), `maxTurns`, `mcpServers`, `skills`, `permissionMode`, `isolation` (`worktree`).
- Read-only architect: `tools: Read, Grep, Glob, mcp__okf__explore, mcp__okf__search, mcp__okf__search_ranked, mcp__okf__search_semantic, mcp__okf__coverage, mcp__okf__graph`. The MCP names are listed explicitly rather than with a wildcard, because a wildcard in `tools` is *Unverified*. Bash is absent, so the subagent cannot change files through the shell. A denylist such as `disallowedTools: Write, Edit` leaves Bash available and is **not** read-only.
- MCP: subagents inherit the main conversation's MCP tools when `tools` is omitted; an explicit `tools` list must name them.

**OpenCode agent** — `.opencode/agents/<name>.md`, `~/.config/opencode/agents/<name>.md`, or the `agents` key in `opencode.json` (https://opencode.ai/docs/agents/)
- Fields: `description`, `mode` (`primary|subagent|all`, default `primary`), `model`, `system`, `permissions`, `steps`, `hidden`, `color`, `disabled`.
- `permissions` is an ordered array of `{action, resource, effect}`; `effect` is `allow|ask|deny`; the last matching rule wins. Actions include `shell`, `edit`, `webfetch`, `read`, `glob`, `grep`, `skill`.

**Unverified**
- MCP tool names inside a SKILL.md body. `mcp__<server>__<tool>` is documented for permission rules, not confirmed for prose in a skill body.
- Wildcards in Claude Code `tools` lists.
- Glob syntax of `shell` resources beyond the documented `src/**` example for `read`.
- Whether OpenCode has an `allowed-tools` equivalent for skills: not documented on the page checked.
- The Claude Code skills and subagents pages were read only to about 100K characters.

---

# 7. L4 — OKF Development Protocol (OKF-DP)

Convention versioned in the repo, referenced by CLAUDE.md / AGENTS.md:

**OKF-DP-001 — Before modifying an existing concept:**
`okf-rs generate --check-fresh` (the bundle must be current; stop and regenerate if it is not) → MCP `explore` → MCP `graph` with relation `plan-change` → plan → human approval

**OKF-DP-002 — Before an architectural refactoring:**
MCP `graph` relations `modules` → `communities` → `cycles` → `path` → affected public APIs (from `api`)

**OKF-DP-003 — After any modification:**
regenerate → `okf-rs validate` → `okf-rs diff` between the pre- and post-change refs (requires commits or refs, not the working tree) → affected tests → (if PR) `okf-rs review`

The master workflow:

```
REQUEST → CLASSIFY (understand/change/debug)
   understand      → okf-explore → synthesis
   change          → okf-impact (OKF-DP-001) → HUMAN APPROVAL
                     → okf-developer → tests → OKF-DP-003
                     → okf-review → DONE
   debug           → okf-debug → hypotheses → fix → OKF-DP-003
```

Note: `okf-review` and `okf-debug` are delivered in P3 (§9) — in v1.1 the
`change` branch ends at OKF-DP-003, and `debug` falls back to OKF-DP-003
alone.

## 7.1 Directive text `[PLANNED]`

The protocol is delivered to agents as a short directive, not as this document. The directive is a block of at most 12 lines, written into the okf-rs-generated AGENTS.md (the source of truth for the okf-rs section, see `crates/okf-generator/src/agents.rs`) so that `okf-rs init` keeps it current. Draft text:

```markdown
## OKF development protocol

Before changing an existing concept: run `okf-rs generate --check-fresh`
(regenerate if stale), then query the bundle with `explore` and `graph`
relation `plan-change`. Do not infer callers or blast radius from source.
Before a refactor, query `graph` modules, cycles, and path. After any
change: regenerate, then `okf-rs validate`. Read source files only after
the graph names them.
```

**Open decision O-3** (§15): whether the directive is always generated or opt-in via `okf.toml` (`[agents] protocol = true`). Default in this spec: opt-in, so projects that do not use the skill pack do not receive an instruction they cannot follow.

`okf-rs init` rewrites only the top-level `output` line of an existing `okf.toml` and keeps every other setting. Before P1 it overwrote the whole file, which would have dropped `[agents] protocol` (and `[diff]`) on every re-run of `init`.

## 7.2 Adherence check

Protocol adherence is measured from session transcripts, not from the agent's own report:

- **OKF-DP-001 followed** if, for a change task, a `generate --check-fresh` call and a `plan-change` call on the target both precede the first Edit/Write to a file that defines the target.
- **OKF-DP-003 followed** if a `validate` call runs after the last Edit/Write of the task, and the regeneration step runs after the last Edit/Write.
- **Source-first violation** if a Read of a source file happens before the first `explore` call of an exploration task.

These three checks are the adherence metric in §9.2.

---

# 8. Use cases

## UC-1 — Exploration (priority 1)
*"Explain the authentication architecture"* → search → explore(Authentication) → graph modules/callers → structured explanation. The original success metric — files read divided by ≥ 3 vs a grep baseline — was measured three times on 2026-11-17 and not met: for "how does X work" questions a strong agent with grep already finds the code in one or two calls, and graph round trips added turns (`docs/benchmarks/agent-integration-p1*.md`). UC-1 stays a supported use; it is no longer the P1 gate (D-9).

## UC-1S — Structural questions (priority 1, P1 gate since D-9)
*"Who calls `persist_temp_file`?"*, *"Which commands can reach `parse_object_format_mismatch`?"*, *"What's the call path from `cmd_resolve` to X?"*, *"Which functions recurse when walking the tree?"* → `graph` relations `callers`, `plan-change`, `path`, `cycles` → answer. These are the questions the graph answers directly and grep reconstructs slowly or incompletely (transitive reach, method calls across crates). Success metric: answer accuracy against independent ground truth, and cost (§9.2).

## UC-2 — Change impact analysis (priority 1, expected high value)
*"I want to modify UserService.authenticate()"* → `plan-change` → plan (direct/indirect/API/tests/risks) → approval → implementation → after change, `okf-rs impact` between the pre-change and post-change refs. Metric: §9.2, blast-radius recall.

## UC-3 — Refactoring (priority 2)
Split PaymentService → boundary derived from the graph → implementation → regen → **before/after graph as the validation artifact** (the graph becomes the refactoring success criterion, not the LLM's opinion). The before/after snapshots must be git refs, because `okf-rs diff` compares refs, not the working tree.

## UC-4 — Code review (priority 2)
PR → git diff → OKF graph diff → deterministically computed blast radius → the AI only interprets the result. A healthy separation: *deterministic computation + AI reasoning*, never *AI guesses architecture*.

## UC-5 — Debugging (priority 3)
processOrder() sometimes fails → explore → paths → cycles → targeted sources → ordered hypotheses.

## UC-6 — Architecture/documentation (priority 3)
modules → communities → cycles → living map (regenerated, therefore never stale).

---

# 9. Phasing and measurement

## 9.1 Phases with exit criteria

| Phase | Deliverable | Exit criterion |
|---|---|---|
| **P0 — Spec** | This document v1.1, reviewed | Review findings applied; open items in §15 resolved or explicitly deferred |
| **P1 — Foundations** | `plan-change` (graph relation + CLI, §4.1); `okf-explore` Skill; OKF-DP directive in the generator template (§7.1) | **Since D-9:** on 5 structural tasks (UC-1S, §9.3), treatment accuracy ≥ 0.9, **and** either accuracy ≥ baseline + 0.15, or tokens ≤ 0.67 × baseline at accuracy ≥ baseline − 0.05 (§9.2). Adherence is recorded, not gated. *(Superseded: treatment reads ≤ 1/3 the distinct source files of the baseline on 5 exploration tasks — not met in three runs.)* |
| **P2 — Impact + Refactor** | Skills `okf-impact`, `okf-refactor`, `okf-graph-validation`; agents (3) in both tool formats | On 3 real modifications (§9.3), blast-radius recall ≥ 0.9 and 0 unpredicted broken tests (§9.2 definitions); before/after graph produced for every refactoring task |
| **P3 — Review + Debug** | Skills `okf-review`, `okf-debug` | Review of a real PR: all concepts affected per graph diff are mentioned in the report |
| **P4 — Feature + Docs** | Skills `okf-feature`, `okf-architecture`, `okf-documentation` | Only if P1–P3 demonstrate value; otherwise deferred |

**Benefit/risk safeguard:** each phase gated on the previous one; any ML/AI task-classification layer is excluded from this spec (deterministic rule-based classification: request keywords → understand/change/debug class).

## 9.2 Metric definitions `[DECIDED]`

- **Files read per exploration task:** the number of distinct source-file paths the agent opened with its Read tool (or with a shell `cat` / `type`) before the task's final answer. A Grep or Glob result that lists a file does not count unless the file is then opened. Counted from the session transcript's tool-call records. Baseline: the same task run with okf-mcp and the Skill pack disabled, grep allowed.
- **Skill followed without prompting:** the Skill is invoked (or loaded automatically) before the first Read, and the task prompt does not name the Skill or the graph. Checked from the transcript.
- **Blast-radius recall:** Let `P` = {target} ∪ `affected_concepts` from the `plan-change` output taken before any edit. Let `A` = the concepts changed in the **reference** fix (the merged commit being replayed, computed as the `added`, `changed` and `removed` ids of `okf-rs impact` between the pre- and post-commit refs) ∪ the concepts directly called by any test that failed on the unmodified code. Recall = |P ∩ A| / |A|. `A` is computed from the reference, not from the agent's own edit. Tasks with |A| = 0 are excluded and reported separately.
- **Unpredicted broken tests:** tests that fail after the change and whose id is not in the `tests` field of `plan-change`. Counted per task; the P2 target is 0.
- **Protocol adherence rate:** fraction of tasks where all applicable checks in §7.2 passed.
- **Tokens consumed:** total input + output tokens per task, from the session log.
- **Characters read per tool:** characters returned to the agent by `Read`, `Grep`, and the `okf` MCP tools, from the transcript. Added after the first run showed that Grep's content output and MCP responses are reading too, which "files read" misses.
- **Structural accuracy (UC-1S, since D-9):** per task, against ground truth fixed before the run. Set answers (callers, commands, cycle members): recall = required items named ÷ required items, precision = correct items ÷ items claimed, score = F1; test functions are optional (count neither way). Path answers: 1 if every required hop appears in order with no wrong hop, else 0. Accuracy = mean task score per variant. The exact scoring of each task set is pre-registered in its results document.
- **Repeated runs (D-10):** when a gate is measured over several runs, accuracy is the mean over every task-run and the token ratio is the ratio of totals over the same task-runs; the number of runs and the rule are written down before the first of them, and no run is excluded.

## 9.3 Task selection `[DECIDED]`

Benchmark tasks are taken from **this repository's own history**, so ground truth exists:

- 5 exploration tasks: questions whose answers are in merged commits (e.g. "how does the bundle cache invalidate?", "where is the LSP fallback decided?").
- 3 modification tasks: replays of merged feature or fix commits, reverted in a scratch worktree, given to the agent with the original issue text, and graded against the reference commit's diff and the tests it added or changed.
- If fewer than 3 modification tasks have |A| > 0, the task pool is extended with further merged commits before the P2 gate is evaluated. The gate is never passed on fewer than 3 valid tasks.

Tasks are fixed before the first run and recorded in `examples/agent-workflow/README.md` (A/B scenarios, Appendix A).

**Structural tasks (UC-1S, since D-9).** Run on a repository large enough that grep navigation is costly (Jujutsu, 456 Rust files, for the first set). Ground truth must not come from okf-rs output, or the treatment wins by construction: call sites are found by grep, mapped to enclosing functions by a text-only helper, and every result is reviewed by reading the code; transitive answers are built level by level the same way. Building or running the target (e.g. a compiler-based oracle) is avoided for third-party repositories, since it executes their build scripts. Tasks, ground truth, and scoring are committed before the first run (`docs/benchmarks/agent-integration-p1-structural.md`). The set mixes a grep-easy control, a case the graph is known to get wrong (method calls on a widely shared name), transitive reach, a call path, and a cycle.

---

# 10. Expected benefits (to be measured)

The benefits below are hypotheses. Each is tied to a metric in §9 and is claimed as fact only after measurement.

## On an existing (brownfield) codebase

- **Faster onboarding.** New contributors (human or agent) can ask the graph "who calls X, what does it affect". Measured by files read and time to a correct explanation (UC-1).
- **Change impact before the edit.** Each modification starts from a computed blast radius (§9.2 recall). Refactorings and dependency upgrades become planned operations with a known affected surface, including the tests to run.
- **Architecture drift is visible.** The graph is regenerated and versioned like code: dependency cycles and new coupling appear in `okf-rs diff` and can be reviewed in a PR.
- **Reviews grounded in computed facts.** PR review diffs the graph: public API changes, call-graph changes, and cycle introductions are listed by the tool.
- **Documentation generated from the graph.** Architecture docs are regenerated from the bundle rather than written by hand.
- **Lower agent cost.** *Not supported in general.* "How does X work" questions: no reduction (three runs). Structural questions: answers were equally accurate with and without the graph on both pre-registered sets; tokens fell to 0.53× on Jujutsu (cross-crate questions, where grep is expensive) and rose to 1.59× on the held-out Helix set (local answers, where the graph route's fixed overhead exceeds grep's cost). The supported claim is narrower: the graph saves cost on questions that are expensive to answer by search.

## On a codebase in active build (greenfield)

- **Design feedback while you code.** Module boundaries, communities, and cycles are visible from the first commits.
- **A history of structure.** Each regeneration snapshots the architecture; the Git history of the bundle records structural changes.
- **The same loop from day one.** Every change goes through explore → impact → plan → build → graph-diff → review.

## In one sentence

> OKF gives humans and agents a versioned, queryable map of the codebase's architecture; whether that reduces cost and errors is what §9 measures.

---

# 11. Benefits / risks

| Benefit | Risk | Mitigation |
|---|---|---|
| The agent no longer guesses the architecture | Stale graph → wrong answers with authority | OKF-DP-001 precondition `generate --check-fresh` before any edit; `validate` in CI |
| Reduced context cost (explore bundles several facts) | MCP over-calling (latency, tokens) | Skills cap at 2–3 concepts before synthesis |
| Versioned, reviewable, diffable graph | Heavy OKF bundle on large repos | `coverage` to measure; dead-concept pruning |
| SKILL.md portability | Claude Code / OpenCode divergence | Test the Skills on both tools in P1 |
| Reproducible plan→build→graph-diff loop | Over-rigid workflow → agent works around it | OKF-DP written as directives, not blockers; measure adherence (§7.2) in P1 |
| `plan-change` predicts blast radius from the graph | Test heuristic misses tests reached through mocks or dynamic dispatch | `limits` field lists the rules and blind spots; §9.2 recall exposes the gap |
| `plan-change` gives a confident plan for the wrong concept | Agent acts on a plan for a near-miss id | Exact-id resolution only (§4.1.2) |
| Protocol directive adds instructions to projects that don't use the pack | Noise, ignored instructions | Opt-in default (O-3) |
| Architect agent can still write files through a permitted command | Unexpected writes to the bundle or agent files | Shell allowlist limited to read-only subcommands (§6.3) |

---

# 12. Medium-term vision

> Claude Code / OpenCode become *graph-aware* through OKF — with two implicit representations: the source (how it is implemented) and the graph (how the system is connected).

Next concrete step: an **OKF Agent Workflow** shipped inside okf-rs as an official example — SKILL.md structure, the 3 agent prompts, AGENTS.md/CLAUDE.md conventions, and the reproducible explore → impact → plan → build → graph-diff → review workflow.

---

# 13. Decisions log

Defaults chosen in v1.1. Each can be changed; the reason to revisit is listed.

| ID | Decision | Default | Revisit when |
|---|---|---|---|
| D-1 | `plan-change` exposure | `graph` relation + CLI subcommand, no new MCP tool (§4.0) | Schema token cost measured to be negligible, or the relation becomes unwieldy |
| D-2 | `plan-change` id resolution | exact id only (§4.1.2) | Users report friction; then add an explicit `--search` mode, never silent fallback |
| D-3 | Test detection | path/name heuristic plus the extractor's `test` tag (Rust), rules in §4.1.5 | Blast-radius recall (§9.2) < 0.9 on P2 tasks; next step would be tagging in more extractors |
| D-4 | Skill distribution | `examples/` only, not installed by `init` (§5.5) | After P2 shows a stable pack |
| D-5 | Skill MCP naming | MCP form, CLI never in skill bodies (§4.3) | — |
| D-6 | Protocol directive | opt-in (O-3, §15) | Adherence rate (§7.2) shows agents ignore it even when present |
| D-7 | Cycle membership | SCC with more than one member, or a self-loop (§4.1.4) | — (matches `explore` today) |
| D-8 | Architect shell access | read-only `okf-rs` subcommands and `git diff`/`git log` only (§6.3) | Read-only requirement for `diff` is confirmed, then add it |
| D-11 | P1 outcome (decided 2026-11-17 by the project owner) | Accept the narrower claim: the graph lowers agent cost on structural questions that are expensive to answer by search (cross-crate transitive reach, widely shared method names) and raises it on local lookups; accuracy was equal with and without it on both pre-registered sets. P1 is closed on that basis; P2 is decided on its own merits. The skill is not re-tuned to route by question type (that would need a new held-out test) | A future held-out set shows routing would change the result |
| D-10 | Repeated measurement (decided 2026-11-17 by the project owner) | P1.8 re-measured as 3 runs after receiver-type resolution; mean accuracy and ratio-of-totals tokens, rule pre-registered in `docs/benchmarks/agent-integration-p1-structural.md` | — |
| D-9 | P1 gate (decided 2026-11-17 by the project owner) | Structural questions (UC-1S): accuracy against independent ground truth, then accuracy gain or token savings (§9.1). Replaces "≤ 1/3 files read" on "how does X work" questions | Evidence for the change: okf-rs run (files 0.46; floor 0.38 on that repo), Jujutsu run 1 (0.87, adherence 4/5), Jujutsu run 2 after the `explore` fix (1.23, tokens 2.3×). Revisit if structural answers turn out no better than grep's |

---

# 14. Changes from v1

- Header: date, version, supersedes line; status reflects the review.
- §1 and §3: MCP tool list corrected; `review`, `impact`, `diff` marked CLI-only (v1 said MCP provided them).
- §2: determinism claim qualified for `generate --enrich`; `impact`/`review` marked CLI only.
- §4.1: `plan-change` fully specified (was a single JSON sketch): input (`bundle` removed from the MCP form), output with key order, deterministic ordering rule, cycle definition, test heuristic rules, risk rule thresholds, boundary matching, recommended-inspection reason strings, errors, and acceptance tests. The example is made consistent with those rules.
- §4.3: MCP naming rule added; the v1 mix of `okf graph relation=…` and `okf-rs graph …` is removed.
- §5.1: `allowed-tools` description corrected: it grants, it does not restrict. agentskills.io claim marked unverified.
- §5.3: `metadata.okf_pack` added to the example, as §5.5 requires; `[PLANNED]` tags removed from `communities` and `path` (both exist).
- §5.4: `okf-graph-validation` no longer lists regeneration as its first step (regeneration belongs to the protocol).
- §6.2–§6.3: architect shell allowlist replaced; `generate`, `init`, `diff` removed; `build` mode removed (not an OpenCode mode).
- §6.4: `okf-mcp serve` removed; the verified invocation is `okf-mcp <project-root>`.
- §6.5: Claude Code architect tool list names the MCP tools explicitly; wildcard usage marked unverified.
- §7: OKF-DP-001 now runs `generate --check-fresh` before the first edit (v1 ran it after, which fails on any stale bundle); OKF-DP-001 uses `explore` + `plan-change` rather than seven separate calls; OKF-DP-003 uses regeneration after the change.
- §7.2: adherence checks follow the corrected order.
- §9.2: recall definition now uses the reference commit's diff as ground truth; "unpredicted broken tests" defined; "files read" and "skill followed" operationalized.
- §9.3: minimum valid task count; §9.1 P2 exit criterion refers to the defined metrics.
- §10: benefits recast as hypotheses tied to §9 metrics.
- §11: one risk added (architect writes through permitted commands).
- §13: D-7 and D-8 added.
- §15: O-2 closed; O-1 narrowed.
- Corrections from the P1 implementation (2026-11-17): §4.1.2 CLI parameters match the other `graph` subcommands (positional `bundle`, `--project`); §4.1.3 example replaced by the real golden output; §4.1.4 `kind` is the concept kind, and self-calls are excluded from `direct_callers`/`callees`; §4.1.5 `symbol_pattern` kinds listed exactly, `limits` strings fixed, and the measured Rust inline-test limitation recorded; §4.1.6 boundary terms skip the `modules/` kind segment; §4.1.7 missing-bundle message matches `okf_query::require_bundle`; §7.1 `init` keeps the rest of `okf.toml`; O-4 closed.
- Rust test tagging (2026-11-17, before P2): §4.1.5 adds the `tag` rule backed by the Rust extractor's `test` tag, updates the third `limits` string, and replaces the "measured limitation" note with the measured result; §4.1.6 rules 3 and 4 skip tests; §4.1.3 example regenerated; D-3 updated.
- `explore` location and caps (2026-11-17, after the Jujutsu benchmark): §4.1.8 documents the new `explore` output (concept and neighbour locations; capped, production-first lists with exact counts); §5.3 example skill reads the returned line ranges (pack 1.2).
- Receiver-type call resolution (2026-11-17, after the first structural run): the Rust extractor records what each method call's receiver is (`self`, a typed parameter or `let`, a field, a chained call, a `Type::f()` constructor) plus project-wide type facts (field types, method owners and return types, trait signatures, aliases); the analyzer resolves an ambiguous method name to the one candidate the receiver's type owns (`resolved_by: tree-sitter-receiver`), and drops name-only edges from calls on standard-library or third-party values. On Jujutsu: +10,144 edges, −443 false edges; a manual check of 27 random added and 27 random removed edges found every one correct. The structural run's two graph misses (`Store::concurrency`, `GitFetch::fetch`) now resolve, so `plan-change` reaches all four commands in S3.
- Self-call fix and D-11 (2026-11-17): the analyzer now records a function calling itself when the syntax vouches for it — `f(..)` not shadowed by a function-local `use`, `self.f(..)`, `Self::f(..)` (Rust; plus any recursion a language server confirms with `--lsp`) — so §4.1.4's self-recursion case occurs in real bundles. Delegation such as `self.inner.f(..)` is not recorded as recursion. Checked by hand: all 15 self-edges on Helix and a random 15 of 28 on Jujutsu are genuine recursion. Known limits documented: macro-bound receivers get no type; type names are compared without their crate. D-11 accepts the narrower P1 claim.
- Stale-file pruning (2026-11-17): `okf-rs generate` (and `watch`) now removes the files an earlier run wrote for concepts that no longer exist, so a renamed function no longer leaves an orphan that `validate` reports and `generate --check-fresh` flags forever. Only concept files stamped `generated.by: okf-rs/…` and okf-rs's own per-kind `index.md` are removed, and only inside the per-kind directories. This matters for P2's impact measurement, which regenerates the bundle after each change (OKF-DP-003).
- Held-out structural benchmark (2026-11-17): pre-registered on Helix, failed (tokens 1.59×, accuracy tied); §10 cost claim narrowed. Findings recorded in `docs/benchmarks/agent-integration-p1-heldout.md`, including a spec/implementation mismatch: the analyzer never records self-calls, so §4.1.4's self-recursion case never occurs in a real bundle.
- P1 re-scope (2026-11-17, D-9): §8 adds UC-1S (structural questions) and records the UC-1 result; §9.1 P1 exit criterion replaced; §9.2 adds characters read per tool and structural accuracy; §9.3 adds structural task rules (independent ground truth, pre-registration); §10 cost claim corrected; the shipped skill is pack 1.3 (structural questions), while §5.3 keeps the 1.2 example.

---

# 15. Open items

| ID | Item | Status |
|---|---|---|
| O-1 | Agent and Skill formats (§6.5) | Partly resolved 2026-11-17. *Unverified* items in §6.5 must be tested before agent files are committed |
| O-2 | `okf-mcp` invocation | Closed. Verified in code: `okf-mcp <project-root>`, no `serve`. The `claude mcp add` syntax is still *Unverified* (§6.4) |
| O-3 | Directive opt-in vs always-on (§7.1) | Default: opt-in. Confirm |
| O-4 | `okf.toml` keys `plan_change.large_blast_radius` and `plan_change.boundary_terms` | Closed — implemented in P1 (`crates/okf-core/src/config.rs`). Invalid values warn and keep the default; `large_blast_radius` must be ≥ 1 |
| O-5 | Shell pattern syntax in OpenCode `permissions` (§6.3) | Open — confirm in a P2 session |
| O-6 | Whether `okf-rs diff` writes outside the project (temporary worktree) | Open — decides whether the architect may run `diff` (D-8) |

---

# Appendix A — Target pack layout

```
okf-rs/
├── examples/
│   └── agent-workflow/
│       ├── .claude/skills/
│       │   ├── okf-explore/SKILL.md
│       │   ├── okf-impact/SKILL.md
│       │   ├── okf-refactor/SKILL.md
│       │   └── okf-graph-validation/SKILL.md
│       ├── .opencode/agents/
│       │   ├── okf-architect.md
│       │   ├── okf-developer.md
│       │   └── okf-reviewer.md
│       ├── CLAUDE.md          (OKF-DP directive, generated by okf-rs init)
│       ├── AGENTS.md
│       └── README.md          (A/B test scenarios, task list from §9.3)
```

Claude Code subagent files (`.claude/agents/`) are added in P2, using the format in §6.5.

# Appendix B — A/B validation metrics

- **Files read per exploration task** (OKF vs grep baseline) — §9.2
- **Blast-radius recall** (predicted vs actual affected concepts) — §9.2
- **Unpredicted broken tests** — §9.2
- **Protocol adherence rate** (OKF-DP checks in §7.2) — §9.2
- **Tokens consumed** per equivalent task — §9.2

# Appendix C — Glossary

- **Concept id:** stable identifier such as `functions/src/auth/verify_token`, shown by `search`.
- **Blast radius:** every concept that transitively calls a given concept (`Graph::transitive_callers`), excluding the concept itself.
- **Public API:** concept with `is_public` set and not structural (`okf_graph::is_public_api`).
- **Cycle member:** concept in a strongly-connected component with more than one member, or a self-recursive concept (self-loop), as returned by `Graph::cycles()`.
- **Structural concept:** `Module`, `Package`, or `Document`; no call edges, never public API.
- **Graph diff / impact:** comparison of two git refs' bundles (`okf-rs diff`, `okf-rs impact`).
