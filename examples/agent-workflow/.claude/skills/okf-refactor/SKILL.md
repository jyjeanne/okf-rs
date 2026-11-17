---
name: okf-refactor
description: >
  Plan and verify an architectural refactor with the OKF knowledge graph:
  splitting or moving a module, extracting a component, breaking a
  dependency cycle, decoupling two parts of the code. The graph sets the
  boundary before the edit, and a before/after graph diff checks the
  result. Use when the user asks to restructure, split, extract, move,
  decouple, untangle, or reorganize code.
metadata:
  okf_pack: "1.4"
---

# OKF Refactor — the graph draws the boundary and checks the result

The MCP tools below belong to the `okf` MCP server (in Claude Code they
are named `mcp__okf__<tool>`). The `okf-rs` and `git` commands are shell
commands.

## Method

1. **Fix the starting ref.** `okf-rs diff` and `okf-rs impact` compare
   git refs, not the working tree, so the refactor needs a commit before
   and a commit after. Run `git status --porcelain`: if it prints anything,
   stop and ask the user to commit or stash first. Record
   `git rev-parse HEAD` as BEFORE.
2. **Check the bundle is current.** Run `okf-rs generate --check-fresh`;
   if it reports stale files, run `okf-rs generate`.
3. **Read the current structure.** MCP `graph` with relation `modules`
   (which modules call into which), then `communities` (which packages
   belong together), then `cycles`.
4. **Trace the edges to cut.** For each dependency the refactor should
   remove, MCP `graph` relation `path` between one concept on each side
   (`from`, `to`): the chain is the list of calls that must change. MCP
   `explore` on each concept that moves (at most 3 before writing the
   plan).
5. **Find the API at stake.** MCP `graph` relation `api`, kept to the
   modules involved: public concepts there are the ones whose outside
   callers break if they move or change signature. For each of them that
   changes, MCP `graph` relation `plan-change` with its id.
6. **Write the plan** in the format below, then **stop for approval**.
7. **Implement** the approved steps. Run the tests the `plan-change`
   outputs listed.
8. **Commit the result** (ask the user first if they have not asked you
   to commit). Record `git rev-parse HEAD` as AFTER. Run `okf-rs generate`,
   then `okf-rs validate`.
9. **Check against the plan.** Run `okf-rs diff BEFORE AFTER` and
   `okf-rs impact BEFORE AFTER`, then MCP `graph` relations `modules` and
   `cycles` again. Compare with "Expected graph diff": report every
   concept added, removed, or changed that the plan did not expect, every
   dependency the plan cut that still exists, and every cycle that
   remains or is new. `diff` counts a concept as changed only when its
   signature or call edges change; for edits inside bodies, use
   `git diff BEFORE AFTER`.

## Plan format (fixed)

```
## Refactor plan: <goal in one line>

BEFORE: <sha>
Current structure: <module edges and cycles involved, from `modules`/`cycles`>
Boundary: moves → <concept ids>; stays → <concept ids>
Dependencies to cut: <from> → … → <to> (from `path`), one line each
Public API affected (<n>): <id> — <n> callers outside the boundary
Cycles: before <list>; expected after <list>
Steps:
1. <edit>, in <concept id>
Expected graph diff: added <ids>; removed <ids>; changed <ids>
Tests to run: <from plan-change `tests`>
```

## Result format (fixed)

```
## Refactor result: BEFORE <sha> → AFTER <sha>

okf-rs diff: added <n>, removed <n>, changed <n>
Unexpected changes: <ids, or none>
Dependencies still present: <edges, or none>
Cycles: before <list>; after <list>
validate: <no issues | issues verbatim>
Tests: <command> — <result>
```

## Anti-patterns (forbidden)

- Starting from a dirty working tree, or comparing against the working
  tree instead of the AFTER commit.
- Choosing the boundary from directory names without `modules`,
  `communities`, and `path`.
- Reporting the refactor as done without the before/after graph diff.
