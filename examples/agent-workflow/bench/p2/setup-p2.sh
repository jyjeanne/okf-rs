#!/usr/bin/env bash
# Prepares one P2 agent session (docs/benchmarks/agent-integration-p2.md).
#
# usage: setup-p2.sh <okf-rs checkout> <reference commit> <work dir>
#
# <work dir>/repo becomes a fresh git repository holding the parent of the
# reference commit as one commit (no history, so the answer can't be read
# from it). Every `.claude/` folder of the old tree is removed; the pack's
# skills, agents and settings are installed; okf-rs and okf-mcp come from
# the okf-rs checkout's release build (build it first); the bundle is
# generated and left untracked; the test targets are built once so the
# agent's first test run fits the Bash timeout.
set -euo pipefail

okf="$(cd "${1:?okf-rs checkout}" && pwd)"
commit="${2:?reference commit}"
work="${3:?work dir}"
mkdir -p "$work"
work="$(cd "$work" && pwd)"
if [ -e "$work/repo" ]; then echo "error: $work already holds a run" >&2; exit 1; fi

ext=""; [ -f "$okf/target/release/okf-rs.exe" ] && ext=".exe"
mkdir -p "$work/bin" "$work/runs" "$work/repo"
cp "$okf/target/release/okf-rs$ext" "$okf/target/release/okf-mcp$ext" "$work/bin/"

git -C "$okf" archive "$commit^" | tar -x -C "$work/repo"
find "$work/repo" -type d -name .claude -prune -exec rm -rf {} +
pack="$okf/examples/agent-workflow/.claude"
mkdir -p "$work/repo/.claude"
cp -r "$pack/skills" "$pack/agents" "$pack/settings.json" "$work/repo/.claude/"
grep -qx '/knowledge' "$work/repo/.gitignore" 2>/dev/null || echo '/knowledge' >> "$work/repo/.gitignore"

(
  cd "$work/repo"
  git init -q
  git add -A
  git -c user.name=bench -c user.email=bench@localhost commit -q -m "base"
  "$work/bin/okf-rs$ext" generate . | head -1 > "$work/bundle.txt"
  cargo test --workspace --locked --no-run > "$work/prebuild.log" 2>&1
)

native="$(cd "$work" && (pwd -W 2>/dev/null || pwd))"
cat > "$work/mcp.json" <<EOF
{ "mcpServers": { "okf": { "command": "$native/bin/okf-mcp$ext", "args": ["$native/repo"] } } }
EOF
# Headless sessions don't pass the launching shell's PATH to the Bash tool;
# CLAUDE_ENV_FILE does (spec §6.5).
echo "export PATH=\"$work/bin:\$PATH\"" > "$work/env.sh"
{
  echo "reference: $(git -C "$okf" rev-parse "$commit") (base = its parent)"
  echo "okf-rs: $(git -C "$okf" rev-parse HEAD)"
  echo "bundle: $(cat "$work/bundle.txt")"
} > "$work/run-info.txt"
cat "$work/run-info.txt"
