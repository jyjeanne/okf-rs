# Community feedback: scoring the tool-consolidation benchmark (August 2026)

A third round of feedback from an external reviewer (Dipankar Sarkar, Medium), this time on the
Phase E tool-*selection* benchmark itself (`okf_mcp::tool_selection_benchmark`/
`tool_selection_live`, shipped in
[`docs/improvement-plan-provenance-diff.md`](../improvement-plan-provenance-diff.md)) and on
Phase B/C's provenance-aware diff classification. Kept verbatim here for traceability, the same
way the two earlier rounds are recorded in this directory. The distilled response — what the
review gets right about this codebase, what doesn't quite apply to it as written, and the actual
work it drove — lives in
[`docs/improvement-plan-provenance-diff.md`](../improvement-plan-provenance-diff.md#11-phase-g--benchmark-scoring-and-ci-signal-follow-up-medium-review-august-2026--shipped)'s
Phase G.

Recorded 2026-08-17.

---

## The feedback, verbatim

> The trap in that benchmark is that the two interfaces fail in different registers, so a single
> selection-accuracy number will flatter the consolidated one. With the specialized `graph_*`
> tools a wrong choice usually fails loudly: no tool matches, or the call errors on a schema the
> model cannot satisfy. With one `explore` tool the model always gets a well-formed call away, and
> a wrong mode argument returns real data that is simply the wrong data. Score those as separate
> outcomes. Silent-wrong is the expensive one, because nothing downstream retries it.
>
> For the break-even, the unit that matters is turns, not tokens. Tool schemas are re-serialized
> into the prompt on every request in a session, so consolidating N tools saves roughly N-1
> schemas multiplied by the number of requests, and that saving is real and predictable. The cost
> side is not predictable in tokens at all: one bad selection costs a full extra round trip
> carrying the entire conversation prefix, and if it is a silent-wrong it may cost the whole
> session. Express both sides as requests-per-answered-question and the comparison stops depending
> on how long the session ran.
>
> On the diff work, the classification you shipped enables something worth measuring on your own
> corpus: the rate at which a resolver bump alone rewrites edges. If that number is near zero
> across a rust-analyzer minor version, resolver-class changes can default to ignore in CI and the
> policy knob mostly disappears. If it is not near zero, that is a rust-analyzer finding worth
> publishing on its own.

---

## What this gets right, and one naming mismatch worth flagging up front

The review's "score those as separate outcomes" point is correct and, before Phase G, genuinely
unaddressed: the live benchmark's `DesignReport` already recorded, per outcome, whether the
call errored at all (`error: Option<String>`) separately from whether the wrong tool/relation was
chosen (`tool_selection_correct: bool`) — the *data* to distinguish a loud failure from a
well-formed-but-wrong call already existed — but nothing aggregated or reported that distinction.
`selection_accuracy()` and the rendered `[WRONG]`/`[ERROR]` lines treated both as one undifferentiated
"wrong" bucket. See Phase G below for the fix.

One thing the review's own framing doesn't quite map onto this codebase, worth correcting rather
than silently reinterpreting: this server's actual `explore` tool (`crates/okf-mcp/src/tools.rs`)
has no mode argument at all — it always returns the same fixed bundle of facts (signature,
description, callers, callees, blast radius, API/cycle membership) for one concept. There is no
"wrong mode" failure mode for `explore` to have, because there's no mode to pick. The tool this
review's critique actually describes — one consolidated entry point with an argument that selects
*which* query runs, where a wrong value returns real-but-wrong data rather than an error — is
`graph(relation=...)`, the one Phase E already benchmarks. The distilled response below is scoped
to `graph`, matching what Phase E actually measures, not to `explore`.

## Response

See [`docs/improvement-plan-provenance-diff.md`](../improvement-plan-provenance-diff.md)'s Phase G
for the concrete, shipped follow-up: a `FailureMode` split (`Correct`/`LoudFailure`/`SilentWrong`)
on the live tool-selection benchmark, a `requests_per_answered_question()` metric expressed in the
unit this review argues actually matters, and a `resolver_only_rate()` on `okf-rs diff --ci`'s
`CiSummary` so a project can watch this exact number on its own corpus instead of guessing from one
worked example.
