# Roadmap

This roadmap tracks delivery against the plan in [`docs/specification.md`](docs/specification.md#roadmap). Each phase builds strictly on the previous one's output — a feature is only listed once, in the phase where it first ships, and later phases assume everything in earlier phases already works.

## Status at a glance

| Phase | Status |
|---|---|
| Phase 1 — Foundations | ✅ Complete |
| Phase 2 — Depth & Integration | ⬜ Not started |
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

## Phase 2 — Depth & Integration

- [ ] LSP integration (`okf-lsp`) to enrich/disambiguate symbols beyond what Tree-sitter alone can resolve
- [ ] Incremental indexing: content-hash-based re-analysis of only changed files
- [ ] Extended language coverage: Java, Kotlin, C#, C/C++, PHP, Swift
- [ ] Multi-package workspace and monorepo aggregation
- [ ] Graph queries (`okf-graph`): cross-module links, ownership, API surface, cycle/dependency analysis
- [ ] Relationship-aware search: extend Phase 1 search with "Relationship" and "API" queries backed by `okf-graph`
- [ ] Bundle diffing (`okf-rs diff`)
- [ ] MCP server (`okf-mcp`) exposing symbol, call-graph, and API-surface queries
- [ ] Agent entry-point generation: `CLAUDE.md`, `AGENTS.md`, `.github/copilot-instructions.md`
- [ ] Basic documentation generation (Markdown, HTML) templated directly from the bundle — no LLM required
- [ ] Continuous indexing in local development (`okf-watch`)

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
