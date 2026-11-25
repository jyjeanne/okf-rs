---
name: okf-graph-validation
description: >
  Check a finished code change against the OKF knowledge graph: which
  concepts the change actually added, removed, or changed, whether the
  regenerated bundle is valid, and which tests reach the changed code. Use
  after any modification to existing code, before saying the change is
  done, and before opening a pull request.
metadata:
  okf_pack: "1.4"
---

# OKF Graph Validation — what did the change really touch?

The MCP tools below belong to the `okf` MCP server (in Claude Code they
are named `mcp__okf__<tool>`). The `okf-rs` commands are shell commands.

This skill assumes the bundle was current **before** the change (the
`okf-impact` skill checks that first). If you don't know, say so in the
report: step 1's list then mixes this change with older drift.

The graph sees signatures, call edges, and line ranges, not function
bodies: an edit inside a body that changes no call shows up in `git diff`
only. Steps 1 and 2 cover both sides.

## Method

1. **Concepts edited.** Run `git diff -U0` (add `BEFORE AFTER` for
   committed work). Map each hunk to the function, method, or type that
   encloses it, and confirm each id with MCP `search`. These are the
   concepts the change edited, whether or not the graph noticed.
2. **Graph changes of the working tree.** Run
   `okf-rs generate --check-fresh` before regenerating. It lists the
   bundle files (one `.md` per concept; ignore `index.md`) whose
   signature, call edges, or line range changed, including callers whose
   edges changed. An edit that adds or removes lines also lists the later
   concepts in the same file, because their line ranges moved: drop those
   unless step 1 found them. "Up to date" means no signature, call edge,
   or line range changed; it does not mean nothing changed.
3. **Regenerate.** Run `okf-rs generate`.
4. **Validate.** Run `okf-rs validate`. It must end with "no issues
   found". Report every error or warning verbatim; do not edit files under
   the bundle directory to silence them.
5. **Graph diff between refs (only if the change is committed).** With the
   commit before the change as BEFORE and the change's commit as AFTER, run
   `okf-rs diff BEFORE AFTER` and `okf-rs impact BEFORE AFTER`. These
   compare git refs, never the working tree, and like step 2 they count a
   concept as changed only when its signature or call edges change. Skip
   this step for uncommitted work.
6. **Affected tests.** For each concept from steps 1 and 2 that still
   exists (at most 5, the ones with the largest blast radius first), MCP
   `graph` with `relation: "plan-change"` and its id; collect the union of
   their `tests`. Run those tests, or the project's whole test command if
   they cannot be run one by one. Report the command and the result.
7. **Compare with the plan.** If an `okf-impact` plan exists, list every
   concept from steps 1 and 2 that the plan did not name (as target,
   caller, callee edited, or new concept) under "Unplanned changes".

## Report format (fixed)

```
## Graph validation

Edited (git diff): <ids>
Graph changes (<check-fresh | diff BEFORE..AFTER>):
  Added: <ids>   Removed: <ids>   Changed: <ids>
Bundle: regenerated; validate: <no issues | the issues, verbatim>
Impact (refs only): <largest blast radius, public API changed, cycles>
Tests reaching the change (<n>): <ids>
Tests run: <command> — <passed | failed: names>
Unplanned changes: <ids, or none, or "no plan">
```

## Anti-patterns (forbidden)

- Regenerating before step 2: the working-tree graph changes are lost.
- Reporting "no changes" from `check-fresh`, `diff`, or `impact` when
  `git diff` shows edits.
- Editing generated bundle files by hand.
- Saying the change is done while `validate` reports issues or the
  affected tests fail.
