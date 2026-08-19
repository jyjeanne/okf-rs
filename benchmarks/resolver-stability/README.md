# Resolver-stability benchmark

**Question:** how much of a diff's relationship-level churn between two resolver versions (or any
two independently generated bundles of the same source) is resolver-only metadata versus a genuine
structural rewire? If that rate stays near zero across a real resolver-version bump, resolver-class
changes can safely default to `ignore` in CI instead of `warn` — and if it isn't near zero, that's
a resolver finding worth reporting on its own.

## Code

- [`okf_analyzer::resolver_only_rate(&DiffReport)`](../../crates/okf-analyzer/src/lib.rs) — the
  classifier: the share of relationship-*pair* changes in a diff that are `ResolverChange`/
  `ProvenanceChange` (same target, only resolver identity/version/confidence differs) rather than a
  genuine `SourceChange` (the target itself differs). Computed directly from `DiffReport`, not from
  `CiSummary`'s aggregate counts, so unrelated concept-level churn elsewhere in the same diff can't
  dilute it.
- [`okf-rs diff --ci`](../../crates/okf-cli/src/main.rs) — surfaces the rate on every run with a
  nonzero `RESOLVER CHANGES` count, comparing two git refs.
- [`okf-rs diff-bundles <bundle-a> <bundle-b>`](../../crates/okf-cli/src/main.rs) — the same
  classified report, but for two bundle directories already on disk instead of two git refs. This
  is the one that actually makes the resolver-version measurement runnable: `diff`'s two-git-ref
  comparison has nothing to check out when the two bundles came from the *identical* source
  snapshot, analyzed under two different resolver versions.
- [`okf_lsp::LspClient::wait_until_ready`](../../crates/okf-lsp/src/lib.rs) — real readiness gating
  for the control experiment below: waits for the server's own `$/progress` indexing signal to
  settle before any query is sent, instead of inferring readiness from whether an arbitrary first
  query happened to succeed (the mechanism behind the disagreement this benchmark originally found —
  see "Results so far").
- [`okf-rs generate --check-determinism --check-determinism-repeats N`](../../crates/okf-cli/src/main.rs)
  — the control experiment itself, generalized from one pairwise comparison to an actual
  within-version disagreement distribution: `N` independent analyses, every run after the first
  diffed against run 1, reporting how many comparisons each concept flipped in.

## Run it

```sh
# Generate the same source snapshot under two resolver versions (PATH controls
# which `rust-analyzer` binary `--lsp` spawns):
PATH=/path/to/rust-analyzer-1.90.0/bin:$PATH okf-rs generate . --lsp --no-cache --output /tmp/bundle-old
PATH=/path/to/rust-analyzer-1.94.1/bin:$PATH okf-rs generate . --lsp --no-cache --output /tmp/bundle-new

okf-rs diff-bundles /tmp/bundle-old /tmp/bundle-new
```

Two toolchains can be installed side by side via `rustup`, without needing GitHub release downloads
(useful in a network-restricted environment): `rustup toolchain install <version>` then
`rustup component add rust-analyzer --toolchain <version>`.

**Control experiment, before trusting any cross-version delta**: `--lsp` resolution isn't
guaranteed deterministic run-to-run even under one fixed resolver version (see Results below). Run
`okf-rs generate . --lsp --check-determinism` (twice, independently, same version) first, so a
cross-version difference isn't mistaken for a real version-caused one when it's actually baseline
indexing noise. For more than one pairwise sample — two repeats gives you one pair, not a
disagreement *rate* — add `--check-determinism-repeats N`:

```sh
okf-rs generate . --lsp --check-determinism --check-determinism-repeats 10
```

This runs `N` independent in-process analyses and diffs every run after the first against run 1,
reporting how many of the `N-1` comparisons each concept flipped in — the shape needed to say
"X% of concepts disagree within one version," not just "these two runs happened to differ."

## Results so far

Run for real against this repository's own source (1153 concepts, 54 files), comparing
`rust-analyzer` 1.90.0 (1159e78, 2025-09-14) against 1.94.1 (e408947, 2026-03-25):

```
❌ SOURCE CHANGES: 2
⚠️  RESOLVER CHANGES: 1292
   (99.8% of relationship-level changes in this diff were resolver-only)
```

1292 of 1294 relationship-level changes were purely `resolver_version` metadata — real evidence
that `resolver_changes: "ignore"` is a defensible policy choice for a project willing to accept the
confound below. The 2 remaining `SourceChange` entries were both directions of one edge
(`cmd_check_determinism → Project::load`) and looked, at first, like a genuine cross-version
resolution disagreement — until the control experiment above
(`generate --lsp --check-determinism`) showed `--lsp` resolution on this codebase isn't fully
deterministic run-to-run even holding the resolver version fixed: 1.94.1 disagreed with itself on
2/2 repeated runs (different call sites each time), and 1.90.0 disagreed with itself on 1/3. That
means the one real `SourceChange` found between versions can't be confidently attributed to the
version bump at all — it's at least as well explained by that baseline noise. **The more solidly
reproducible finding here is the non-determinism itself, present in both tested versions, not a
1.90.0-vs-1.94.1 semantic disagreement.**

This same run caught a real, now-fixed display bug: `render_ci_report`'s resolver-only-rate line
originally rounded at zero decimal places, so this genuine 99.8454...% displayed as a bare "100%" —
indistinguishable from an actually-clean diff. See
[`docs/improvement-plan-provenance-diff.md`](../../docs/improvement-plan-provenance-diff.md)'s
Phase G, "`diff-bundles`, and the real measurement it made possible," for the full writeup,
including the exact commands run and the regression test that reproduces this ratio.

### The self-disagreement's mechanism, and what fixing it changed

External review (recorded in
[`docs/feedback/2026-08-rust-analyzer-self-disagreement-review.md`](../../docs/feedback/2026-08-rust-analyzer-self-disagreement-review.md))
named a concrete cause worth ruling in before calling the numbers above baseline noise: a query
landing before the server has actually finished loading the workspace. Read against
`okf_lsp`/`okf_analyzer`'s real client (not guessed at), that's exactly what was happening —
`resolve_ambiguous_calls` gated its retry budget on whether *any* earlier query for a language had
already succeeded, a first-response proxy for "the workspace is ready" that was itself a source of
the disagreement. `okf_lsp::LspClient` now gates on the server's own `$/progress` indexing signal
instead (`LspClient::wait_until_ready`), and the per-query retry budget applies to every ambiguous
lookup, not just the first one per language — see
[`docs/improvement-plan-provenance-diff.md`](../../docs/improvement-plan-provenance-diff.md)'s
Phase H for the full mechanism writeup.

Measured again, for real, after the fix:

- **Ordinary conditions** (`--check-determinism`, the default two repeats, one run at a time):
  three separate invocations, all clean — `Deterministic: 2 independent generate --lsp runs on .
  all produced byte-identical output (1173 concepts)` every time. A real improvement over the
  pre-fix 2/2 and 1/3 disagreement rates above, at the same repeat count most projects would
  actually run in CI.
- **Stress test** (`--check-determinism-repeats 6`, deliberately run *concurrently with a second,
  independent instance of the same command* on a 4-core sandbox — the harshest case, not the
  typical one): still found one run (the very first `LspClient` started, cold-starting directly
  into contention from the second job) disagreeing with all 5 later runs on exactly one edge:

  ```
  Non-deterministic: 2 file(s) flipped across 6 independent `generate --lsp` runs on . :
    5/5 repeat run(s) disagreed with run 1 on at least one file
    functions/crates/okf-graph/src/Graph/get.md: differed in 5/5 comparison(s) against run 1
    functions/crates/okf-graph/src/Graph/transitive_callers.md: differed in 5/5 comparison(s) against run 1
  ```

  The content diff: run 1's render was missing one edge every other run found
  (`Graph::get → Graph::transitive_callers`) — a `textDocument/definition` query that came back
  empty on the first attempt and stayed empty through all 4 retries (~1.2s) while the concurrent
  job starved it of CPU. Applying the review's own diagnostic — "look at which concepts flipped: if
  they cluster... it is a race with load state and it is fixable" — this flip is a single, tightly
  clustered pair, not scattered across unrelated plain source, confirming the mechanism rather than
  turning up something stranger. Readiness gating narrows the window; it doesn't close it under
  sustained, adversarial CPU contention, and this benchmark now says so with a number instead of a
  guess.

  A follow-up review proposed a sharper mechanism for this specific flip — salsa's per-crate,
  demand-driven lowering rather than a readiness-detection gap, since `$/progress` covers indexing,
  not necessarily every crate's first-touch analysis cost. Checked against the source, it doesn't
  fit this incident: `Graph::get` and `Graph::transitive_callers` are the same `impl` block in the
  same crate (`okf-graph`), not a cross-crate call, and that crate was already warm from earlier
  queries in the same run. The CPU-contention explanation above still stands; see
  [`docs/feedback/2026-08-rust-analyzer-salsa-readiness-review.md`](../../docs/feedback/2026-08-rust-analyzer-salsa-readiness-review.md)
  for the full exchange.

### Testing the lazy-analysis theory directly: `--warm-crate`

`okf-rs generate --check-determinism --lsp --warm-crate <name> [--warm-queries N]` exists to test
the salsa demand-driven-lowering theory on its own terms, separated from CPU contention: before the
real, measured resolution pass, it fires `N` throwaway `textDocument/definition` queries into the
named crate, forcing whatever lazy analysis a salsa-backed server defers to first contact before
anything is timed. Comparing a `--warm-crate` run against an otherwise-identical run without it, if
a flip only appears cold, that's real evidence for the lowering theory; if it appears either way
(or not at all), contention is still the better explanation.

Run for real against this repository, on the same 4-core sandbox as the measurements above, without
any deliberately concurrent second job this time — the sandbox's own baseline load turned out to be
enough on its own: a plain `generate --lsp --check-determinism-repeats 6` (no `--warm-crate`, no
extra concurrent process) produced a fresh, genuinely **cross-crate** flip on the very first attempt
— unlike the intra-crate `Graph::get`/`transitive_callers` case above, this one really does cross a
crate boundary (`okf-cli` → `okf-core`):

```
Non-deterministic: 2 file(s) flipped across 6 independent `generate --lsp` runs on . :
  5/5 repeat run(s) disagreed with run 1 on at least one file
  functions/crates/okf-cli/src/cmd_scan.md: differed in 5/5 comparison(s) against run 1
  functions/crates/okf-core/src/Project/load.md: differed in 5/5 comparison(s) against run 1
```

Three more plain (cold) repeats of the same command came back clean; three `--warm-crate okf-core
--warm-queries 30` repeats also came back clean — 1 flip in 3 cold attempts vs. 0 in 3 warm ones, a
direction consistent with the lazy-analysis theory but nowhere near a real distribution from a
single event.

A full 10-vs-10 batch followed (still this same 4-core sandbox, no artificial contention — its own
baseline load is what produces flips here): **10 plain `--check-determinism-repeats 6` runs, then 10
`--warm-crate okf-core --warm-queries 30` runs.** Cold: 1 of the 10 flipped —

```
Non-deterministic: 5 file(s) flipped across 6 independent `generate --lsp` runs on . :
  5/5 repeat run(s) disagreed with run 1 on at least one file
  functions/crates/okf-analyzer/src/cache/AnalysisCache/load.md: differed in 5/5 comparison(s) against run 1
  functions/crates/okf-analyzer/src/cache/round_trips_through_save_and_load.md: differed in 5/5 comparison(s) against run 1
  functions/crates/okf-cli/src/cmd_check_determinism.md: differed in 1/5 comparison(s) against run 1
  functions/crates/okf-cli/src/cmd_generate.md: differed in 5/5 comparison(s) against run 1
  functions/crates/okf-core/src/Project/load.md: differed in 5/5 comparison(s) against run 1
```

— every one of those five files is a *different caller* (from `okf-analyzer` and `okf-cli`, two
different crates) missing an edge into the exact same callee: `okf-core::Project::load`. Warm: 0 of
the 10 flipped, none of them on `Project::load` or anything else.

Combined across both batches (the earlier 3-vs-3 feasibility check plus this 10-vs-10 run): **2
flips in 13 cold attempts (15%) vs. 0 in 13 warm ones (0%)**, and — this is the more specific claim —
*every single flip observed, in either batch, lands on `okf-core::Project::load` specifically, the
exact crate `--warm-crate okf-core` targets.* That specificity is more mechanistically telling than
the raw 2-vs-0 count: it isn't "cold runs are generally flakier," it's "the one callee that keeps
going missing is the one crate being warmed makes disappear entirely."

That said, the raw count alone is not statistically significant — a one-sided Fisher's exact test on
the 2/13 vs. 0/13 table gives p ≈ 0.24, comfortably above conventional significance thresholds. A
15% base rate producing zero flips in 13 tries by chance alone is not remarkable. So: real,
mechanistically coherent corroborating evidence for the lazy-analysis theory (specifically for
`okf-core::Project::load` as a genuine first-touch cold crate, distinct from the `Graph::get` case
this document otherwise concerns), but not proof — a larger batch (25-30 per arm, the point at which
a 15%-vs-0% true difference would clear conventional significance) is the next step if this needs to
move from "consistent with" to "established."

**Open**: whether the 2/13-vs-0/13 `--warm-crate` gap holds up at a size that clears statistical
significance (p ≈ 0.24 at n=13 per arm; a 25-30-per-arm batch is the next step) is still open — see
above.

**Open**: this measurement hasn't been run on a corpus other than okf-rs's own source, or across a
wider resolver-version gap. A different codebase's ambiguous-call density could plausibly show a
different rate — that's exactly the kind of project-specific number this benchmark exists to let a
team collect for itself rather than assume from one worked example. The residual stress-test gap
above is also open: further hardening (a larger `READY_QUIET_PERIOD`, or a harder readiness signal
than a quiet-period heuristic) is possible future work, not something this phase claims to have
closed.
