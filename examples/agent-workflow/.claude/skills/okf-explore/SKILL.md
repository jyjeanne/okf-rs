---
name: okf-explore
description: >
  Answer questions about the codebase's structure through the OKF knowledge
  graph before reading source: who calls a function, what code or which
  commands a change would reach, the call path between two functions,
  recursion and call cycles, module dependencies. Also use it to explain,
  locate, or navigate code, symbols, and modules.
metadata:
  okf_pack: "1.3"
---

# OKF Exploration — graph first, source second

The tools below belong to the `okf` MCP server (in Claude Code they are
named `mcp__okf__<tool>`, e.g. `mcp__okf__explore`). If they are not
available, tell the user the okf MCP server is not configured and stop
using this skill.

## Method

1. Identify candidate concepts from the user request (names, domain terms).
2. MCP `search` with those terms to get exact concept ids. If results are
   ambiguous or empty, MCP `search_ranked` with a descriptive phrase.
3. MCP `explore` on the chosen concept id: location, signature,
   description, callers and callees (each with its location), blast
   radius, public API membership, cycle membership. ONE call.
4. Structural questions — answer from the graph:
   - Who calls X: `graph` relation `callers` (the full list; `explore`
     caps it).
   - What X reaches, or which entry points reach X: `graph` relation
     `plan-change` on X (`affected_concepts` is every transitive caller),
     then `path` from an entry point to X for the chain.
   - Call path between two functions: `graph` relation `path`.
   - Recursion: `graph` relation `cycles`, or `explore`'s cycle flag.
   - Module or package dependencies: `graph` relation `modules` or
     `communities`.
5. Check the graph where it can be wrong: callers are matched by name, so
   a method whose name many types share (e.g. `concurrency`, `find`) may
   show no callers. If the graph reports **no** callers for something that
   is clearly used, grep for its call sites and say in the answer which
   facts came from grep.
6. Read source only to confirm or explain: the line ranges the `Location`
   values point to (`file#Lstart-Lend`), and only those. Do not search for
   a function the graph has already located.
7. Synthesize. For every structural claim, name the MCP call (or grep)
   that showed it.

## Anti-patterns (forbidden)

- Reading files before the first `explore` call.
- Grepping for a concept `explore` already gave a location for.
- Reconstructing callers or call chains by reading source when the graph
  lists them.
- Calling `explore` on more than 2–3 concepts before synthesizing.
