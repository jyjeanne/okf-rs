# Improvement plan: okf-rs vs. CodeGraph and code-review-graph

This document compares `okf-rs` against two other codebase-to-knowledge-graph tools —
[CodeGraph](https://github.com/colbymchenry/codegraph) (colbymchenry) and
[code-review-graph](https://github.com/tirth8205/code-review-graph) (tirth8205) — and turns the gap
analysis into a prioritized improvement plan. It complements, rather than duplicates,
[`docs/specification.md`'s "Comparison with Other Tools"](specification.md#comparison-with-other-tools)
section, which covers `okf-generator` and Graphify; this document covers the other two tools an
issue asked to be analyzed, plus concrete next steps for `okf-rs` itself.

Based on public information about both projects as of July 2026 (READMEs, documented CLI/MCP
surfaces, published benchmarks); figures from each project's own marketing are treated as
directional, not independently verified.

## 1. What the two projects are

### CodeGraph

A TypeScript/JavaScript CLI with a native Rust extraction kernel (tree-sitter, 20 languages,
17 web-framework routing heuristics), persisting to a local **SQLite database with FTS5**
(`.codegraph/codegraph.db`). Its core pitch is "one-call context": a single MCP tool,
`codegraph_explore`, returns verbatim source, call flow, and blast radius for a symbol in one
round trip, instead of an agent grep-and-read looping over files. It watches the filesystem
(FSEvents/inotify/ReadDirectoryChangesW), debounces changes, and marks responses with a
staleness banner while a re-sync is pending. Published benchmarks claim 60–69% fewer tokens and
89% fewer tool calls than file-crawling, and indexing the Linux kernel (70k files) in under 12
minutes on a 2-core VPS.

### code-review-graph

A Python 3.10+ MCP server, also tree-sitter + **SQLite**-backed, purpose-built for *PR review*:
it computes "blast radius" (which files/functions actually depend on changed code) and feeds a
review model only that minimal context. It ships a GitHub Action that posts risk-scored, sticky
PR comments with `fail-on-risk` merge gating, a 30-tool MCP surface, Leiden community detection
for domain clustering (with recursive splitting of oversized clusters), optional local or cloud
embeddings layered on FTS5, and multiple export formats (D3.js, GraphML, Neo4j Cypher, Obsidian
vaults, SVG, JSON). Reports a median 82x token reduction across six benchmarked repositories.

### Shared philosophy, and where it diverges from okf-rs

Both tools converge on the same thesis `okf-rs` also holds — pre-index the codebase once,
answer agent queries from that index instead of re-reading source — but both implement it as a
**local database plus a bespoke query/MCP layer**, not as **the artifact itself being the
knowledge base**. `okf-rs`'s bundle is plain markdown+YAML that any tool can read without going
through `okf-rs`; CodeGraph's and code-review-graph's SQLite databases are implementation
details of their own CLI/MCP server. That's `okf-rs`'s real differentiator (same as the existing
comparison against `okf-generator` and Graphify) — but on the concrete, day-to-day capabilities
that make these tools valuable in an agentic workflow, `okf-rs` is currently behind both.

## 2. Feature gap matrix

| Capability | okf-rs (today) | CodeGraph | code-review-graph |
|---|---|---|---|
| Blast-radius / change-impact analysis | Not present — `diff` reports concept-level added/removed/changed only, with no transitive downstream-callers step | Yes — core primitive of `codegraph_explore` | Yes — core primitive, drives PR review |
| PR / code-review automation | Not present | Not present | Yes — GitHub Action, sticky risk-scored comments, `fail-on-risk` gating |
| MCP tool shape | 15 granular tools (`search`, `graph_callers`, `graph_callees`, `graph_api`, ...) — an agent composes several calls to answer one question | 1 primary tool (`codegraph_explore`) bundling source + call flow + blast radius per call | 30 tools, but with a small number of composite/workflow prompts (review, architecture, debug, onboard, pre-merge) |
| Query performance at scale | Every CLI/MCP call re-reads the bundle from disk and rebuilds an in-memory Tantivy index from scratch (`okf_parser::read_bundle` + `FullTextIndex::build` per invocation) | Persistent SQLite+FTS5, updated incrementally | Persistent SQLite+FTS5, updated incrementally |
| Parallel extraction | None found — no `rayon`, no threads, no async anywhere in the workspace, despite "Parallel" being a stated design principle in `docs/specification.md` | Rust kernel sizes worker pools to core count (container-aware) | Not documented in detail, but SHA-256 diff-based incremental updates keep re-analysis small |
| File watching | Yes (`okf-rs watch`), single project, in-process | Yes, native OS file events, debounced, with staleness banners on stale-read | Yes (`watch`/`daemon`), plus multi-repo `register`/`unregister` |
| Multi-repo / organization scale | Not shipped (`okf-server`, Phase 4, not started) | Not a stated focus (single-project tool) | Yes — `register`/`unregister`, background daemon |
| Domain/community detection | Connected components of the undirected package graph (`okf_arch::domains`) — a documented known limitation: on this repo's own bundle it collapses nearly everything into one component because of call-graph noise | N/A (no domain-clustering feature) | Leiden modularity-based community detection, with recursive splitting of oversized clusters |
| Visualization | Not shipped (`okf-server`, Phase 4, not started) | Not a core focus | Interactive D3.js, GraphML, Cypher, Obsidian, SVG, JSON exports |
| Semantic/embedding search | Not shipped — only exact/substring (`okf-search`) and ranked full-text (`okf-search` via Tantivy) | Not documented | Optional, pluggable embeddings (sentence-transformers local, or Gemini/Voyage/OpenAI-compatible cloud) layered on FTS5 |
| Determinism / provenance | Deterministic core; OKF v0.2 `generated`/`verified`/`status`/`stale_after` trust fields | Not applicable (proprietary DB, not a spec'd interchange format) | Relationships aren't confidence-tagged in public docs the way Graphify's are, but the graph is a private DB, not an inspectable/diffable artifact |
| Output artifact | Plain markdown + YAML, git-diffable, renders on GitHub, readable without the tool | SQLite DB (`.codegraph/codegraph.db`), not meant to be read directly | SQLite DB, with optional exports (GraphML/Obsidian/etc.) as a secondary, generated artifact |
| Standalone binary, no runtime | Yes — links only libc | Bundles its own Node.js runtime | Requires Python 3.10+ |

## 3. What this means, concretely

`okf-rs` wins decisively on **openness, determinism, and packaging** — nothing here changes that
assessment, and it's the correct place to keep investing. But three gaps above are not
"different design philosophy," they're capability gaps that would matter to any agent or
developer choosing between these tools day to day:

1. **No impact analysis.** `okf-rs diff` answers "what changed" at the concept level; neither it
   nor `okf-rs graph` answers "what does this change actually put at risk downstream" — the
   single feature both competing tools lead with.
2. **No single-call, agent-optimized query.** `okf-mcp`'s 15 granular tools mean an agent asking
   "what does this function do and who's affected if I change it" pays for 3-4 round trips
   (`search` → `graph_callers` → `graph_callees` → re-reading source) where CodeGraph answers in
   one. This is a real, measurable token/latency cost in exactly the scenario `okf-rs`'s own
   README uses to justify the MCP server's existence.
3. **Query performance doesn't scale with repo size the way a persistent index does.** Rebuilding
   a Tantivy index from a full bundle re-read on every single CLI/MCP call is fine at hundreds of
   concepts (this repo's own size) and will not stay fine at the tens-of-thousands-of-file scale
   CodeGraph and code-review-graph explicitly benchmark against.

The remaining gaps (visualization, multi-repo serving, community detection, embeddings) are
already named as future work in `ROADMAP.md`/`docs/specification.md` Phase 4, or map cleanly onto
it — this plan mostly reprioritizes them in light of what competitors have already shipped,
rather than inventing new scope.

## 4. Improvement plan

Ordered by leverage: cheapest to build on the existing architecture, and closest to what already
differentiates `okf-rs`, first.

### Near term (extends existing crates, no new runtime dependency)

- **Parallelize extraction with `rayon`.** `okf_core::Project::load`'s file walk and
  `okf-tree-sitter`'s per-file `extract()` are natural data-parallel workloads (independent files
  in, independent `FileExtraction`s out, merged before call-graph resolution). This is the
  single highest-leverage, lowest-risk change: it makes "Parallel" (already a stated design
  principle) true, and it's the most direct answer to CodeGraph's core-count-aware worker pools
  and both tools' scale benchmarks. Bundle output is unaffected (extraction per file is already
  independent of file order; the deterministic-output tests already in place are the regression
  guard).
- **A single composite MCP tool, `okf_explore`.** Add one new `okf-query` function — and its
  `okf-mcp` tool — that takes a concept id (or a search query which is resolved to the top hit)
  and returns, in one response: signature, description, immediate callers, immediate callees,
  and (once available) blast radius. This is pure composition over functions `okf-query` already
  exposes (`search`, `graph_callers`, `graph_callees`, the impact analysis below) — no new
  analysis, just one aggregated response shape, directly addressing CodeGraph's
  measured round-trip savings. The existing 15 granular tools stay; this is additive, matching
  code-review-graph's own pattern of composite "workflow" tools layered over granular ones.
- **Fix `okf_arch::domains` with real community detection.** The connected-components collapse is
  already a documented known limitation in `ROADMAP.md` ("nearly every package collapsed into one
  big layer-0 domain"). A modularity-based clustering step (Louvain is a good fit: simpler than
  Leiden to implement from scratch, no new dependency needed, and addresses the same weighted-edge
  problem) run on top of the existing package-dependency graph would produce genuinely useful
  domain boundaries even when the graph is densely connected, rather than requiring `--lsp` to
  first eliminate call-graph noise as the current fallback plan assumes.
- **Deterministic visualization exports as a stepping stone before `okf-server`.** GraphML and
  Obsidian-vault export are, like `okf-docs --format dita/pdf`, hand-templatable from the existing
  concept graph with no new heavy dependency and no server — a `Graph` walk emitting XML or
  wikilinks. This gets a real visualization story (Gephi/yEd/Obsidian are widely available free
  tools) shipped well before the "interactive graph explorer over `okf-server`" Phase 4 item,
  the same way PDF/DITA export already beat a hypothetical full doc-site generator to market.

### Mid term (new analysis, still deterministic and offline)

- **Change-impact analysis** (see the flagship proposal in §5) — extends `okf-core`'s existing
  git-worktree diffing and `okf-graph`'s existing call-graph traversal; no new dependency.
- **A local, derived query cache — not a new source of truth.** `okf-rs generate` already
  persists `.okf-cache.json` as a disposable, regenerable performance cache keyed by content hash;
  the same pattern extends naturally to a query-side cache (e.g., a persisted Tantivy index
  directory instead of an in-memory rebuild per invocation, invalidated the same way the
  extraction cache already is). This closes the query-performance gap versus CodeGraph/code-review-graph's
  SQLite backends without adopting a database as the bundle format — the markdown+YAML bundle
  remains the only artifact a consumer ever needs to read; the cache is purely an accelerator
  `okf-rs` itself may delete and rebuild at any time.

### Longer term (larger scope, matches existing Phase 4 roadmap)

- **PR review automation** (see §5) — builds on change-impact analysis above.
- **Optional local-first embeddings for semantic search**, pluggable the same way `--enrich`
  already is (OpenAI-compatible endpoint, no hard vendor dependency), layered on top of the
  existing Tantivy ranked search rather than replacing it — matching code-review-graph's
  "local-first, cloud optional" posture instead of requiring a specific provider.
- **Multi-repo, daemon-based indexing** — already scoped as `okf-server`'s "multi-repository,
  organization-wide serving" in Phase 4; this plan doesn't change that scope, just confirms both
  competitors validate it as real demand.

## 5. Best concept: an impact-first knowledge base, without giving up "just files"

The single idea most worth building, because it closes the two most concretely valuable gaps
(impact analysis, single-call agent queries) with the least architectural risk, and does it in a
way neither competitor can — entirely from the bundle's existing plain-file artifacts, with no
database, daemon, or hosted component required:

**`okf-rs impact <ref-a> <ref-b>` (or `--from`/`--to`, defaulting to working tree vs. `HEAD`)**

1. Reuse `okf-core`'s existing non-destructive git-worktree diffing (already built for
   `okf-rs diff`) to get the set of concepts added/removed/changed between two refs.
2. For each changed/removed concept, walk `okf-graph`'s existing `Calls`/`CalledBy` edges
   transitively (bounded by depth, same shape `graph path` already traverses) to compute the set
   of concepts downstream of the change — the blast radius.
3. Score each affected concept by a **deterministic, structural criticality signal** computed
   from data the graph already has — in-degree (how many other concepts call it), whether it's
   part of the public API (`graph api` already computes this), and whether it sits in a call-graph
   cycle (`graph cycles` already computes this) — rather than an LLM judgment call. This keeps the
   feature inside `okf-rs`'s existing "deterministic core, AI layered on top as optional" design
   principle instead of importing code-review-graph's model-dependent risk scoring wholesale.
4. Render the result as: (a) human/CI-readable text or JSON from the CLI, (b) a new
   `graph_impact` MCP tool composed into `okf_explore` (§4) so an agent gets blast radius for
   free alongside signature/callers/callees in one call, and (c) an `okf-rs review` subcommand
   that formats the same report as a PR comment body and a GitHub Action that posts/updates it —
   `code-review-graph`'s flagship use case, minus the SQLite dependency, minus the hosted/daemon
   requirement, minus a model in the loop for the core risk signal (an optional `--enrich` pass
   can still add a prose summary on top, exactly like `okf-rs generate --enrich` already layers
   LLM enrichment onto deterministic extraction).

Why this is the right "best concept" rather than just the top item in a backlog: it's the one
place where all three of this plan's top findings — no impact analysis, no single-call MCP
query, and the "bundle is a plain, inspectable file, not a private database" differentiator — 
compose into one feature instead of trading off against each other. The impact report is not a
new opaque store: it's computed on demand from the same markdown+YAML bundle every other
`okf-rs` command already reads, so it inherits every existing guarantee (deterministic, git-diffable
inputs, no proprietary runtime) while directly matching the concrete, benchmarked value
proposition (blast-radius-scoped context, PR-automation) that's currently `okf-rs`'s biggest
competitive gap against both CodeGraph and code-review-graph.

## References

- [CodeGraph](https://github.com/colbymchenry/codegraph)
- [code-review-graph](https://github.com/tirth8205/code-review-graph)
- [`docs/specification.md` — Comparison with Other Tools](specification.md#comparison-with-other-tools) (okf-generator, Graphify)
- [`ROADMAP.md`](../ROADMAP.md)
