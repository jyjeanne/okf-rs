# Roadmap

This roadmap tracks delivery against the plan in [`docs/specification.md`](docs/specification.md#roadmap). Each phase builds strictly on the previous one's output — a feature is only listed once, in the phase where it first ships, and later phases assume everything in earlier phases already works.

## Status at a glance

| Phase | Status |
|---|---|
| Phase 1 — Foundations | ✅ Complete |
| Phase 2 — Depth & Integration | 🟡 In progress (7/11) |
| Phase 3 — Intelligence & Extended Output | ⬜ Not started |
| Phase 4 — Ecosystem | ⬜ Not started |

---

## Phase 1 — Foundations ✅

- [x] `okf-cli` skeleton (`init`, `scan`, `generate`, `validate`, `search`)
- [x] Repository scanning: recursive walk, `.gitignore` handling, git-aware indexing, single-package workspace support
- [x] Tree-sitter parsing and core symbol extraction (packages, modules, types, functions) for the initial language set: **Rust, Python, TypeScript/JavaScript, Go**
- [x] Direct relationship extraction (imports, call graph) for the initial language set
- [x] OKF bundle generation (`okf-generator`): markdown + YAML frontmatter, cross-linked via markdown links
- [x] Validator (`okf-validator`): schema conformance, frontmatter validity, link integrity, orphan detection, duplicate-identity checks
- [x] Basic search (`okf-search`): by symbol, package, module, type, and tag (no relationship queries yet)
- [x] Packaged as a standalone CLI binary with a cross-platform release workflow

Verified by dogfooding: running `okf-rs generate .` on this repository itself produces a full, deterministic OKF bundle (identical output across repeated runs) that `okf-rs validate` reports as clean, with a real resolved call graph across `self.method()`, `Type::method()`, and bare-identifier calls.

**Known limitations carried into Phase 2 by design, not oversight:**
- Call-graph resolution only resolves a call when its callee name is *unambiguous* project-wide (exactly one function/method with that name). Precise, type-aware resolution needs the LSP integration planned for Phase 2.
- Single-package projects only — multi-package workspace/monorepo aggregation lands in Phase 2.
- No schema-version field is emitted yet, so the validator's schema-version conformance check (named in the spec) has nothing to check against until that's introduced.

---

## Phase 2 — Depth & Integration 🟡

- [x] Graph queries (`okf-graph`): cross-module links, ownership, public API surface, and cycle/dependency analysis (Tarjan's SCC), plus shortest call-path — exposed via `okf-rs graph {callers,callees,cycles,api,modules,path}`
- [x] Relationship-aware queries backed by `okf-graph` — delivered as the `okf-rs graph` subcommand family above rather than folded into `okf-rs search` itself, since relationship queries (traversal, cycles) are a different shape than free-text search
- [x] Bundle diffing (`okf-rs diff <ref-a> <ref-b> [path]`) — compares two git refs' concepts (added/removed/changed) using a non-destructive `git worktree` checkout of each ref, so it never touches the caller's working tree
- [x] Agent entry-point generation: `CLAUDE.md`, `AGENTS.md`, `.github/copilot-instructions.md`, written/updated by `okf-rs init` (skip with `--no-agent-files`); idempotent via marker comments, so pre-existing content in those files is preserved and only the okf-rs section is replaced on re-run
- [ ] LSP integration (`okf-lsp`) to enrich/disambiguate symbols beyond what Tree-sitter alone can resolve
- [x] Incremental indexing: content-hash-based re-analysis of only changed files — `okf-rs generate` persists a `.okf-cache.json` at the project root, keyed by each file's path and content hash (not mtime, which git checkouts/CI don't preserve reliably); a file whose hash still matches skips the tree-sitter parse entirely and reuses its cached extraction, while the project-wide call-graph resolution step still runs in full every time (it's cheap, and it's what keeps a changed file's *callers* correctly updated). `--no-cache` bypasses the cache entirely (rule out a stale cache, or verify output determinism independent of cache state)
- [ ] Extended language coverage: ~~Java~~, Kotlin, ~~C#~~, C/C++, PHP, Swift — **Java** shipped (classes, interfaces, enums, methods/constructors, `public`-modifier visibility, and a single `method_invocation` query pattern that captures bare/`this.`/object-receiver/static calls uniformly); **C#** shipped the same shape (classes, structs, interfaces, enums, methods/constructors, `public`-modifier visibility, `invocation_expression`-based call capture); Kotlin, C/C++, PHP, and Swift remain
- [ ] Multi-package workspace and monorepo aggregation
- [x] MCP server (`okf-mcp`) exposing symbol, call-graph, and API-surface queries — a stdio JSON-RPC 2.0 server implementing MCP's `initialize`/`tools/list`/`tools/call`, wrapping `okf-search` and `okf-graph` (via `okf_parser::read_bundle`) as the `search`, `graph_callers`, `graph_callees`, `graph_api`, `graph_cycles`, `graph_modules`, and `graph_path` tools
- [ ] Basic documentation generation (Markdown, HTML) templated directly from the bundle — no LLM required
- [x] Continuous indexing in local development (`okf-watch`) — `okf-rs watch` regenerates the bundle once immediately, then again each time a burst of filesystem activity (recursive, via `notify`) settles for a debounce period (`--debounce-ms`, default 300ms), reusing the exact same `.okf-cache.json` incremental cache `generate` does. Reports a regenerate only when something actually changed (a file was reparsed, or the concept id set differs from the last reported run) — a wakeup from unrelated churn under an ignored path (e.g. `target/` mid-`cargo build`) still costs a cheap `.gitignore`-aware re-scan and a harmless, byte-identical bundle rewrite, but isn't reported, so watch mode doesn't spam the terminal

Verified by dogfooding: `okf-rs graph api .` on this repository lists 71 public concepts, `okf-rs graph cycles .` correctly finds none, `okf-rs graph modules .` shows real cross-crate dependency edges, and `okf-rs diff <commit> <commit> .` against this repo's own history correctly reports added/changed functions using non-destructive worktrees (verified the working tree and branch were untouched afterward, including on an invalid-ref error path).

Relationships (`Calls`/`CalledBy`/`Imports`/...) are now serialized into each concept's `relationships` frontmatter field (target ids grouped by kind), alongside the existing human-readable "# Calls" / "# Called by" markdown body sections. `okf_parser::read_bundle` reverses `okf-generator`'s writer to reconstruct the full relationship-rich concept model from a bundle on disk, so `okf-rs graph` now queries a previously generated bundle directly — the same way `okf-rs search` and `okf-rs validate` do — instead of re-analyzing the project from source; run `okf-rs generate` first (and after any source change, to keep the bundle current).

`okf-mcp` reuses that same bundle-reading path: each tool call re-reads the bundle fresh via `okf_parser::read_bundle`/`okf_search::SearchIndex::build`, so a running server always reflects the latest `generate` without needing a restart. Verified end-to-end with a hand-fed `initialize` → `notifications/initialized` → `tools/list` → `tools/call` sequence over stdio against this repo's own bundle, and with unit tests covering the JSON-RPC dispatch (unknown methods, notifications never getting a response) and each tool (missing bundle, unknown concept id, missing arguments).

Verified by dogfooding: on this repository (22 source files), a cold `okf-rs generate .` reports "22 files parsed, 0 reused from cache"; re-running it immediately after with no source changes reports "0 files parsed, 22 reused from cache"; and the resulting bundle is byte-for-byte identical across the cold run, the warm (cached) run, and a `--no-cache` run (`diff -rq` on all three trees reports no differences) — confirming the cache changes performance, never output.

Java support verified end-to-end against a small standalone sample (a `package`-scoped class with a public/private method pair): `okf-rs scan` recognizes `.java` files, `okf-rs generate` extracts the class and both methods with a resolved `Calls` edge between them, `okf-rs validate` reports the bundle clean, `okf-rs search` finds the method by name, and `okf-rs graph callers` correctly reports the caller from the bundle's serialized relationships. C# support verified the same way against an equivalent `namespace`-scoped class.

`okf-rs watch` verified against a scratch project: the startup (baseline) run reports immediately; appending a function to a watched file reports a regenerate with the new concept count and `reparsed > 0`; a bare `touch` of the same file (mtime bumped, content byte-identical) triggers a wakeup but is correctly silent, since the content hash is unchanged. `okf-watch`'s own test suite drives the real filesystem watcher (not a mock) end-to-end, additionally covering that deleting a file is reported even though nothing needs reparsing (caught via the concept-id-set check, not the reparse count) and that a burst of rapid edits coalesces into a single regenerate.

**Known limitations:**
- Public/private (`is_public`) detection is exact for Rust (`pub` modifier), Go (capitalization, the language's real rule), Java, and C# (`public` modifier — C#'s `internal`/`protected internal`/`private protected` are all folded into "private" for now, since `is_public` is a boolean), but a naming-convention heuristic (leading underscore) for Python/JavaScript/TypeScript, pending real `export`/access-modifier tracking.
- Java's and C#'s per-file `Module` concept follows the same convention as every other language (one module per source file), not their actual package/namespace-spans-multiple-files model; a `package`/`namespace` declaration doesn't yet merge same-package/namespace files into one module concept.
- The incremental cache speeds up `okf-rs generate` only; `okf-rs diff` still re-analyzes both git-worktree checkouts from scratch (a shared, path-relocatable cache across worktrees is possible future work, since the cache key is a repo-relative path + content hash, not an absolute path).
- `okf-rs watch` watches the project root recursively at the OS level (via `notify`), not `.gitignore`-filtered at the watch itself — only the subsequent re-scan is `.gitignore`-aware. A very large, actively churning ignored directory (e.g. a `target/` mid-build) still triggers a debounce cycle and a cheap re-scan per burst, even though nothing gets reported; skipping the watch subscription for ignored paths entirely is possible future work.

## Phase 3 — Intelligence & Extended Output

- [ ] Optional AI enrichment: function summaries, module descriptions, architecture explanations, usage examples, glossary entries
- [ ] Architecture extraction: architectural layers, domain boundaries, design patterns
- [ ] REST endpoint, database model, and event-flow detection
- [ ] DITA export
- [ ] PDF export

## Phase 4 — Ecosystem

- [ ] IDE plugins (VS Code, JetBrains) consuming the bundle and `okf-mcp`
- [ ] Distributed knowledge server (`okf-server`): multi-repository, organization-wide serving
- [ ] Visualization: interactive graph explorer over `okf-server`
- [ ] Continuous/distributed indexing at organization scale (beyond the local `okf-watch` from Phase 2)

---

See [`docs/specification.md`](docs/specification.md) for the full project specification, including the OKF bundle format, architecture, and design principles this roadmap implements against.
