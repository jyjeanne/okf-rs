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

Produce an interoperable OKF bundle.

Example layout:

```
knowledge/
├── index.okf
├── modules/
├── packages/
├── classes/
├── functions/
├── apis/
├── architecture/
└── glossary/
```

Each OKF document contains:

- Metadata
- Source location
- Relationships
- Dependencies
- References
- Documentation
- Tags

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
