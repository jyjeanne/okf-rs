# Community feedback: provenance, graph diff & MCP tool-shape follow-up (August 2026)

A second, more detailed round of feedback from the same external reviewer (Dipankar) whose first
pass is recorded in
[`2026-08-community-roadmap-review.md`](2026-08-community-roadmap-review.md). Kept verbatim here
for traceability, the same way that first review was — the distilled, gap-checked version that
actually drives work lives in
[`docs/improvement-plan-provenance-diff.md`](../improvement-plan-provenance-diff.md).

Recorded 2026-08-14. Source: external review (Medium), proposing a formal `Provenance`/
`ProvenanceOrigin` model, resolver identity/version tracking, provenance-aware graph diffing,
CI failure/warning policy, MCP session-overhead measurement, and specialized-vs-consolidated MCP
tool benchmarking, structured as a phased technical implementation plan with tests and acceptance
criteria per phase.

**Important context the raw feedback below doesn't have**: most of this review's phases 1-3 and
9 restate work that already shipped in this repository *before* this review was written — see
["Optimize the MCP API"](../../ROADMAP.md#improvement-plan--ai-native-platform-maturity-community-feedback),
["Record edge provenance"](../../ROADMAP.md#improvement-plan--ai-native-platform-maturity-community-feedback),
["Confidence levels"](../../ROADMAP.md#improvement-plan--ai-native-platform-maturity-community-feedback),
and
["Measure session-level MCP performance"](../../ROADMAP.md#improvement-plan--ai-native-platform-maturity-community-feedback)
in `ROADMAP.md`. The distilled plan linked above separates what's already done from what's
genuinely new (resolver-version tracking, provenance-*aware diff classification*, a `diff --ci`
policy, artifact-level reproducibility metadata, and a real tool-selection-accuracy benchmark)
rather than re-proposing shipped work.

---

## The proposal, condensed

The reviewer's own summary of their proposal, in their words:

> Provenance: probably the most important architectural suggestion... an edge could carry
> provenance such as `origin: tree-sitter, language: rust, resolver: none` or
> `origin: lsp, language: rust, server: rust-analyzer, server_version: 2026-08-xx`. Without
> provenance you don't know whether the source code changed, Tree-sitter changed, rust-analyzer
> changed, or the LSP resolution algorithm changed. With provenance you can distinguish a
> **source change** (syntactic edge changed → CI failure) from a **resolver/tooling change**
> (semantic edge changed, resolver version bumped → CI warning).
>
> MCP overhead: he is challenging your benchmark methodology... a session with only 2-3 queries,
> the fixed MCP cost can dominate. His proposed metric is essentially
> `break-even = fixed MCP schema cost / tokens saved per structural query`.
>
> Don't necessarily replace `graph_callers`, `graph_callees`, etc... benchmark 7 specialized
> `graph_*` tools vs. 1 consolidated `explore` tool and measure schema token cost,
> tool-selection accuracy, number of calls, total tokens, latency, and answer accuracy.

## The full phased plan submitted

The reviewer's complete "OKF-RS — Provenance, Graph Diff and MCP Optimization Development Plan"
(12 phases: provenance data model, edge attachment, resolver identity/version, provenance-aware
graph diff, graph diff model/tests, CI policy, reproducibility metadata, MCP baseline benchmark,
specialized-vs-consolidated MCP tools benchmark, tool-selection benchmark, benchmark report,
integration tests, golden fixtures, backward compatibility, and a Definition of Done checklist)
was submitted as a Medium-article-style development specification, structured so each phase maps
to concrete unit tests and acceptance criteria rather than being a prose roadmap. It is not
reproduced a second time here to avoid drift between two copies — the original submission is the
source of record for exact wording; this file's job is traceability (who said what, when), and
the linked improvement plan's job is turning it into buildable, codebase-grounded work.

## Recommended follow-up article (reviewer's suggestion)

> This gives you excellent material for a follow-up Medium article: "What We Learned About MCP
> Overhead, Graph Provenance, and AI-Ready Knowledge Graphs" would be a much deeper technical
> piece than the original introduction.

Not actioned here (a marketing/content decision, not a codebase change) — noted for whoever owns
that follow-up.
