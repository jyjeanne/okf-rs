# okf-rs

«A fast, open-source Rust toolkit for generating, validating and serving Open Knowledge Format (OKF) knowledge bases from source code.»

## Vision

"okf-rs" aims to become the reference Rust implementation of the Open Knowledge Format ecosystem for software projects.

Unlike traditional documentation generators, "okf-rs" extracts semantic knowledge from a codebase and produces a portable OKF bundle that can be consumed by AI assistants, IDEs, documentation systems, and developer tools.

The project follows four principles:

- Open — Produce standard OKF bundles.
- Fast — Native Rust performance with parallel analysis.
- Deterministic — Generate identical output for identical source code.
- AI-ready — Create structured knowledge without requiring an LLM.

---

## About the Open Knowledge Format (OKF)

The Open Knowledge Format is an open, vendor-neutral specification for representing metadata, context, and curated knowledge in a way that is equally readable by humans and parseable by AI agents. It formalizes the "knowledge as a living wiki" pattern: a shared library of files that both people and LLMs can read, cross-reference, and keep up to date over time.

Key characteristics of the OKF standard that "okf-rs" targets:

- **Concepts as files** — Each unit of knowledge (a module, a class, a function, a metric, an API, …) is one markdown file with a YAML frontmatter header and a markdown body.
- **One required field** — The frontmatter's only mandatory key is `type`. Everything else (`title`, `description`, `resource`, `tags`, `timestamp`, …) is optional, which keeps the format minimally opinionated about content models.
- **Bundle as a directory** — An OKF bundle is a directory of concept files, typically grouped by kind (`modules/`, `functions/`, `apis/`, …), with optional `index.md` files for progressive disclosure and optional `log.md` files for chronological change history.
- **Graph via links** — Concepts reference each other through ordinary markdown links; the directory structure implies parent/child relationships while cross-links create a richer graph on top of the filesystem hierarchy.
- **Just files, just markdown** — A bundle is readable in any editor, renders natively on GitHub, is indexable by any search tool, and ships as a plain tarball or git repository — no proprietary runtime, database, or SDK is required to read or write it.
- **Producer/consumer independence** — The format is the contract, not the tooling. A human-authored bundle can be consumed by an AI agent, a metadata pipeline can feed a visualizer, and one LLM can synthesize knowledge that another LLM later queries — each side is free to swap implementations.
- **Format, not platform** — OKF is not tied to any cloud provider, database, model vendor, or agent framework, which is what "okf-rs" means by "Open" in its own design principles.

"okf-rs" applies this specification to source code: instead of documenting datasets and tables, it extracts packages, modules, types, functions, and their relationships from a codebase and emits them as a conformant OKF bundle.

---

## Goals

- Generate an OKF knowledge base from any Git repository.
- Support multiple programming languages.
- Extract semantic relationships instead of plain text.
- Enable incremental updates.
- Integrate with modern AI coding assistants.
- Provide a reusable Rust library and CLI.

---

## Main Features

### Repository Analysis

- Recursive repository scanning
- Git-aware indexing
- Incremental updates
- Ignore ".gitignore"
- Workspace support

Supported ecosystems:

- Rust
- Java
- Kotlin
- TypeScript
- JavaScript
- Python
- Go
- C#
- C/C++
- PHP
- Swift
- DITA XML (future)

---

### Semantic Extraction

Using Tree-sitter and Language Server Protocol (LSP), "okf-rs" extracts:

- Packages
- Modules
- Classes
- Traits
- Interfaces
- Structs
- Enums
- Functions
- Methods
- Variables
- Constants

Relationships:

- Imports
- Dependencies
- Call graph
- Inheritance
- Trait implementations
- Interface implementations
- Module hierarchy

---

### Knowledge Graph

Following the OKF convention, the graph is expressed directly in the bundle: directory structure encodes parent/child relationships (e.g. a module's functions live under it), and markdown links between concept files encode arbitrary cross-references (calls, implementations, dependencies). No separate database is required to reconstruct the graph — any OKF-aware tool can traverse it by following links.

Generate a semantic graph containing:

- Symbols
- References
- Ownership
- Dependencies
- API surface
- Cross-module links

Future enhancements:

- Architectural layers
- Domain boundaries
- Design patterns
- REST endpoints
- Database models
- Event flows

---

### OKF Generation

Produce an interoperable OKF bundle: a directory of markdown files with YAML frontmatter, one file per concept, following the Open Knowledge Format specification.

Example layout:

```
knowledge/
├── index.md
├── modules/
│   ├── index.md
│   └── auth.md
├── packages/
│   └── index.md
├── classes/
│   └── index.md
├── functions/
│   ├── index.md
│   └── verify_token.md
├── apis/
│   └── index.md
├── architecture/
│   └── index.md
├── glossary/
│   └── index.md
└── log.md
```

Each concept file contains a YAML frontmatter header plus a markdown body:

```yaml
---
type: Rust Function
title: verify_token
description: Validates a JWT and returns the decoded claims.
resource: src/auth/token.rs#L42
tags: [auth, security]
timestamp: 2026-07-28T10:00:00Z
---

# Signature
`fn verify_token(token: &str) -> Result<Claims, AuthError>`

# Calls
- [decode_jwt](../functions/decode_jwt.md)
- [check_expiry](../functions/check_expiry.md)

# Called by
- [authenticate_request](../functions/authenticate_request.md)
```

Only `type` is mandatory; `okf-rs` populates the remaining frontmatter fields (`title`, `description`, `resource`, `tags`, `timestamp`) and body sections (signature, relationships, documentation) from static analysis, and cross-links concepts using regular markdown links so the bundle forms a navigable graph.

---

### Documentation Generation

Generate documentation from the OKF bundle.

Formats:

- Markdown
- HTML
- DITA (planned)
- PDF (planned)

---

### Search Engine

Provide semantic search by:

- Symbol
- Package
- Module
- Type
- API
- Relationship
- Tag

---

### MCP Server

Expose repository knowledge through the Model Context Protocol.

Example queries:

- Explain this module.
- Show callers of this function.
- Find REST endpoints.
- Find architectural violations.
- List public APIs.

---

### Optional AI Enrichment

When enabled, an LLM may generate:

- Function summaries
- Module descriptions
- Architecture explanations
- Usage examples
- Glossary entries

The generated OKF remains valid even without AI enrichment.

---

## CLI

Examples:

```
okf-rs init

okf-rs scan .

okf-rs generate

okf-rs validate

okf-rs search authentication

okf-rs serve

okf-rs diff main feature/login
```

---

## Library API

```rust
let project = Project::load("./my-project")?;
let bundle = Analyzer::new().analyze(project)?;
bundle.write("./knowledge")?;
```

---

## Proposed Architecture

```
okf-rs/
├── okf-cli
├── okf-core
├── okf-parser
├── okf-tree-sitter
├── okf-lsp
├── okf-analyzer
├── okf-graph
├── okf-generator
├── okf-validator
├── okf-search
├── okf-mcp
├── okf-server
└── okf-watch
```

---

## Design Principles

- Modular
- Extensible
- Plugin-based
- Parallel
- Incremental
- Cross-platform
- Offline-first
- Deterministic

---

## Roadmap

### Phase 1

- CLI
- Tree-sitter parsing
- OKF generation
- Validator
- Search

### Phase 2

- LSP integration
- Incremental indexing
- Graph generation
- MCP server

### Phase 3

- AI enrichment
- Architecture extraction
- Documentation generation
- DITA export

### Phase 4

- IDE plugins
- Continuous indexing
- Distributed knowledge server
- Visualization

---

## Target Users

- Software developers
- Technical architects
- Technical writers
- DevOps engineers
- AI coding assistants
- Documentation teams

---

## Why okf-rs?

Current repository analysis tools generally produce proprietary graphs, Markdown summaries, or AI-specific context.

"okf-rs" instead produces an open, portable, semantic knowledge base based on the Open Knowledge Format, enabling interoperability between AI assistants, IDEs, documentation generators, CI/CD pipelines, and software architecture tools.

Its combination of native Rust performance, deterministic analysis, incremental indexing, and standards-based output aims to make "okf-rs" the reference engine for software knowledge extraction in the OKF ecosystem.

---

## References

- Google Cloud Blog — [How the Open Knowledge Format can improve data sharing](https://cloud.google.com/blog/products/data-analytics/how-the-open-knowledge-format-can-improve-data-sharing?hl=en)
