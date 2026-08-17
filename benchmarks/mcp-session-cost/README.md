# MCP session-level cost benchmark

**Question:** is registering `okf-mcp`'s tools with an MCP client worth it — given every tool
schema is re-serialized into the system prompt for the whole session whether it's used or not, how
many structural queries does a session need to ask before that fixed cost pays for itself?

## Code

[`crates/okf-mcp/src/benchmark.rs`](../../crates/okf-mcp/src/benchmark.rs) — fully offline, no LLM
call or tokenizer dependency (tokens estimated via the common ~4-characters-per-token rule of
thumb, the same heuristic this project's own README already used for its hand-picked worked
example before this benchmark existed).

## Run it

```sh
okf-mcp <project> --benchmark
```

Point it at any project root with a generated bundle (defaults to `.`) — no client, no live
endpoint, no network access needed.

## What it reports

- **`tools/list` schema size** — tool count, byte size, and estimated token count: the fixed cost
  paid once per session regardless of use.
- **Per-query savings** — a deterministic sample of up to 5 `Function`/`Method` concepts that have
  at least one caller, each compared "naive" (grep-and-read every project file, `.gitignore`-aware,
  containing an unqualified substring match on the concept's name) against the real `graph` tool's
  response size for the same "who calls this?" question. Deliberately pessimistic on the naive
  side: a short, common name inflates it in exactly the cases where an agent would also waste
  tokens opening the wrong file.
- **Break-even** — how many structural queries, at the sampled average saving, it takes to recoup
  the fixed schema cost.
- An explicit, undone gap named in its own output rather than fabricated: no comparison against
  RAG-based retrieval, which would need a real vector-store/chunking/re-ranking baseline to be a
  fair number.

## Results so far

Dogfooded against this repository's own bundle (622 functions, 86 methods at the time of
measurement): `tools/list` at 6 tools / 4,716 bytes (~1,179 tokens); average saving of ~39,892
tokens per structural query on the 5 sampled concepts, pushing break-even to ~1 structural query.
That average is inflated by two common method names (`insert`/`save`) picking up unrelated files in
the naive comparison — the report says so in its own output; this repository's own `cmd_generate`
(the README's hand-picked example) gives a tighter, still clearly positive comparison.

Run against three real external projects, one per language (fresh clones, not synthetic fixtures):

| Project | Language | Size | MCP registration | Avg. tokens saved/query | Break-even |
|---|---|---|---|---|---|
| [ripgrep](https://github.com/BurntSushi/ripgrep) | Rust | 110 files / 56K LOC | 6 tools / ~1,275 tokens | ~3,100 (two of five samples were common names like `insert`/`has_command`) | ~1 query |
| [requests](https://github.com/psf/requests) | Python | 37 files / 12K LOC | 6 tools / ~1,275 tokens | — | ~1 query |
| [cobra](https://github.com/spf13/cobra) | Go | 36 files / 17K LOC | 6 tools / ~1,275 tokens | ~21,000 | ~1 query |

MCP registration cost held steady at 6 tools / ~1,275 tokens across all three (bundle-independent,
as expected — it's a function of the tool schemas, not the project). See
[`ROADMAP.md`](../../ROADMAP.md#improvement-plan--ai-native-platform-maturity-community-feedback)
for the full verification notes, including the "not measured" gaps (memory usage, RAG comparison,
AI task-completion time) named honestly rather than fabricated.
