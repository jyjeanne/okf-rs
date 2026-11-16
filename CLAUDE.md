<!-- okf-rs:begin -->
@AGENTS.md
<!-- okf-rs:end -->

## Commands

Rust workspace (edition per `Cargo.toml`); the binary is `okf-rs` (crate `okf-cli`).

- Build: `cargo build --workspace --locked` (CI always uses `--locked`; `Cargo.lock` is committed)
- Release binary: `cargo build --release` → `target/release/okf-rs`
- Test all: `cargo test --workspace --locked`
- Single test: `cargo test -p <crate> <name_filter>`, e.g. `cargo test -p okf-graph cycle`
- Integration tests (CLI end-to-end): `cargo test -p okf-cli --test e2e`
- Lint + format (required before a PR; clippy runs only on the Linux CI job): `cargo fmt --all` and `cargo clippy --workspace --all-targets -- -D warnings`
- Security: `cargo audit` (CI job; ignores are listed in `.cargo/audit.toml` with reasons)

`knowledge/` and `.okf-cache.json` are generated and gitignored. Run `cargo run -p okf-cli -- generate` (or `okf-rs generate`) before using `search`, `graph`, or `validate`, and again after source changes.

## Architecture

`okf-rs` turns source code into an OKF bundle (markdown + YAML frontmatter) and exposes it for querying. The pipeline runs in one direction; the read-side tools never re-analyze source.

1. **Extraction**: `okf-tree-sitter` (per-language grammars) and optional `okf-lsp` (`--lsp` disambiguation) emit symbols and call sites through the shared `okf-parser` abstraction.
2. **Analysis**: `okf-analyzer` orchestrates parsing and produces the language-agnostic semantic model. It keeps a content-hash cache (`cache.rs`) that backs `.okf-cache.json`.
3. **Graph**: `okf-graph` builds the call graph and does cycle, path, and reachability queries. `okf-arch` adds ownership, features, and patterns on top of it.
4. **Output**: `okf-generator` writes the bundle to `knowledge/` and the agent entry files (`okf-rs init`). `okf-validator` checks conformance. `okf-docs` and `okf-dita` render or import other formats.
5. **Read side**: `okf-search`, `okf-query`, and `okf-enrich` (semantic search, LLM descriptions) read the bundle from disk. `okf-mcp` serves the same queries over stdio, and `okf-watch` reruns generation on file changes.
6. **Entry point**: `okf-cli` (binary `okf-rs`) wires these together. `okf-core` holds shared config (`okf.toml`) and git helpers.

The bundle on disk is the contract between the write side (`generate`) and the read side (`search`, `graph`, `validate`, `mcp`). Changes to one side should be checked against the other, and the e2e tests in `crates/okf-cli/tests/e2e.rs` cover that path.

Design context: `docs/specification.md` (crate responsibilities), `ROADMAP.md`, and `docs/improvement-plan*.md`. `okf-server` is planned but not built.
