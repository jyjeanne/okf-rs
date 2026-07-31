# okf-rs

[![CI](https://github.com/jyjeanne/okf-rs/actions/workflows/ci.yml/badge.svg)](https://github.com/jyjeanne/okf-rs/actions/workflows/ci.yml)
[![Release](https://github.com/jyjeanne/okf-rs/actions/workflows/release.yml/badge.svg)](https://github.com/jyjeanne/okf-rs/actions/workflows/release.yml)
[![License: MIT OR Apache-2.0](https://img.shields.io/badge/license-MIT%20OR%20Apache--2.0-blue.svg)](#license)

**A fast, open-source Rust CLI that turns a codebase into a portable [Open Knowledge Format](https://cloud.google.com/blog/products/data-analytics/how-the-open-knowledge-format-can-improve-data-sharing) (OKF) knowledge base — plain markdown files with YAML frontmatter, cross-linked into a real call graph, readable by humans and AI coding agents alike.**

```
$ okf-rs generate .
Generated 146 concepts into knowledge
  Module       16
  Struct       18
  Enum         6
  Function     87
  Method       19

$ okf-rs validate
knowledge — no issues found
```

## Why okf-rs?

Most codebase-analysis tools produce a proprietary graph database, an AI-specific context blob, or a pile of Markdown summaries you can't query or diff. `okf-rs` instead emits a **conformant OKF bundle**: ordinary `.md` files with YAML frontmatter, cross-linked by ordinary markdown links, that live in your repo like any other file — git-diffable, greppable, renderable on GitHub, and readable by any tool without going through `okf-rs` itself.

- **Open** — the output is the artifact; no proprietary runtime, database, or SDK required to read or write it.
- **Fast** — a native Rust core using [tree-sitter](https://tree-sitter.github.io/tree-sitter/) for parsing.
- **Deterministic** — identical source always produces byte-identical output; no wall-clock timestamps, no unordered maps leaking into results.
- **AI-ready** — structured knowledge that doesn't require an LLM to produce, though one can optionally enrich it later.

See [`docs/specification.md`](docs/specification.md) for the full project specification, including how `okf-rs` compares to other tools in this space, and [`ROADMAP.md`](ROADMAP.md) for what's shipped and what's next.

## Features

- **Repository scanning** — recursive, `.gitignore`-aware, with git-aware indexing and manifest detection (`Cargo.toml`, `package.json`, `pyproject.toml`, `go.mod`); a Cargo/npm/monorepo workspace with several member packages is aggregated into one bundle, with one `Package` concept per member correctly linked to its modules
- **Semantic extraction** for **Rust, Python, TypeScript, JavaScript, Go, Java, C#, PHP, Kotlin, C/C++, and Swift** — packages, modules, types (structs/classes/enums/interfaces/traits), functions, and methods, including public/private API-surface detection tailored to each language's actual visibility rules (explicit-opt-in for Rust/Java/C#/Swift, opt-out-by-default for PHP/Kotlin, section-based for C++, capitalization for Go)
- **Relationship extraction** — imports, and a resolved call graph covering bare calls, member/`self`/`this` calls, static/scoped calls, and qualified module calls across all eleven languages
- **LSP-backed disambiguation** (optional) — `okf-rs generate --lsp` resolves calls whose name is ambiguous project-wide by asking the project's real language server (`textDocument/definition`), on top of Tree-sitter's own unambiguous-name-only resolution; verified end to end against real `rust-analyzer`/`pyright` servers, with a timeout on unresponsive servers, Windows-aware executable lookup, and percent-encoded `file://` URIs so paths with spaces or non-ASCII characters resolve correctly
- **OKF bundle generation** — markdown + YAML frontmatter, cross-linked, with `index.md` navigation at every level; targets [OKF v0.2](https://github.com/GoogleCloudPlatform/knowledge-catalog/blob/main/okf/SPEC.md), declaring `okf_version: "0.2"` in the bundle root and filling in each concept's `generated.by` trust field
- **Incremental indexing** — `okf-rs generate` caches each file's extraction by content hash, so a re-run only re-parses what actually changed
- **Watch mode** — `okf-rs watch` keeps a project's bundle up to date as files change, reusing the same incremental cache
- **Validation** — frontmatter/schema checks (including the v0.2 `generated`/`verified`/`sources`/`status`/`stale_after` trust families and the root `okf_version` declaration), dangling-link detection, redundant-link detection, orphan detection, duplicate-identity checks (both path collisions and same-symbol-different-file), relationship-target resolution, and bundle-wide `Calls`/`CalledBy` symmetry checking
- **Search** — ranked free-text search by symbol, package, module, type, and tag
- **Graph queries** — callers/callees, call-graph cycle detection, isolated-concept detection, cross-module dependencies, shortest call path, and topology statistics (per-kind counts, relationship edge counts, connected components)
- **Coverage metrics** — `okf-rs coverage` reports description/tag completeness and call-graph participation across the bundle
- **Bundle diffing** — compare a project's concepts between two git refs without touching your working tree
- **Documentation generation** — `okf-rs docs` renders a bundle into a browsable static HTML site, a single consolidated Markdown document, or a single paginated PDF, templated directly from the concept data — no LLM required
- **AI agent integration** — `okf-rs init` writes/updates `CLAUDE.md`, `AGENTS.md`, and `.github/copilot-instructions.md`, idempotently; `AGENTS.md` holds the generated content and `CLAUDE.md` just imports it (`@AGENTS.md`), so tools that prefer `AGENTS.md` over `CLAUDE.md` (e.g. opencode) never end up reading stale or missing content; `okf-mcp` exposes search and graph queries as an MCP server for tools like Claude Code and opencode
- **Standalone binary** — no runtime dependency beyond the OS's standard C library; see [Packaging & Distribution](docs/specification.md#packaging--distribution)

See [`ROADMAP.md`](ROADMAP.md) for what's shipped (Phases 1 & 2 complete, Phase 3 in progress) and what's next.

## Installation

### From a release binary

Prebuilt binaries for Linux (glibc + static musl), macOS (x86_64 + arm64), and Windows are attached to each [GitHub Release](https://github.com/jyjeanne/okf-rs/releases). Download the archive for your platform, extract it, and put `okf-rs` on your `PATH`.

### With Cargo

```sh
cargo install --git https://github.com/jyjeanne/okf-rs okf-cli
```

### From source

```sh
git clone https://github.com/jyjeanne/okf-rs
cd okf-rs
cargo build --release
# binary at target/release/okf-rs
```

## Quick start

```sh
$ okf-rs scan .
Project root: /path/to/your/project
Manifest: Cargo
1 source files:
  Rust         1

$ okf-rs generate .
Generated 6 concepts into knowledge
  Package      1
  Module       1
  Struct       1
  Function     1
  Method       2

$ okf-rs validate
knowledge — no issues found

$ okf-rs search verify_token
 80  verify_token             Rust Method          functions/src/Auth/verify_token
```

`okf-rs init` records a project's default output directory in `okf.toml`, so later commands (`generate`, `validate`, `search`) don't need `--output`/`--project` repeated on every call.

### What comes out

A generated concept file — `knowledge/functions/src/Auth/verify_token.md` from the example above — looks like this:

```markdown
---
type: Rust Method
title: verify_token
resource: src/main.rs#L4-L6
generated:
  by: okf-rs/0.1.0
---

# Signature

`fn verify_token(&self, token: &str) -> bool`

# Calls

- [decode_jwt](../../../functions/src/Auth/decode_jwt.md)
```

Just a markdown file. Open it in any editor, render it on GitHub, or point an AI coding agent at the `knowledge/` directory and let it follow the links.

## Tutorial: adding okf-rs to an existing codebase

The quick start above uses a toy example. This walks through adopting `okf-rs` in a real, already-existing project — install once, then wire it into how you and your AI coding agent actually work day to day.

### 1. Install the binary

Pick one (see [Installation](#installation) for details):

```sh
# Prebuilt binary — download from https://github.com/jyjeanne/okf-rs/releases,
# extract, and put `okf-rs` on your PATH, or:
cargo install --git https://github.com/jyjeanne/okf-rs okf-cli
```

### 2. Initialize it in your project

From your project's root:

```sh
cd /path/to/your-existing-project
okf-rs init .
```

This writes `okf.toml` (recording `knowledge/` as the default bundle location, so later commands don't need `--output`/`--project` repeated), and creates or idempotently updates `CLAUDE.md`, `AGENTS.md`, and `.github/copilot-instructions.md` with a marked section pointing AI agents at the bundle — existing content in those files is preserved untouched. Skip the agent files with `okf-rs init . --no-agent-files` if you'd rather add that section yourself, or aren't using an AI agent.

### 3. Generate the bundle

```sh
okf-rs generate
```

This is safe to run on a large, real codebase: it's `.gitignore`-aware (never descends into `node_modules`, `target`, `vendor`, ...), aggregates a multi-package workspace/monorepo into one bundle automatically, and caches each file's extraction by content hash in `.okf-cache.json` — the first run parses everything, every run after that only re-parses files that actually changed. Add `.okf-cache.json` to `.gitignore`; it's a disposable local performance cache, not part of the bundle.

Decide whether `knowledge/` itself belongs in git. Both are reasonable: committing it means the bundle reviews alongside the code that produced it (and is diffable, per PR, in `git diff`); `.gitignore`-ing it means treating it as a build artifact regenerated in CI. Either way, run `okf-rs validate --ci` in CI (see step 6) so a stale or broken bundle never ships silently.

#### Optional: fill in missing descriptions with an LLM

```sh
okf-rs generate --enrich --enrich-base-url http://localhost:11434/v1 --enrich-model llama3.1
```

Entirely optional, and never a hard dependency on one vendor: `--enrich` speaks the same `chat/completions` shape every OpenAI-compatible endpoint implements, so it works unmodified against [Ollama](https://ollama.com), LM Studio, LocalAI, [Crustly](https://github.com/jyjeanne/crustly), or a cloud provider (pass `--enrich-api-key`, or set `OKF_ENRICH_API_KEY`) — a local server generally doesn't need one. Only concepts with no description are ever queried or overwritten: a hand-written one is left alone, and a previous `--enrich` run's output is reused straight from the bundle on disk rather than re-querying the endpoint on every `generate`.

Once descriptions exist, `okf-rs suggest-links` (same `--enrich-*` flags) looks for concepts that are semantically close by full-text search but have no relationship edge yet, and asks the endpoint whether each candidate looks like a genuinely missing link — advisory only, nothing is written back into the bundle:

```sh
okf-rs suggest-links --enrich-base-url http://localhost:11434/v1 --enrich-model llama3.1
```

### 4. Explore it

```sh
okf-rs search verify_token                              # find a symbol by name
okf-rs search "parses a jwt" --ranked                    # ranked, relevance-scored full-text search (title/type/description/signature/tags)
okf-rs graph callers functions/src/auth/verify_token     # who calls it
okf-rs graph api                                         # the whole public API surface
okf-rs graph layers                                      # each package's layer in the dependency graph (0 = foundational)
okf-rs graph domains                                     # clusters of packages that depend on each other
okf-rs graph patterns                                    # Builder/Singleton/Factory/Visitor matches, by structural heuristic
okf-rs graph features                                    # REST endpoints, database models, event-flow participants, by naming heuristic
okf-rs docs --format html                                # a browsable static site, into docs/
okf-rs docs --format pdf                                 # a single paginated PDF, into docs.pdf
```

Concept ids (like `functions/src/auth/verify_token` above) come from `search`'s output — copy one from there rather than guessing the path convention.

### 5. Keep it current while you work

```sh
okf-rs watch
```

Regenerates once immediately, then again each time a burst of file changes settles (default 300ms debounce), reusing the same incremental cache `generate` does. Leave it running in a terminal alongside your editor; stop with Ctrl+C.

### 6. Wire it into CI

```yaml
- name: Validate OKF bundle
  run: |
    okf-rs generate --no-cache
    okf-rs validate --ci
```

`--ci` treats orphaned-concept warnings as failures too, not just schema errors — useful once the bundle is something other tooling (docs, agents) actually depends on being correct. `--no-cache` in CI ensures a clean, from-scratch parse rather than trusting a cache that may not exist on that runner; drop it if you cache `.okf-cache.json` between CI runs and want the speed-up instead.

### 7. Register it with your AI coding agent

See [MCP server](#mcp-server) below — this is the step that turns "an agent can technically read these files" into "an agent can query the graph directly," and is where most of the token savings in day-to-day agent use come from.

## CLI reference

```
Usage: okf-rs <COMMAND>

Commands:
  init      Scan a project and write an `okf.toml` recording defaults for later commands
  scan      Recursively scan a repository and report what would be analyzed
  generate  Analyze a repository and write an OKF bundle
  watch     Watch a project and keep its OKF bundle up to date as files change
  validate  Validate that a directory is a conformant OKF bundle
  search    Search an OKF bundle by symbol, type, or tag
  coverage  Report content-completeness metrics: description/tag coverage and call-graph participation
  graph     Query the concept graph: callers, callees, cycles, isolated concepts, public API, cross-module dependencies, and topology stats
  diff      Compare the OKF concepts between two git refs (added/removed/changed)
  docs      Generate human-readable documentation (HTML site or Markdown) from an OKF bundle
```

Run `okf-rs <command> --help` for each command's options. `okf-rs generate` persists a `.okf-cache.json` at the project root keyed by each file's content hash, so a re-run only re-parses files that actually changed since the last one (report line: `N files parsed, M reused from cache`); pass `--no-cache` to bypass it and re-parse everything (the bundle it produces is byte-identical either way — the cache only affects how long it takes). Pass `--lsp` to also resolve calls whose name is ambiguous project-wide (more than one function/method sharing that name) by asking the call site's real language server (`rust-analyzer` for Rust, `pyright-langserver` for Python, wired up so far); this is strictly additive to Tree-sitter's own resolution and never changes the concept set, only adds `Calls`/`CalledBy` edges Tree-sitter's name-matching alone can't draw — it spawns and queries a real language server process, so it's meaningfully slower than a plain `generate` and skipped silently for a language with no available server. `okf-rs watch` runs that same cycle continuously: it regenerates once immediately, then again each time a burst of filesystem activity settles (`--debounce-ms`, default 300), printing a line only when something actually changed — silent on a wakeup caused by unrelated churn (e.g. a background `cargo build` touching `target/`) — and keeps running until interrupted (Ctrl+C). `okf-rs graph` has its own subcommands (`callers`, `callees`, `cycles`, `isolated`, `api`, `modules`, `stats`, `path`) — e.g. `okf-rs graph callers functions/src/auth/verify_token` lists everything that calls it, `okf-rs graph cycles` flags any call-graph cycles, `okf-rs graph isolated` lists concepts with no `Calls`/`CalledBy` edge at all (dead code or unresolved calls), and `okf-rs graph stats` reports per-kind concept counts, relationship edge counts by kind, and connected components of the call graph. Like `search` and `validate`, `graph` reads a previously generated bundle rather than re-analyzing the project, so run `okf-rs generate` first (and again after source changes). `okf-rs coverage` reports what fraction of the bundle has a description, has at least one tag, and actually participates in the call graph — a quick signal for how filled-in the knowledge base is, distinct from `validate`'s pass/fail checks. `okf-rs diff <ref-a> <ref-b>` compares two git refs' concepts without touching your working tree (it uses a temporary `git worktree` checkout for each ref). `okf-rs docs` reads a previously generated bundle (like `search`/`validate`/`graph`) and renders it for humans: `--format html` (the default) writes a browsable static site — one page per concept, cross-linked, plus a root index and one index per concept kind — into `docs/`; `--format markdown` writes a single consolidated `docs.md` with a table of contents instead; `--format pdf` writes a single paginated `docs.pdf` (grouped by kind, with a PDF outline/bookmark per concept for navigation) for printing, archiving, or sharing as one file. All three are templated directly from the bundle's concept data, no LLM involved. `okf-rs init` also writes/updates `CLAUDE.md`, `AGENTS.md`, and `.github/copilot-instructions.md` to point AI coding agents at the bundle — pass `--no-agent-files` to skip that.

## Architecture

`okf-rs` is a Cargo workspace of small, single-purpose crates under [`crates/`](crates/); `okf-cli` is a thin wrapper over the rest, so the same logic can be embedded by other Rust tools. See the [Architecture section](docs/specification.md#proposed-architecture) of the specification for the full crate-by-crate breakdown, including crates not yet built (`okf-server` — see [`ROADMAP.md`](ROADMAP.md)).

### MCP server

`okf-mcp` exposes a bundle's search, coverage, and graph queries as [Model Context Protocol](https://modelcontextprotocol.io) tools (`search`, `coverage`, `graph_callers`, `graph_callees`, `graph_api`, `graph_cycles`, `graph_isolated`, `graph_modules`, `graph_stats`, `graph_path`) over stdio, so an MCP-aware agent can query the knowledge base directly instead of re-reading raw source. Point it at a project root (defaults to `.`); it resolves the bundle the same way `search`/`validate`/`graph` do (`okf.toml`'s `output`, or `knowledge`), and re-reads the bundle on every call so it always reflects the latest `okf-rs generate`.

Register it with Claude Code:

```sh
claude mcp add okf-rs -s project -- /path/to/okf-mcp /path/to/project
```

`-s project` writes the registration to `.mcp.json` at the repo root so it's shared via git with every contributor's Claude Code instance, instead of only the local user's.

Register it with opencode by adding it to `opencode.json`:

```json
{
  "mcp": {
    "okf-rs": {
      "type": "local",
      "command": ["/path/to/okf-mcp", "/path/to/project"]
    }
  }
}
```

or point any other MCP client's stdio transport at the same binary and argument.

#### Why this reduces token consumption

Without `okf-rs`, an agent answering "who calls `verify_token`?" has to `grep` for the name, then open every file that matches to read enough surrounding code to confirm which hits are real call sites — each opened file costs its full size in context tokens, and a large file costs that every single time it's reopened across a session, including after a context compaction.

With `okf-mcp`, the same question is one `graph_callers` tool call returning just the answer — no source file enters the agent's context at all. Concretely, on this repository itself: asking "who calls `cmd_generate`?" by hand means `grep`-ing to `crates/okf-cli/src/main.rs` and reading enough of that 672-line, ~24 KB file to find the answer (`run`) — roughly 6,000 tokens by the common ~4-chars-per-token rule of thumb. `okf-rs graph callers functions/crates/okf-cli/src/cmd_generate` (or the equivalent `graph_callers` MCP call) returns exactly one line, `functions/crates/okf-cli/src/run — Rust Function` — on the order of 15 tokens. That's not a one-off: it's the same gap on every call-graph/API-surface question an agent asks, and it compounds over a long session, since each `grep`-and-read round re-pays a whole file's token cost while each MCP call stays cheap regardless of how many times it's made — the expensive part (parsing and resolving the call graph) already happened once, at `okf-rs generate` time, not on every query.

The same logic applies beyond MCP: even just pointing an agent at the `knowledge/` bundle's `index.md` files (no MCP server registered) means it's skimming pre-extracted signatures and relationships instead of full implementation bodies, comments, and boilerplate — a smaller, more targeted read for the same "what does this module expose" question.

## Contributing

Issues and pull requests are welcome. Before opening a PR:

```sh
cargo fmt --all
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

All of the above run in CI on every pull request.

## License

Licensed under either of

- [MIT license](LICENSE-MIT)
- [Apache License, Version 2.0](LICENSE-APACHE)

at your option.
