# Community feedback: is the Phase H stress-test survivor a salsa per-crate lowering gap? (August 2026)

A fifth round of feedback from the same external reviewer (Dipankar Sarkar, Medium), this time
reacting to Phase H's own published result rather than the original problem: the one edge that
still flipped in the stress-test run recorded in
[`docs/improvement-plan-provenance-diff.md`](../improvement-plan-provenance-diff.md#12-phase-h--rust-analyzer-readiness-gating-not-first-success-medium-review-august-2026--shipped)
and [`benchmarks/resolver-stability/README.md`](../../benchmarks/resolver-stability/README.md)
(`Graph::get → Graph::transitive_callers`, 5/5 disagreement under deliberate CPU contention). Kept
verbatim here for traceability, the same way the four earlier rounds are recorded in this directory.

The reviewer's message opened by repeating round four's original feedback verbatim (already
addressed in full by Phase H — see
[`docs/feedback/2026-08-rust-analyzer-self-disagreement-review.md`](2026-08-rust-analyzer-self-disagreement-review.md)),
then added the new material recorded below.

Recorded 2026-08-19.

---

## The feedback, verbatim

> The one edge that still flips under contention may not be a readiness-detection gap. If the
> server is rust-analyzer, its analysis is demand-driven through salsa: the indexing pass that
> emits $/progress builds the symbol index, while def-map lowering and type inference for a crate
> happen on the first query that touches it. A workspace can be fully indexed and a cross-crate
> definition query can still be the call that pays the lowering cost. Readiness is a property of
> the crate the query lands in rather than of the server, which is why gating on the global signal
> narrows the window without closing it, and why the survivor was Graph::get to
> Graph::transitive_callers rather than something intra-crate.

---

## What this gets right, and where it doesn't hold up against this incident specifically

The general architectural point is real and worth keeping on file: rust-analyzer's `$/progress`
indexing signal covers roots-scanning and the crate graph, not necessarily every crate's def-map
lowering and type inference, which salsa computes lazily on first demand. A global "ready" signal
is in principle a necessary, not sufficient, condition for every subsequent query being cheap — a
crate nothing has queried yet could still owe its first caller a lowering cost the `$/progress`
stream never announced. That's a legitimate, more precise refinement of "gate on readiness" than
Phase H's own writeup offered, and worth recording as an open question for any *future* flip this
project sees.

It doesn't fit the one flip actually on record, though — checked against the source rather than
assumed, the way this project's previous rounds of feedback have been:

- `Graph::get` and `Graph::transitive_callers` are not in different crates. Both are defined in the
  *same* `impl<'a> Graph<'a>` block, in the same file,
  [`crates/okf-graph/src/lib.rs`](../../crates/okf-graph/src/lib.rs) (`get` at line 110,
  `transitive_callers` at line 369, which calls `self.get(i)` directly at line 389). There is no
  crate boundary for this specific `textDocument/definition` query to cross — the "why the survivor
  was ... rather than something intra-crate" framing has the two functions' relationship backwards:
  the survivor *is* intra-crate.
- The mechanism Phase H already documented for this exact flip isn't a first-touch lowering cost
  either: `okf-graph` was far from an untouched crate by the time this query landed — `resolve_ambiguous_calls`
  had already sent many other `textDocument/definition` queries into the same crate earlier in the
  same run, any of which would have already forced its def-map lowering under salsa's normal
  memoization. What Phase H's writeup actually traced this flip to was `wait_until_ready` doing its
  job — the workspace really was ready — followed by a single query landing on a CPU-starved
  process (a second, independent `generate --lsp` invocation was deliberately running concurrently
  on the same 4-core sandbox as this stress test's whole point) and not getting an answer inside its
  4-retry, ~1.2s budget. That is resource contention on an already-warm crate, not a demand-driven
  lowering cost on a cold one.

So the specific evidence offered for the theory doesn't survive a read of the code it's about, even
though the theory itself — readiness as a per-crate rather than per-server property — is a real
property of salsa-backed servers and worth keeping in mind if a future flip does land on a crate's
genuinely first query.

## Response

No code change follows from this round: the incident it's diagnosing doesn't match the diagnosis,
and Phase H's own explanation (CPU contention exhausting the retry budget on an already-indexed,
already-touched crate) still stands as the best-supported account of the one flip actually measured.
The per-crate/salsa distinction is recorded here rather than acted on, in case a future
`--check-determinism-repeats` run turns up a flip that *does* land on a crate's genuinely first
query — at which point "prime every crate with a cheap query before running the real ones" would be
the concrete follow-up, not a change to `wait_until_ready` itself.
