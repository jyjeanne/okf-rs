---
name: okf-impact
description: >
  Plan a change to existing code from the OKF knowledge graph before
  editing it: everything that calls it directly or transitively, the public
  API, modules, and tests it reaches, risk signals, and what to inspect
  first. Use when the user wants to change, fix, modify, rename, extend, or
  remove an existing function, method, or type, or asks what a change
  would break.
metadata:
  okf_pack: "1.4"
---

# OKF Impact — plan the change before the edit

The tools below belong to the `okf` MCP server (in Claude Code they are
named `mcp__okf__<tool>`, e.g. `mcp__okf__graph`). If they are not
available, tell the user the okf MCP server is not configured and stop
using this skill.

## Method

1. **Check the bundle is current.** Run `okf-rs generate --check-fresh`.
   If it reports stale files, run `okf-rs generate` and continue. If you
   cannot run shell commands, say in the plan that it assumes a current
   bundle.
2. **Find the target.** MCP `search` with the name the user gave → exact
   concept id. If several concepts match, MCP `explore` on each candidate
   (at most 3) and choose by location; if it is still ambiguous, ask the
   user which one.
3. **Explore it.** MCP `explore` on the target id: location, signature,
   callers, callees. ONE call.
4. **Compute the plan.** MCP `graph` with `relation: "plan-change"` and
   `id` set to the target id. ONE call. It takes only an exact id; free text
   is rejected, so never pass the user's wording.
5. **Read only what the plan names.** Read the target's `Location` and the
   locations of the concepts in `recommended_inspection`, nothing else
   yet. If the change will also edit a callee, `explore` that callee too.
6. **Write the plan** in the format below. Every list comes from the
   `plan-change` output; do not add callers or tests from your own reading
   without marking them `(from source)`.
7. **Stop for approval.** Do not edit any file until the user approves the
   plan.

## Plan format (fixed)

```
## Change plan: <target id>

Target: <id> — <kind>, <file#Lstart-Lend>. Public API: yes|no. In a call cycle: yes|no.

Direct callers (<n>): <id>, …
Indirect callers (<n>): <first 10 ids>, … (+<rest> more)
Public API in scope (<n>): <id>, …
Modules: <module ids>
Tests that reach it (<n>): <id>, …
  Test detection is a heuristic: <the `limits` entries, verbatim>
Risk signals: <signal> — <detail>; …  (or: none)
Inspect first: <ref> — <reason>; …

Steps:
1. <edit>, in <concept id>
2. …
New concepts: <functions or types the change adds>  (or: none)
Callees edited too: <ids>  (or: none)
Tests to run: <the tests above, plus any new ones>
```

Keep the counts exact even when a list is shortened.

## Anti-patterns (forbidden)

- Editing before the plan is approved.
- Listing callers or blast radius from grep or from reading source when
  `plan-change` gives them.
- Calling `plan-change` with a name or a guessed id instead of the id
  `search` returned.
- Dropping a risk signal or a `limits` entry from the plan because it looks
  minor.
