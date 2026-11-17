---
name: okf-explore
description: >
  Explore an unfamiliar or existing part of the codebase through the OKF
  knowledge graph before reading any source file. Use when the user asks
  to explain, understand, locate, or navigate code, symbols, modules,
  call paths, or architecture.
metadata:
  okf_pack: "1.2"
---

# OKF Exploration — graph first, source second

The tools below belong to the `okf` MCP server (in Claude Code they are
named `mcp__okf__<tool>`, e.g. `mcp__okf__explore`). If they are not
available, tell the user the okf MCP server is not configured and stop
using this skill.

## Method

1. Identify candidate concepts from the user request (names, domain terms).
2. MCP `search` with those terms to locate concepts and get their ids. If
   results are ambiguous or empty, MCP `search_ranked` with a descriptive
   phrase.
3. MCP `explore` on the chosen concept id: location, signature,
   description, callers and callees (each with its location), blast
   radius, public API membership, cycle membership. ONE call.
4. If the question is architecture-level, MCP `graph` with relation
   `modules`, `communities`, or `path` (with `from` and `to` ids).
5. NOW read source: read the line ranges the `Location` values point to
   (`file#Lstart-Lend` → read that file from `start` for `end - start + 1`
   lines), and only those. Do not search for a function the graph has
   already located.
6. Synthesize the answer. For every claim about who calls what, name the
   MCP call that showed it. Never grep the whole repository to find
   something the graph already knows.

## Anti-patterns (forbidden)

- Reading files before the first `explore` call.
- Grepping for a concept `explore` already gave a location for.
- Guessing callers or callees from reading source.
- Calling `explore` on more than 2–3 concepts before synthesizing.
