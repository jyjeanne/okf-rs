# MCP tool-selection benchmark

**Question:** does a model pick the right `relation` inside the consolidated `graph(relation=...)`
tool as reliably as it used to pick the right specialized tool name, before `okf-mcp` 0.3.0
collapsed 13 `graph_*` tools into one? And when it doesn't, how expensive is that miss really —
does the mistake announce itself, or does it look exactly like a correct answer?

## Code

- [`crates/okf-mcp/src/tool_selection_benchmark.rs`](../../crates/okf-mcp/src/tool_selection_benchmark.rs)
  — the offline half: the fixed 14-question set, each with a known-correct `relation`/answer
  checked directly against a fixture bundle (no model involved), the reconstructed pre-0.3.0
  specialized tool-name mapping, `scores_correctly`, and `is_negative_response` (the rule behind
  the `DetectableWrong` failure mode, below).
- [`crates/okf-mcp/src/tool_selection_live.rs`](../../crates/okf-mcp/src/tool_selection_live.rs)
  — the live half: a small OpenAI-compatible tool-calling client, `FailureMode` classification,
  and the report both designs are scored and rendered through.

## Run it

```sh
export OKF_BENCHMARK_MODEL_BASE_URL=http://localhost:11434/v1   # any OpenAI-compatible endpoint
export OKF_BENCHMARK_MODEL=llama3.1                              # the model to benchmark
# OKF_BENCHMARK_MODEL_API_KEY optional, for a hosted provider

okf-mcp --benchmark-tool-selection
```

Deliberately opt-in and never run in CI: a live model call isn't offline, deterministic, or free,
unlike every other test/benchmark in this codebase. `OKF_BENCHMARK_MODEL_*` is a separate set of
variables from `search_semantic`'s `OKF_ENRICH_*`, so the model under benchmark and the one used
for description enrichment can differ.

## What it reports

Per design (consolidated `graph` vs. the 13 reconstructed specialized `graph_*` tools):

- **Tool/relation-selection accuracy** — did the model pick the right tool/relation at all.
- **Final-answer accuracy** — did the resulting call's response actually contain the expected
  answer (only meaningful when selection was correct).
- **`requests_per_answered_question`** — `1 / final-answer accuracy`: cost expressed in requests,
  the unit that stays comparable regardless of session length (tool schemas are re-serialized into
  every request, so the *savings* side of consolidation scales predictably with request count; the
  *cost* of a wrong selection doesn't scale with tokens at all — one bad selection is one extra
  round trip carrying the whole conversation prefix, however large that prefix is).
- **A four-way failure-mode breakdown**, not one undifferentiated "wrong" count:
  - `Correct` — the expected tool/relation was chosen.
  - `[LOUD-FAIL]` — no tool matched, or the tool/endpoint itself rejected the call outright.
    Visible the moment it happens; whatever's driving the session can react to it.
  - `[DETECTABLE-WRONG]` — the wrong tool/relation was chosen, but its response is itself one of
    `okf-query`'s own empty/negative "nothing found" results (`is_negative_response`) — a signal a
    downstream consumer could act on without knowing the right answer.
  - `[SILENT-WRONG]` — the wrong tool/relation was chosen, and the response is real, populated
    data — indistinguishable in shape from a correct answer. The expensive case: nothing about the
    response itself gives anything downstream a reason to retry it.

## Why four categories, and why not more

The specialized-vs-consolidated tradeoff is often framed as one accuracy percentage, but the two
designs fail in different registers: a hallucinated specialized tool name tends to fail loudly (no
matching tool), while a wrong `relation` value inside one consolidated tool always produces a
well-formed call. Collapsing both into "wrong" flatters whichever design happens to fail in the
cheap (loud) register more often.

`DetectableWrong` is a deliberately narrow, conservative middle category — not an attempt to model
"did the model notice its own mistake," which isn't measurable by a harness that scores one tool
call per question and never asks a model to reflect on its own answer. What *is* measurable without
a second model call: whether the wrong tool's response is structurally empty for these arguments.
It does **not** catch the broader "populated, but obviously about a different subject" case (e.g. a
`stats` breakdown returned for a "who calls X" question) — that stays `SilentWrong`, since a general
notion of "expected response shape per question" risks false positives a narrower rule avoids.

## Results so far

No real live-model run has been recorded yet — only real dogfooding against real, separate-process
mock HTTP servers (proving the full chain: CLI flag parsing → env-var config → socket HTTP request
→ OpenAI-compatible response parsing → dispatch through the real `graph` tool → scoring → report
rendering), not a live third-party model. See
[`ROADMAP.md`](../../ROADMAP.md#improvement-plan--provenance-depth-graph-diff--mcp-tool-selection)'s
Phase E verification section for those runs' exact numbers. Running this against a real model and
recording the result here is open — see
[`docs/improvement-plan-provenance-diff.md`](../../docs/improvement-plan-provenance-diff.md)'s
Phase E/G for the full design.
