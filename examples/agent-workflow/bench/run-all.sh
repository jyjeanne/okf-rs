#!/usr/bin/env bash
# Runs the 10 P1 sessions (5 questions x baseline/treatment) in parallel.
# The questions are verbatim from examples/agent-workflow/README.md.
P="$(cd "$(dirname "$0")" && pwd)"
: > "$P/runs/status.txt"
declare -A Q
Q[E1]='How does the MCP server avoid re-parsing the bundle on every tool call, and when does it re-read it?'
Q[E2]='How does `okf-rs generate --check-fresh` decide that the bundle on disk is stale?'
Q[E3]='How does `okf-rs diff --ci` tell a resolver-only relationship change from a source-level one?'
Q[E4]='How does okf-rs decide that a language server is ready to answer queries?'
Q[E5]='What happens when a source file is not valid UTF-8?'
for t in E1 E2 E3 E4 E5; do
  for v in baseline treatment; do
    "$P/run.sh" "$v" "$t" "${Q[$t]}" &
  done
done
wait
cat "$P/runs/status.txt"
