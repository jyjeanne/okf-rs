#!/usr/bin/env bash
# One benchmark session. usage: run.sh <baseline|treatment> <task-id> <prompt>
#
# Same model and the same built-in tools (Read, Grep, Glob, Skill) in both
# variants; no user settings; no MCP servers except okf in the treatment.
# Transcript: runs/<task>-<variant>.jsonl (stream-json).
P="$(cd "$(dirname "$0")" && pwd)"
v="${1:?variant}"; t="${2:?task id}"; prompt="${3:?prompt}"
cd "$P/$v" || exit 2
args=(-p "$prompt" --output-format stream-json --verbose --no-session-persistence
      --setting-sources project,local --permission-mode dontAsk --strict-mcp-config
      --tools "Read,Grep,Glob,Skill")
if [ "$v" = treatment ]; then
  args+=(--mcp-config "$P/mcp.json"
         --allowedTools Read Grep Glob Skill mcp__okf__search mcp__okf__search_ranked
           mcp__okf__search_semantic mcp__okf__explore mcp__okf__coverage mcp__okf__graph)
else
  args+=(--allowedTools Read Grep Glob Skill)
fi
timeout 1500 claude "${args[@]}" < /dev/null > "$P/runs/$t-$v.jsonl" 2> "$P/runs/$t-$v.err"
echo "$t-$v exit=$?" >> "$P/runs/status.txt"
