# Agent integration — P1 measurement

Exit criterion (spec §9.1, P1): on the 5 exploration tasks in [`examples/agent-workflow/README.md`](../../examples/agent-workflow/README.md), the agent invokes `okf-explore` without prompting on every task, and the treatment reads ≤ 1/3 of the baseline's distinct source files. Metric definitions: spec §9.2.

**Status:** not run yet.

## Run conditions

| Field | Value |
|---|---|
| okf-rs commit | |
| Agent and version | |
| Model | |
| Date | |
| Bundle | `okf-rs generate .` on the same commit, no `--enrich`, no `--lsp` |

## Results

| Task | Variant | Distinct source files opened | Skill invoked before first read | Source-first violation | Tokens | Answer correct |
|---|---|---|---|---|---|---|
| E1 | baseline | | n/a | n/a | | |
| E1 | treatment | | | | | |
| E2 | baseline | | n/a | n/a | | |
| E2 | treatment | | | | | |
| E3 | baseline | | n/a | n/a | | |
| E3 | treatment | | | | | |
| E4 | baseline | | n/a | n/a | | |
| E4 | treatment | | | | | |
| E5 | baseline | | n/a | n/a | | |
| E5 | treatment | | | | | |

## Verdict

- Skill invoked without prompting on all 5 treatment runs: _yes / no_
- Total distinct files, treatment ÷ baseline: _ratio_ (pass if ≤ 0.33)
- P1 exit criterion: _pass / fail_
