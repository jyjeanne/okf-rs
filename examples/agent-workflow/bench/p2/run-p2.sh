#!/usr/bin/env bash
# Runs one P2 agent session prepared by setup-p2.sh.
#
# usage: run-p2.sh <work dir> <prompt file>
#
# The developer agent (`--agent okf-developer`) gets the task statement
# verbatim. If a turn ends before any file was edited (it stopped for plan
# approval or anything else), the session is resumed with exactly
# "Approved. Implement it.", at most twice. Transcripts:
# <work dir>/runs/turn<N>.jsonl (stream-json).
set -uo pipefail
work="$(cd "${1:?work dir}" && pwd)"
prompt="$(cat "${2:?prompt file}")"
native="$(cd "$work" && (pwd -W 2>/dev/null || pwd))"
cd "$work/repo" || exit 2

args=(--output-format stream-json --verbose --agent okf-developer --model claude-opus-5-5
      --setting-sources project,local --permission-mode dontAsk
      --strict-mcp-config --mcp-config "$native/mcp.json"
      --allowedTools Read Grep Glob Skill Edit Write
        mcp__okf__search mcp__okf__search_ranked mcp__okf__search_semantic
        mcp__okf__explore mcp__okf__coverage mcp__okf__graph
        "Bash(cargo *)" "Bash(okf-rs *)" "Bash(git *)"
        "Bash(head *)" "Bash(tail *)" "Bash(grep *)")
export CLAUDE_ENV_FILE="$work/env.sh"

edited() { grep -q '"name":"\(Edit\|Write\)"' "$work"/runs/turn*.jsonl 2>/dev/null; }
session_of() { node -e 'const fs=require("fs");for(const l of fs.readFileSync(process.argv[1],"utf8").split("\n")){try{const j=JSON.parse(l);if(j.session_id){console.log(j.session_id);break}}catch{}}' "$1"; }

timeout 3600 claude -p "$prompt" "${args[@]}" < /dev/null > "$work/runs/turn1.jsonl" 2> "$work/runs/turn1.err"
echo "turn1 exit=$?" >> "$work/runs/status.txt"
sid="$(session_of "$work/runs/turn1.jsonl")"
for n in 2 3; do
  edited && break
  timeout 3600 claude -p "Approved. Implement it." --resume "$sid" "${args[@]}" < /dev/null \
    > "$work/runs/turn$n.jsonl" 2> "$work/runs/turn$n.err"
  echo "turn$n exit=$?" >> "$work/runs/status.txt"
done
echo "session $sid done" >> "$work/runs/status.txt"
