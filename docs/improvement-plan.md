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

**Update, August 2026:** the "15 granular tools stay" framing below (§2, §3, and the GO/no-go
table) was the reasoning at the time this document was written. Further external review argued
the granular `graph_*` surface was itself a cost worth fixing, not just working around with an
additive composite tool — see the "Optimize the MCP API" item in
[`ROADMAP.md`](../ROADMAP.md#improvement-plan--ai-native-platform-maturity-community-feedback),
now shipped: the 13 `graph_*` tools collapsed into one `graph(relation=...)` tool, taking
`okf-mcp`'s registered tool count from 18 down to 6. The composite `okf_explore` item below (§4,
delivered as `explore`) is unaffected and still addresses the separate "single concept, several
facts" case this doesn't.

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

## 6. Cost/benefit study and go/no-go

Effort is sized against this codebase specifically (existing crates, existing test/CI
harness), not in the abstract: **S** = a few focused hours to a day, well-contained in one
crate; **M** = multi-day, touches 2-3 crates and their tests; **L** = a new crate or a genuinely
new capability with real design risk; **XL** = server/infra-scale, comparable to `okf-server`
itself. Benefit is scored against how directly the item closes one of §3's three named gaps
(impact analysis, single-call MCP query, query performance at scale) versus how speculative or
"nice-to-have" it is. Risk covers both regression risk to the deterministic core and
maintenance/scope-creep risk.

| # | Item | Effort | Benefit | Risk | Decision |
|---|---|---|---|---|---|
| 1 | Parallelize extraction (`rayon`) | S | Medium — real, but this repo's own 700-concept bundle already generates fast; benefit is proportional to repo size, unproven at the 10k+-file scale competitors benchmark | Low — per-file extraction is already independent; the deterministic-output tests already guard against order-dependent bugs | **GO** |
| 2 | Composite MCP tool (`okf_explore`) | S | High — directly answers the round-trip cost named in §3.2, and is pure composition over functions that already exist and are already tested | Low — additive; the 15 granular tools stay, nothing existing changes shape | **GO** |
| 3 | Change-impact analysis (`okf-rs impact`) | M | High — the single biggest named gap (§3.1); reuses existing worktree diffing and call-graph traversal, no new dependency | Medium — transitive closure over a graph that can contain cycles needs to reuse the existing `cycles()`-safe traversal correctly, or it can loop or double-count; needs real test coverage on a cyclic fixture, not just an acyclic one | **GO** |
| 4 | Community detection for `okf_arch::domains` (Louvain) | M | Medium — fixes a documented, real known limitation, but only changes behavior on large/dense graphs; small projects (most of this tool's own dogfooding) see no visible difference | Medium-High — a from-scratch modularity-optimization implementation is genuinely easy to get subtly wrong, and naive Louvain has randomized tie-breaking that would quietly break the "deterministic core" principle if not deliberately pinned (fixed iteration order, no RNG, or an explicit seed) | **CONDITIONAL GO** — only with a deterministic (seeded or order-stable) implementation and a test asserting byte-identical output across repeated runs on the same input, matching the bar every other `okf-rs` analysis already meets; ship §3's cheaper items first and revisit this with real large-graph test data, not just this repo's own bundle |
| 5 | Deterministic viz export: GraphML + Obsidian vault | S-M | Medium — genuine, low-cost visualization story using free existing tools (Gephi/yEd/Obsidian), same "hand-templated, no new heavy dependency" pattern as the DITA/PDF exporters | Low — output-only, read-only over the existing `Graph`, same shape as `okf-docs`'s existing exporters | **GO** |
| 5b | Deterministic viz export: SVG (force-directed static render) | M-L | Low-Medium — a static force-directed layout is a real graph-drawing problem (node overlap, edge crossing minimization), not a templating exercise like GraphML/Obsidian, and the eventual interactive explorer over `okf-server` (Phase 4) is the actual right answer for exploration, not a static image | Medium — either a hand-rolled layout algorithm (real effort, mediocre result) or a new layout-crate dependency, for a deliverable the Phase 4 item will likely obsolete | **NO-GO** — defer to the Phase 4 interactive explorer; GraphML/Obsidian already cover "open it in a real graph tool" at a fraction of the cost |
| 6 | Persisted/derived query cache (Tantivy index on disk, not in-memory rebuild) | M | Unproven — real only if query latency is actually a measured problem; this repo's own ~700-concept bundle gives no evidence either way, and CLI/MCP calls are typically one-shot per invocation, not a hot loop, so an in-memory rebuild's cost is paid once per process, not per query | Medium — cache-invalidation bugs are a classic source of "stale answer silently served," which directly undercuts the trust story `--ci` validation and the OKF `stale_after` fields exist to protect; adds a second cache format to keep consistent alongside `.okf-cache.json` | **NO-GO for now** — benchmark first: add a `hyperfine`/criterion-style benchmark of `search`/`graph` commands against a synthetic 10k-concept bundle before building this. Revisit as a **GO** only if that benchmark shows the in-memory rebuild is actually the bottleneck, not a hypothetical one |
| 7 | PR review automation (`okf-rs review` + GitHub Action) | L | High — directly matches code-review-graph's flagship, most-marketable use case; strong adoption/differentiation value once shippable | Medium-High — a GitHub Action is an ongoing maintenance surface (auth/token handling, sticky-comment update logic, API version drift) distinct from a pure CLI feature, and it's only as good as the impact analysis (#3) it's built on | **CONDITIONAL GO** — sequence strictly after #3 ships and is dogfooded on a few real PRs via the plain CLI report first; don't build the Action until the underlying report is already trustworthy standalone |
| 8a | Optional semantic search via an existing OpenAI-compatible embeddings endpoint (reusing the `--enrich-base-url` pattern) | S-M | Medium — extends a pattern (`okf-enrich`'s pluggable, no-hard-vendor-dependency HTTP client) that already exists and is already tested; genuinely additive to Tantivy ranked search for "find by meaning, not wording" queries | Low — no new runtime dependency (`ureq` already there), opt-in, same "never a hard dependency on one vendor" posture as `--enrich` today | **GO** |
| 8b | Bundled/local embedding **model runtime** (e.g. sentence-transformers via ONNX, shipped in-process) | L | Medium — matches code-review-graph's local-first default, but Tantivy ranked search already covers most of the practical "find by meaning" need day to day | High — directly contradicts the "standalone binary, no runtime dependency beyond the OS's standard C library" packaging property this project explicitly verifies (§ Packaging & Distribution in `docs/specification.md`); bundling a model runtime or weights is a real regression on binary size, build complexity, and the "no proprietary runtime required" openness claim this project uses to differentiate itself from Graphify | **NO-GO** — 8a already delivers the actual capability (semantic search) without the packaging regression; a bundled local model runtime is the wrong trade for this project's own stated principles |
| 9 | Multi-repo / daemon indexing (`okf-server` scope) | XL | High long-term, but already correctly scoped as Phase 4 and not something either competitor makes look urgent to pull forward — both are single-project-first tools too, with multi-repo as an add-on (`register`/`unregister`), not their core value prop | High — full server/auth/lifecycle surface, the largest single item in this entire plan | **NO-GO (no change)** — confirm existing Phase 4 placement is correct; nothing in this comparison argues for accelerating it |

### Net recommendation

Ship, in order: **#1 → #2 → #3 → #5 → #8a**, all **GO** with no unresolved conditions — this
sequence alone closes both concretely-scored-High gaps (impact analysis, single-call MCP query)
plus the parallelism gap, at a combined cost of roughly S+S+M+S+S-M, before touching anything
conditional. **#7** (PR review automation) is the highest-value remaining item but is
deliberately sequenced *after* #3, not parallel to it — building the GitHub Action against an
impact report that hasn't been validated standalone risks shipping automation on top of an
unproven signal. **#4** (community detection) is worth doing but is not blocking anything else
and carries a real determinism risk if rushed, so it's explicitly not in the critical path.
**#5b**, **#6**, **#8b**, and **#9** are deliberate no-gos for now, each for a different reason
(wrong tool for the job, unproven need, contradicts a stated project principle, and
correctly-already-deferred scope, respectively) — not oversights.

## References

- [CodeGraph](https://github.com/colbymchenry/codegraph)
- [code-review-graph](https://github.com/tirth8205/code-review-graph)
- [`docs/specification.md` — Comparison with Other Tools](specification.md#comparison-with-other-tools) (okf-generator, Graphify)
- [`ROADMAP.md`](../ROADMAP.md)
