# Community feedback: roadmap review (August 2026)

Raw improvement ideas submitted by a technical reviewer evaluating `okf-rs` for use as a
knowledge-graph backend for AI coding agents. Kept verbatim here for traceability — the
distilled, prioritized version tracked against actual delivery lives in
[`ROADMAP.md`](../../ROADMAP.md#phase-5--ai-native-platform-maturity-community-improvement-plan).

Recorded 2026-08-07. Source: external review, unsolicited feedback on the MCP tool surface,
determinism guarantees, and provenance/confidence tracking.

---

Here's a polished Markdown roadmap that you can add directly to your GitHub repository (for example, ROADMAP.md or docs/ROADMAP.md).

# OKF-rs Roadmap

This roadmap summarizes improvement ideas gathered from early community feedback and aims to make **OKF-rs** the reference knowledge graph generator for AI agents, CI pipelines, and developer tooling.

---

# Vision

OKF-rs should become:

- Deterministic
- Auditable
- Git-native
- AI-friendly
- CI-ready
- Language-independent

---

# High Priority

## 1. Optimize the MCP API

### Motivation

Each MCP tool contributes its JSON schema to the LLM system prompt.

A large number of narrowly focused tools increases prompt size and token consumption for every session.

### Goal

Reduce the number of exposed tools by providing more generic APIs.

Current approach:

- graph_callers
- graph_callees
- graph_dependencies
- graph_children
- graph_parents
- ...

Possible future approach:

```
explore(
    relation="callers"
)

explore(
    relation="dependencies"
)

explore(
    relation="children"
)
```

### Expected benefits

- Lower prompt token usage
- Faster agent initialization
- Simpler API
- Easier long-term maintenance

---

## 2. Measure Session-Level Performance

Current benchmarks focus on individual query speed.

Future benchmarks should also include:

- MCP initialization cost
- Prompt token overhead
- Session break-even point
- Total token savings
- Cost comparison against RAG

Example metrics:

- Tokens added by MCP
- Tokens saved per structural query
- Break-even after N queries

This provides a more realistic picture of real-world usage.

---

## 3. Strengthen Deterministic Builds

Tree-sitter parsing is deterministic.

Semantic resolution performed through Language Servers (Rust Analyzer, Pyright, TypeScript LS, etc.) may depend on workspace state, cache, or indexing.

### Goals

- Clearly document deterministic guarantees
- Improve reproducibility of LSP mode
- Detect non-deterministic graph generation

---

## 4. Record Edge Provenance

Every generated relationship should include information describing how it was produced.

Examples:

```yaml
resolved_by: tree-sitter
```

```yaml
resolved_by: rust-analyzer
```

Possible metadata:

- parser
- language server
- resolver version
- resolution strategy

### Benefits

- Easier debugging
- Better CI diagnostics
- Improved reproducibility
- Easier graph auditing

---

## 5. Confidence Levels

Not every relationship has the same level of certainty.

Possible confidence values:

- exact
- semantic
- inferred
- unresolved

This allows AI agents to prioritize high-confidence information.

---

# Medium Priority

## 6. Better Highlight Git-Native Markdown

One of OKF-rs' strongest features is that the generated knowledge graph is plain Markdown.

Advantages:

- Human-readable
- GitHub-native
- Reviewable in Pull Requests
- Version-controlled
- Searchable
- Auditable without specialized tools

Unlike proprietary graph databases or vector stores, anyone can inspect graph changes directly from GitHub.

---

## 7. CI Validation Mode

Introduce a dedicated validation command:

```
okf validate --ci
```

Possible checks:

- Graphs are up-to-date
- No unexpected changes
- Deterministic output
- Provenance metadata is valid
- Broken references are detected

Perfect for GitHub Actions.

---

## 8. Separate Syntax and Semantic Relationships

Differentiate between:

- Syntax-derived edges
- Semantic-derived edges

Example:

```yaml
kind: syntax
```

```yaml
kind: semantic
```

This helps users understand where information comes from and allows filtering during graph exploration.

---

# Long-Term

## 9. Real-World Benchmarks

Benchmark OKF-rs on large open-source projects.

Metrics:

- Graph generation time
- Graph size
- Memory usage
- MCP prompt size
- Token savings
- AI task completion time
- Comparison against RAG-based approaches

Languages:

- Rust
- Java
- Python
- TypeScript
- Go
- C#

---

## 10. Explainability

Provide richer explanations for graph relationships.

Example:

```
Function A
    ↓
calls
    ↓
Function B

Reason:
Resolved through Rust Analyzer symbol resolution.
```

This improves trust in AI-generated answers.

---

## 11. AI Agent Optimization

Continue optimizing OKF-rs specifically for AI agents.

Areas of investigation:

- Token-efficient graph serialization
- Incremental graph loading
- Lazy exploration
- Context-aware graph extraction
- Hybrid Graph + RAG workflows

---

# Research Topics

Potential future research includes:

- Deterministic semantic analysis
- Incremental graph updates
- Graph compression
- Provenance tracking
- Multi-language projects
- Graph federation
- AI-native graph formats
- Knowledge graph diff algorithms

---

# Guiding Principles

Every new feature should improve one or more of the following:

- Determinism
- Transparency
- Auditability
- AI efficiency
- Developer experience
- CI/CD integration
- Git friendliness

---

# Ultimate Goal

OKF-rs is not just another code graph generator.

Its objective is to become a **transparent, deterministic, Git-native knowledge graph platform** designed specifically for modern AI coding agents, developer tooling, and continuous integration workflows.

I would also recommend adding a "Status" column (✅ Completed, 🚧 In Progress, 💡 Planned) and converting this into a GitHub roadmap with milestones and linked issues. That makes the project look more professional and makes it easier for contributors to pick up work.
