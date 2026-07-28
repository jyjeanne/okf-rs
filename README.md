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

- **Repository scanning** — recursive, `.gitignore`-aware, with git-aware indexing and manifest detection (`Cargo.toml`, `package.json`, `pyproject.toml`, `go.mod`)
- **Semantic extraction** for **Rust, Python, TypeScript, JavaScript, Go, Java, and C#** — packages, modules, types (structs/classes/enums/interfaces/traits), functions, and methods, including public/private API-surface detection
- **Relationship extraction** — imports, and a resolved call graph covering bare calls, `self.method()`/`this.method()`, `Type::method()`/`Type.method()`, and `module::func()` forms across all seven languages
- **OKF bundle generation** — markdown + YAML frontmatter, cross-linked, with `index.md` navigation at every level
- **Incremental indexing** — `okf-rs generate` caches each file's extraction by content hash, so a re-run only re-parses what actually changed
- **Validation** — frontmatter/schema checks, dangling-link detection, orphan detection, and duplicate-identity checks (both path collisions and same-symbol-different-file)
- **Search** — ranked free-text search by symbol, package, module, type, and tag
- **Graph queries** — callers/callees, call-graph cycle detection, cross-module dependencies, and shortest call path
- **Bundle diffing** — compare a project's concepts between two git refs without touching your working tree
- **AI agent integration** — `okf-rs init` writes/updates `CLAUDE.md`, `AGENTS.md`, and `.github/copilot-instructions.md`, idempotently; `okf-mcp` exposes search and graph queries as an MCP server for tools like Claude Code
- **Standalone binary** — no runtime dependency beyond the OS's standard C library; see [Packaging & Distribution](docs/specification.md#packaging--distribution)

See [`ROADMAP.md`](ROADMAP.md) for what's shipped (Phase 1 complete, Phase 2 in progress) and what's next.

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
---

# Signature

`fn verify_token(&self, token: &str) -> bool`

# Calls

- [decode_jwt](../../../functions/src/Auth/decode_jwt.md)
```

Just a markdown file. Open it in any editor, render it on GitHub, or point an AI coding agent at the `knowledge/` directory and let it follow the links.

## CLI reference

```
Usage: okf-rs <COMMAND>

Commands:
  init      Scan a project and write an `okf.toml` recording defaults for later commands
  scan      Recursively scan a repository and report what would be analyzed
  generate  Analyze a repository and write an OKF bundle
  validate  Validate that a directory is a conformant OKF bundle
  search    Search an OKF bundle by symbol, type, or tag
  graph     Query the concept graph: callers, callees, cycles, public API, and cross-module dependencies
  diff      Compare the OKF concepts between two git refs (added/removed/changed)
```

Run `okf-rs <command> --help` for each command's options. `okf-rs generate` persists a `.okf-cache.json` at the project root keyed by each file's content hash, so a re-run only re-parses files that actually changed since the last one (report line: `N files parsed, M reused from cache`); pass `--no-cache` to bypass it and re-parse everything (the bundle it produces is byte-identical either way — the cache only affects how long it takes). `okf-rs graph` has its own subcommands (`callers`, `callees`, `cycles`, `api`, `modules`, `path`) — e.g. `okf-rs graph callers functions/src/auth/verify_token` lists everything that calls it, and `okf-rs graph cycles` flags any call-graph cycles. Like `search` and `validate`, `graph` reads a previously generated bundle rather than re-analyzing the project, so run `okf-rs generate` first (and again after source changes). `okf-rs diff <ref-a> <ref-b>` compares two git refs' concepts without touching your working tree (it uses a temporary `git worktree` checkout for each ref). `okf-rs init` also writes/updates `CLAUDE.md`, `AGENTS.md`, and `.github/copilot-instructions.md` to point AI coding agents at the bundle — pass `--no-agent-files` to skip that.

## Architecture

`okf-rs` is a Cargo workspace of small, single-purpose crates under [`crates/`](crates/); `okf-cli` is a thin wrapper over the rest, so the same logic can be embedded by other Rust tools. See the [Architecture section](docs/specification.md#proposed-architecture) of the specification for the full crate-by-crate breakdown, including crates not yet built (`okf-lsp`, `okf-server`, `okf-watch` — see [`ROADMAP.md`](ROADMAP.md)).

### MCP server

`okf-mcp` exposes a bundle's search and graph queries as [Model Context Protocol](https://modelcontextprotocol.io) tools (`search`, `graph_callers`, `graph_callees`, `graph_api`, `graph_cycles`, `graph_modules`, `graph_path`) over stdio, so an MCP-aware agent can query the knowledge base directly instead of re-reading raw source. Point it at a project root (defaults to `.`); it resolves the bundle the same way `search`/`validate`/`graph` do (`okf.toml`'s `output`, or `knowledge`), and re-reads the bundle on every call so it always reflects the latest `okf-rs generate`.

Register it with Claude Code:

```sh
claude mcp add okf-rs -- /path/to/okf-mcp /path/to/project
```

or point any other MCP client's stdio transport at the same binary and argument.

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
