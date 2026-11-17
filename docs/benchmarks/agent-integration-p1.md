# Agent integration — P1 measurement

Exit criterion (spec §9.1, P1): on the 5 exploration tasks in [`examples/agent-workflow/README.md`](../../examples/agent-workflow/README.md), the agent invokes `okf-explore` without prompting on every task, and the treatment reads ≤ 1/3 of the baseline's distinct source files. Metric definitions: spec §9.2.

**Status:** run on 2026-11-17. **Verdict: fail** on the file-count criterion; adherence criteria pass. See [Interpretation](#interpretation).

## Run conditions

| Field | Value |
|---|---|
| okf-rs commit | `137d792` (exported with `git archive`, so no `.git` history in the copies) |
| Agent and version | Claude Code 2.1.326, headless (`claude -p`, `--output-format stream-json`) |
| Model | `claude-opus-5-5` (all 10 runs) |
| Date | 2026-11-17 |
| Bundle | `okf-rs generate .` on the treatment copy, no `--enrich`, no `--lsp` (1289 concepts) |
| Built-in tools | `Read`, `Grep`, `Glob`, `Skill` in both variants (`--tools`); no Bash, no edits |
| MCP | baseline: none (`--strict-mcp-config`); treatment: only `okf` |
| Settings | `--setting-sources project,local` (no user settings), `--permission-mode dontAsk`, `--no-session-persistence` |
| Prompt | the question only, verbatim from the pack README |
| Runs | 1 per task and variant (n = 1); all 10 in parallel |

Both copies had `examples/agent-workflow/.claude/` removed (so the skill can't be discovered from a nested folder in the baseline) and `examples/agent-workflow/README.md` removed (it maps each question to its answer's files). The baseline copy has no `knowledge/` bundle. Harness: [`examples/agent-workflow/bench/`](../../examples/agent-workflow/bench/).

## Results

"Files" counts distinct project files opened with `Read` (bundle files under `knowledge/` excluded; a Grep hit doesn't count). Tokens are input + cache creation + cache read + output, from the session's result record.

| Task | Variant | Files opened | Skill invoked before first read | Source-first violation | okf MCP calls | Tokens | Cost | Answer correct |
|---|---|---|---|---|---|---|---|---|
| E1 | baseline | 2 | n/a | n/a | — | 58,935 | $0.170 | yes |
| E1 | treatment | 1 | yes | no | search_ranked, explore ×2 | 111,757 | $0.179 | yes |
| E2 | baseline | 2 | n/a | n/a | — | 66,904 | $0.189 | yes |
| E2 | treatment | 1 | yes | no | search, explore | 81,239 | $0.125 | yes |
| E3 | baseline | 4 | n/a | n/a | — | 97,720 | $0.227 | yes |
| E3 | treatment | 2 | yes | no | search, search_ranked, explore ×2 | 107,898 | $0.216 | yes |
| E4 | baseline | 2 | n/a | n/a | — | 102,886 | $0.186 | yes |
| E4 | treatment | 1 | yes | no | search_ranked, explore | 105,960 | $0.147 | yes |
| E5 | baseline | 3 | n/a | n/a | — | 50,721 | $0.181 | yes |
| E5 | treatment | 1 | yes | no | search_ranked, search, explore | 105,575 | $0.147 | yes |
| **Total** | baseline | **13** | | | | **377,166** | **$0.95** | 5/5 |
| **Total** | treatment | **6** | 5/5 | 0/5 | | **512,429** | **$0.81** | 5/5 |

Files opened per run:

- E1 — baseline: `okf-mcp/src/cache.rs`, `okf-mcp/src/tools.rs`; treatment: `okf-mcp/src/cache.rs`.
- E2 — baseline: `okf-cli/src/main.rs`, `okf-generator/src/lib.rs`; treatment: `okf-cli/src/main.rs`.
- E3 — baseline: two `tests/fixtures/diff/resolver-change/*/functions/foo.md` fixtures, `okf-analyzer/src/lib.rs`, `okf-cli/src/main.rs`; treatment: `okf-analyzer/src/lib.rs`, `okf-cli/src/main.rs`.
- E4 — baseline: `okf-analyzer/src/lsp.rs`, `okf-lsp/src/lib.rs` (plus a failed read of `knowledge/index.md`, which `AGENTS.md` points to); treatment: `okf-lsp/src/lib.rs`.
- E5 — baseline: `okf-core/src/lib.rs`, `okf-analyzer/src/lib.rs`, `okf-tree-sitter/src/lib.rs`; treatment: `okf-core/src/lib.rs`.

Correctness was graded against the ground-truth commits in the pack README: every answer names the mechanism the commit introduced (E1 fingerprint of count/size/mtime; E2 fresh render to a temp directory and byte-diff ignoring `source_revision`; E3 pairing by `(kind, target)` then `classify_provenance_change`; E4 quiet period over `$/progress` tokens; E5 Windows-1254 decoding of invalid byte runs only).

### Characters read (added after the second run)

The analyzer now also counts characters returned to the agent per tool, because Grep's content mode returns source lines that "files opened" doesn't see, and MCP responses cost context too. Re-analyzing the same transcripts:

| Task | Baseline source chars (Read + Grep) | Treatment source chars | Treatment MCP chars |
|---|---|---|---|
| E1 | 22,258 | 15,846 | 6,402 |
| E2 | 27,230 | 8,991 | 1,862 |
| E3 | 33,239 | 13,568 | 16,209 |
| E4 | 22,976 | 9,518 | 6,003 |
| E5 | 29,062 | 5,024 | 6,908 |
| **Total** | **134,765** | **52,947** (0.39) | **37,384** |

Source plus MCP characters: 90,331 vs 134,765, i.e. **−33%** for the treatment on this repository. The second run, on a larger repository, went the other way: [`agent-integration-p1-jj.md`](agent-integration-p1-jj.md).

## Verdict

- Skill invoked without prompting on all 5 treatment runs: **yes** (5/5, always before the first file read)
- Source-first violations: **0/5**
- Total distinct files, treatment ÷ baseline: **6 / 13 = 0.46** (criterion ≤ 0.33)
- P1 exit criterion: **fail** (file ratio)

## Interpretation

- **Adherence works.** With only the skill installed and a plain question, the agent loaded `okf-explore` first and queried the graph before reading source on every task. That was the open question behind the spec's "agent forgets the tool" risk (§1).
- **The file criterion is unreachable on this repository.** The treatment opened exactly one file on 4 of 5 tasks; the floor is one file per task (5), and 5 / 13 = 0.38 > 0.33. The baseline is already efficient here: about 55 source files with descriptive crate and module names, so Grep lands on the right file in 2–4 reads. The spec's 3× target assumes a repository where grep-based navigation is expensive; this one isn't a fair test of it.
- **Tokens went up, cost went down.** The treatment used 36% more tokens (more turns: 47 vs 32, plus MCP responses) but cost 15% less, because more of its input was served from the prompt cache. Neither direction is a P1 criterion; both are worth tracking.
- **Limits of this run:** one sample per cell (no variance estimate), one model, one repository, tasks written by the same team that built the tool.

## Options (decision needed; not taken here)

1. Re-run the same protocol on a larger, less familiar repository (several hundred source files) where the 3× bar is meaningful. **Done 2026-11-17 on Jujutsu (456 Rust files): fail, ratio 0.87** — see [`agent-integration-p1-jj.md`](agent-integration-p1-jj.md).
2. Change the criterion for small repositories (e.g. files opened beyond the files that hold the answer, or tokens of source read), and record the change in the spec's decisions log.
3. Accept P1 on adherence alone and gate P2 on its own criteria.

Per spec §9.1, P2 does not start until one of these is chosen.
