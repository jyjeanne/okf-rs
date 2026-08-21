# Community feedback: cache priming is the lever, and it explains both prior readings (August 2026)

A sixth round of feedback from the same external reviewer (Dipankar Sarkar, Medium), naming the
concrete rust-analyzer configuration lever behind why a genuinely cold crate had been so hard to
catch, plus a sharp read on the earlier stress-test flip's retry pattern. Kept verbatim here for
traceability, the same way the five earlier rounds are recorded in this directory.

Recorded 2026-08-19.

---

## The feedback, verbatim

> The lever you want is cache priming. rust-analyzer.cachePriming.enable defaults to true, and on
> workspace load the server walks the crate graph and lowers def maps for the whole workspace up
> front. That is most of why a genuinely cold crate is so hard to catch by chance: by the time your
> first query lands, priming has usually already run, so almost every crate is warm at the point you
> start measuring. Set it to false in the server's initializationOptions for the determinism harness
> and lowering goes back to being demand-driven, so the first query into each crate is cold by
> construction rather than by luck.
>
> That also fixes the sample size. With priming off there is exactly one cold-crate observation per
> crate per process, and you know in advance which query it will be, so sort the probe list by crate
> and label the first probe into each. A run over N crates gives you N cold-first measurements
> instead of one lucky flip every few thousand queries.
>
> One control worth keeping: first-query lowering cost scales with the crate's item count, so
> compare cold against warm within the same crate in the same process, not across crates. Otherwise
> the crate-size spread swamps the effect you are trying to measure, and okf-graph is not the same
> size as its dependencies.
>
> Your contention read looks right to me independently of any of that. Four retries all coming back
> empty over 1.2 seconds is a starvation shape. Demand-driven lowering would normally resolve on a
> retry once the def map is built, because the second attempt gets the memoized result. Staying
> empty through every retry says the query never got scheduled, not that it was doing expensive
> work.

---

## Checked against the actual client, and against a real measurement

`okf_lsp::LspClient::initialize` had never sent `initializationOptions` at all — `rust-analyzer.cachePriming.enable`
had been at its default (`true`) in every measurement this project has made, including the full
10-vs-10 `--warm-crate` batch. And `LspClient::wait_until_ready`'s own doc comment, written back in
Phase H for an unrelated reason, already named *"proc-macro/build-script cache priming"* as one of
the `$/progress` phases it waits through — independent, pre-existing confirmation that priming is
real and that this project's own readiness gate waits for the whole priming pass before measuring
anything. That is exactly why a genuinely cold crate had been so hard to catch: 2 flips in 13 tries
across the whole `--warm-crate` investigation.

Both claims were implemented and measured for real, not just accepted:

1. **`okf_lsp::LspClient::start_with_init_options`** (`crates/okf-lsp/src/lib.rs`) forwards a caller-
   supplied `initializationOptions` object; `disable_rust_analyzer_cache_priming()` returns
   `{"cachePriming": {"enable": false}}`.
2. **`okf-rs cold-crate-probe [path]`** (`crates/okf-analyzer/src/lsp.rs`'s `probe_cold_crates`,
   `crates/okf-cli/src/main.rs`'s `cmd_cold_crate_probe`) probes one crate at a time, in one process,
   priming disabled: the first `textDocument/definition` query into each distinct crate an ambiguous
   call was found in, immediately followed by a repeat of the exact same query — cold vs. warm,
   within the same crate, exactly the control the feedback names in its third paragraph. This turns
   "wait for a lucky flip every several runs" into one cold measurement per crate, every run, per the
   feedback's second paragraph.

### The result, run for real against this repository (18 crates, reproduced twice)

```
Cold-crate probe: 18 crate(s) probed on . (cache priming disabled):
  okf-analyzer         cold: found ( 7745-8305ms)   warm: found (    0ms)
  okf-cli              cold: found ( 7259-7937ms)   warm: found (    1ms)
  okf-docs             cold: found ( 2047-2057ms)   warm: found (    0ms)
  ...every other crate...     cold: found (   3-238ms)   warm: found (0-5ms)
  0/18 cold probes came back empty, 0/18 warm (immediate-repeat) probes did
```

Two things this establishes, both new:

- **Cold, demand-driven lowering cost is real and highly uneven across crates** — most of this
  project's 18 crates lower in under 250ms even stone cold, but `okf-cli` and `okf-analyzer`
  (this project's two largest, most heavily-typed crates) take **7-8 seconds**, and `okf-docs` takes
  ~2 seconds, reproducibly across two separate probe runs. Against `okf_analyzer::lsp`'s
  `DEFINITION_RETRIES: 2` / `DEFINITION_RETRY_DELAY: 300ms` (at most ~300ms of retry budget beyond
  the first attempt), an 8-second cold lowering cost is not something that budget was ever going to
  cover — it depends entirely on cache priming having already paid it before the real resolution
  pass starts.
- **Every one of this project's own observed flips — the original `okf-cli::cmd_scan` one, and every
  file in the 10-vs-10 batch's one cold flip — had its *caller* inside `okf-cli` or `okf-analyzer`**,
  the two crates this probe found to be by far the most expensive to cold-lower. `textDocument/definition`
  is answered from the *caller's* position, so the crate whose analysis the query actually needs
  first is the caller's crate, not necessarily the callee's — and that lines up exactly.

### Reconciling the feedback's two points against each other

The priming point and the starvation point looked like they could be in tension: if lowering
`okf-cli` really costs ~8 seconds, why would the failing queries come back *empty* across all 4
retries in only ~1.2 seconds total, rather than the client patiently waiting ~8 seconds and getting a
correct answer (which is exactly what this probe's own cold, uncontended measurement shows
happening)? They're not in tension once read together: the probe shows that *without* contention,
`okf-cli`'s cold lowering runs to completion and returns correctly, just slowly. The stress-test
flip's four fast, empty responses in 1.2 seconds are not the client waiting a long time and getting
nothing — they're the server returning quickly with *no* answer, four times, rather than doing the
slow work at all. That is a materially different, more precise shape than "lowering was too slow" —
it matches the feedback's own diagnostic: a query that never got scheduled to do the real (evidently
~8-second) work, not one that started the work and ran out of time. Under ordinary, uncontended
conditions, this project's `wait_until_ready` already pays that ~8-second cost once, up front, via
cache priming — which is exactly why plain `--check-determinism` runs without artificial contention
have stayed clean.

## Response

Implemented and measured, not just filed: `okf-rs cold-crate-probe` (see above) is a permanent
addition to this project's diagnostic surface — `disable_rust_analyzer_cache_priming`,
`LspClient::start_with_init_options`, and the probe command itself, all with unit and live-server
test coverage. It measurably improves on the earlier `--warm-crate` approach exactly as the feedback
predicted: one cold-vs-warm, same-crate measurement per crate per run, instead of hoping a whole-
project `--check-determinism-repeats` invocation happens to land on the one crate that matters.

No change follows for `wait_until_ready`, `resolve_ambiguous_calls`, or the retry constants
themselves from this round — the readiness gate already appears to be doing the right thing under
ordinary conditions (cache priming pays the ~8-second cost before any real query, per the clean
`--check-determinism` runs on record), and the residual gap is specifically about what happens to
that ~8-second job under sustained contention, which this round's tooling makes measurable but
doesn't yet directly probe (a contended cold-crate-probe run, deliberately run alongside a second
concurrent process, is the natural next experiment if this needs to go further).

See `benchmarks/resolver-stability/README.md` for the full reproducible run and
[`docs/feedback/2026-08-rust-analyzer-salsa-readiness-review.md`](2026-08-rust-analyzer-salsa-readiness-review.md)
for the round this one follows on from.
