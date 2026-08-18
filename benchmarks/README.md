# Benchmarks

A discoverable index of every reproducible benchmark/measurement this project ships, and the real
results already collected. This directory holds **documentation and recorded results only** — the
actual benchmark code lives inside the crates it benchmarks (`crates/okf-mcp/src/benchmark.rs`,
`crates/okf-mcp/src/tool_selection_{benchmark,live}.rs`, `crates/okf-analyzer`'s
`resolver_only_rate`, `crates/okf-cli`'s `diff-bundles`), fully covered by that crate's own test
suite. Nothing here is a separate script or a fork of that logic — every command below is a real,
already-tested `okf-rs`/`okf-mcp` CLI invocation, not a one-off tool that could drift from what's
actually shipped.

This layout follows a structure proposed in external review
([`docs/feedback/2026-08-tool-consolidation-benchmark-review.md`](../docs/feedback/2026-08-tool-consolidation-benchmark-review.md)),
adapted to match what's actually built rather than mirrored literally: the review's suggested
`specialized-vs-explore`/`silent-wrong` as two separate nodes are one benchmark here (the live
tool-selection runner reports both facets — selection accuracy *and* the loud/detectable/silent
failure-mode breakdown — from the same run), and `provenance-diff` doesn't get its own directory
separate from `resolver-stability`, since `okf-rs diff-bundles` *is* the provenance-diff classifier
applied to the resolver-version-comparison use case, not a second tool.

## Suites

| Directory | What it measures | Live model/resolver needed? |
|---|---|---|
| [`mcp-tool-selection/`](mcp-tool-selection/) | Whether a model picks the right `relation` inside the consolidated `graph` tool as reliably as it picked the right specialized tool name pre-consolidation — selection accuracy, final-answer accuracy, and the loud-failure/detectable-wrong/silent-wrong cost breakdown | Yes — a real OpenAI-compatible endpoint |
| [`mcp-session-cost/`](mcp-session-cost/) | The fixed per-session token cost of registering `okf-mcp`'s tool schemas against the tokens saved per structural query, and the resulting break-even point | No — fully offline |
| [`resolver-stability/`](resolver-stability/) | How much of a diff's relationship-level churn between two resolver versions (or any two independently generated bundles) is resolver-only metadata vs. a genuine structural rewire | Only to *run* it against a real resolver-version pair — the classifier itself is offline |

## Design rationale

The full design decisions behind these — why selection accuracy alone was the wrong single metric,
why cost needs to be expressed in requests rather than tokens, why `resolver_only_rate` had to be
computed from `DiffReport` rather than `CiSummary` — are documented in
[`docs/improvement-plan-provenance-diff.md`](../docs/improvement-plan-provenance-diff.md), Phases E
and G. This directory doesn't repeat that reasoning; it's the "here's how to actually run it, and
here's what we found" companion to it.
