# Community feedback: the rust-analyzer self-disagreement mechanism (August 2026)

A fourth round of feedback from the same external reviewer (Dipankar Sarkar, Medium), this time on
the within-version, run-to-run `--lsp` non-determinism first reported in
[`benchmarks/resolver-stability/README.md`](../../benchmarks/resolver-stability/README.md) (and
distilled from the raw two-run measurement in
[`docs/improvement-plan-provenance-diff.md`](../improvement-plan-provenance-diff.md)'s Phase G).
Kept verbatim here for traceability, the same way the three earlier rounds are recorded in this
directory. The distilled response — what the review gets right, what it found once checked against
this codebase's actual client, and the actual work it drove — lives in
[`docs/improvement-plan-provenance-diff.md`](../improvement-plan-provenance-diff.md#12-phase-h--rust-analyzer-readiness-gating-not-first-success-medium-review-august-2026--shipped)'s
Phase H.

Recorded 2026-08-18.

---

## The feedback, verbatim

> The self-disagreement has a mechanism worth ruling out before you call it baseline noise:
> rust-analyzer starts answering requests before the workspace is fully loaded. Proc-macro
> expansion and build-script OUT_DIR code land late, so a symbol defined in generated code can
> resolve on one run and miss on the next depending on when the query lands relative to load
> state.
>
> Two checks separate that from real nondeterminism. Gate each run on the server reporting ready
> (the indexing progress token completing, or analyzerStatus) instead of on the first response
> coming back. Then look at which concepts flipped: if they cluster in macro-expanded or
> build-script-generated code, it is a race with load state and it is fixable. If they are spread
> across plain source, you have something more interesting than a version diff.
>
> Either way the sample is the constraint. Two repeats gives you one pair. Ten repeats of a single
> version gives you a within-version disagreement distribution, and 0.2% only means something
> measured against that.
>
> The four-way split plus requests_per_answered_question is the right shape.
> DetectableWrong scoped to the tool response being empty or negative is the version that survives
> someone else running it.

---

## What this gets right, checked against the actual client rather than assumed

The "gate on ready, not on the first response" diagnosis matched this codebase's actual
`okf_lsp::LspClient` mechanism exactly, once read rather than guessed at:
`okf_analyzer::lsp::resolve_ambiguous_calls` retried the *first* `textDocument/definition` query
per language up to 20 times (10s total), then set a `warmed_up` flag and gave every later query for
that language exactly one attempt, zero retry, forever. That flag used "some earlier query for this
language already got an answer" as a proxy for "the server has finished loading" — the exact
first-response heuristic the review names, just one layer removed from being visible as such. See
Phase H for why that proxy is itself a source of the disagreement, not just a missed opportunity to
avoid it.

The one thing this project's own source can't confirm or deny is the proc-macro/build-script half of
the mechanism specifically — okf-rs itself has no build scripts and negligible proc-macro use, so
the one real flip found in the original two-run measurement
(`cmd_check_determinism → Project::load`, a cross-crate call) is best read as the same *class* of
race (a query landing before some specific unit of work — here, a slower crate's own index, not
macro expansion — has settled), not literally the proc-macro/build-script case as written. Phase H's
stress-test run (repeats well beyond two) found the same class of race again, now visibly clustered
by file rather than uniformly spread — see Phase H for the concrete numbers and what triggers it.

The four-way failure-mode split and `requests_per_answered_question()`, and `DetectableWrong`'s
narrow empty/negative-response scoping, were both already shipped in Phase G — this feedback
confirms that design rather than asking for new work on it.

## Response

See [`docs/improvement-plan-provenance-diff.md`](../improvement-plan-provenance-diff.md)'s Phase H
for the concrete, shipped follow-up: real `$/progress`-based readiness gating in `okf_lsp::LspClient`
(replacing the first-success proxy), a smaller retry budget applied to *every* ambiguous-call query
rather than only the first per language, and `okf-rs generate --check-determinism-repeats N` to turn
the "two repeats gives you one pair" measurement into an actual within-version disagreement
distribution instead of a single sample.
