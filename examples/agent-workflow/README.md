# OKF agent workflow pack

Skills and conventions that make a coding agent (Claude Code, OpenCode) query the OKF knowledge graph before it reads source. The design and its rationale are in [`docs/OKF_Agent_Integration_Spec_EN_v1.md`](../../docs/OKF_Agent_Integration_Spec_EN_v1.md); the delivery plan is [`docs/agent-integration-plan.md`](../../docs/agent-integration-plan.md).

**Pack version:** 1.3 (matches `metadata.okf_pack` in each `SKILL.md`). 1.2: `okf-explore` reads the line ranges `explore` returns instead of searching for the concept. 1.3: `okf-explore` covers structural questions (callers, transitive reach, paths, cycles) relation by relation, and says when to check the graph with grep. Benchmark runs record the pack version they used.

## What's in it

| Path | Status |
|---|---|
| `.claude/skills/okf-explore/SKILL.md` | P1 — exploration: graph first, source second |
| `okf-impact`, `okf-refactor`, `okf-graph-validation` skills | P2, not yet written |
| Agent definitions (`.claude/agents/`, `.opencode/agents/`) | P2, not yet written |

## Setup

1. **Build the binaries.** From the okf-rs checkout: `cargo build --release` gives `target/release/okf-rs` and `target/release/okf-mcp`. Put both on your `PATH`.
2. **Generate the bundle** in your project: `okf-rs generate .` (writes `knowledge/`).
3. **Register the MCP server** under the name `okf`. `okf-mcp` takes the project root as its only argument (default: the current directory); there is no `serve` subcommand. For Claude Code:

   ```sh
   claude mcp add okf -- okf-mcp /path/to/your/project
   ```

   Check this against your Claude Code version's `claude mcp add --help`; the spec lists the exact syntax as unverified (§6.4).
4. **Copy the skill** into your project: `examples/agent-workflow/.claude/skills/okf-explore/` → `<your project>/.claude/skills/okf-explore/`. OpenCode also reads `.claude/skills/`, so the same folder serves both tools.
5. **Optional: add the protocol directive.** In your project's `okf.toml`:

   ```toml
   [agents]
   protocol = true
   ```

   then run `okf-rs init .`. The okf-rs section of `AGENTS.md` gains a short "OKF development protocol" block (and `CLAUDE.md` sees it through its `@AGENTS.md` import). `init` keeps the rest of `okf.toml` as it is.

`plan-change` thresholds can be tuned in the same file:

```toml
[plan_change]
large_blast_radius = 10          # default
boundary_terms = ["auth", "payment"]   # default: none
```

## A/B scenarios (P1 exit criterion)

Spec §9.1: on 5 real exploration tasks, the agent loads and follows `okf-explore` without being told to, and reads at most a third of the distinct source files the baseline reads. Tasks come from this repository's history so the answer is checkable (spec §9.3). They are fixed here before the first run.

| # | Question given to the agent | Ground truth (commit) | Files the answer lives in |
|---|---|---|---|
| E1 | How does the MCP server avoid re-parsing the bundle on every tool call, and when does it re-read it? | `d5fce5b` | `crates/okf-mcp/src/cache.rs` |
| E2 | How does `okf-rs generate --check-fresh` decide that the bundle on disk is stale? | `bf74256` | `crates/okf-cli/src/main.rs` |
| E3 | How does `okf-rs diff --ci` tell a resolver-only relationship change from a source-level one? | `97474f6` | `crates/okf-analyzer/src/lib.rs` |
| E4 | How does okf-rs decide that a language server is ready to answer queries? | `194f887` | `crates/okf-lsp/src/lib.rs`, `crates/okf-analyzer/src/lsp.rs` |
| E5 | What happens when a source file is not valid UTF-8? | `c0986c6` | `crates/okf-core/src/lib.rs` |

### Procedure

Run every task twice, in fresh sessions, on the same commit of this repository with a freshly generated bundle:

- **Baseline:** no `okf` MCP server, no skill. The agent may use grep and file reads.
- **Treatment:** `okf` MCP server registered and `okf-explore` installed. The task prompt is the question only — it must not mention the skill, the graph, or okf.

For each run, record from the session transcript (definitions in spec §9.2):

- distinct source files opened (Read tool or shell `cat`/`type`; a grep hit doesn't count unless the file is then opened);
- whether `okf-explore` was invoked before the first file read;
- whether a source file was read before the first `explore` call (a source-first violation);
- total tokens;
- whether the answer is correct against the ground-truth commit.

Results go in [`docs/benchmarks/agent-integration-p1.md`](../../docs/benchmarks/agent-integration-p1.md) (first run: 2026-11-17). If the exit criterion fails, record the result and stop: P2 starts only after P1 passes.

### Automated harness

[`bench/`](bench/) runs this procedure with headless Claude Code (`claude -p`):

```sh
# okf-rs itself as the target:
examples/agent-workflow/bench/setup.sh . . <commit> <empty work dir>
<work dir>/run-all.sh tasks/okf-rs.tsv

# any other git repository as the target:
examples/agent-workflow/bench/setup.sh . <target repo> <commit> <empty work dir>
<work dir>/run-all.sh tasks/<name>.tsv        # e.g. tasks/jj.tsv

node <work dir>/analyze.js <work dir>         # metrics per §9.2; answers saved for grading
```

`setup.sh` builds okf-rs from this checkout, exports the target `<commit>` with `git archive` (no history for the agent to read), removes every `.claude/` folder from both copies (no nested skill in the baseline, no project settings or hooks from an untrusted repository), generates the bundle only in the treatment copy, and installs the skill there. Both variants get the same built-in tools (`Read`, `Grep`, `Glob`, `Skill`) and no user settings; only the treatment gets the `okf` MCP server. A tasks file is `<id><TAB><question>` per line, written before the first run. Grading answers against the ground-truth commits is manual.

| Tasks file | Target | Results |
|---|---|---|
| `bench/tasks/okf-rs.tsv` | this repository (55 Rust files) | [`docs/benchmarks/agent-integration-p1.md`](../../docs/benchmarks/agent-integration-p1.md) |
| `bench/tasks/jj.tsv` | [Jujutsu](https://github.com/jj-vcs/jj) at `4351061bb` (456 Rust files), "how does X work" | [`docs/benchmarks/agent-integration-p1-jj.md`](../../docs/benchmarks/agent-integration-p1-jj.md) |
| `bench/tasks/jj-structural.tsv` | Jujutsu at `4351061bb`, structural questions (P1 gate after the re-scope) | [`docs/benchmarks/agent-integration-p1-structural.md`](../../docs/benchmarks/agent-integration-p1-structural.md) |
