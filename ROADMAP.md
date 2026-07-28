# Roadmap

This roadmap tracks delivery against the plan in [`docs/specification.md`](docs/specification.md#roadmap). Each phase builds strictly on the previous one's output — a feature is only listed once, in the phase where it first ships, and later phases assume everything in earlier phases already works.

## Status at a glance

| Phase | Status |
|---|---|
| Phase 1 — Foundations | ✅ Complete |
| Phase 2 — Depth & Integration | 🟡 In progress (4/11) |
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
- [ ] Incremental indexing: content-hash-based re-analysis of only changed files
- [ ] Extended language coverage: Java, Kotlin, C#, C/C++, PHP, Swift
- [ ] Multi-package workspace and monorepo aggregation
- [ ] MCP server (`okf-mcp`) exposing symbol, call-graph, and API-surface queries
- [ ] Basic documentation generation (Markdown, HTML) templated directly from the bundle — no LLM required
- [ ] Continuous indexing in local development (`okf-watch`)

Verified by dogfooding: `okf-rs graph api .` on this repository lists 71 public concepts, `okf-rs graph cycles .` correctly finds none, `okf-rs graph modules .` shows real cross-crate dependency edges, and `okf-rs diff <commit> <commit> .` against this repo's own history correctly reports added/changed functions using non-destructive worktrees (verified the working tree and branch were untouched afterward, including on an invalid-ref error path).

Relationships (`Calls`/`CalledBy`/`Imports`/...) are now serialized into each concept's `relationships` frontmatter field (target ids grouped by kind), alongside the existing human-readable "# Calls" / "# Called by" markdown body sections. `okf_parser::read_bundle` reverses `okf-generator`'s writer to reconstruct the full relationship-rich concept model from a bundle on disk, so `okf-rs graph` now queries a previously generated bundle directly — the same way `okf-rs search` and `okf-rs validate` do — instead of re-analyzing the project from source; run `okf-rs generate` first (and after any source change, to keep the bundle current).

**Known limitations:**
- Public/private (`is_public`) detection is exact for Rust (`pub` modifier) and Go (capitalization, the language's real rule), but a naming-convention heuristic (leading underscore) for Python/JavaScript/TypeScript, pending real `export`/access-modifier tracking.

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
