#!/usr/bin/env bash
# Prepares the P1 A/B benchmark (docs/benchmarks/agent-integration-p1.md).
#
# usage: setup.sh <okf-rs checkout> <commit> <work dir>
#
# Creates <work dir>/{baseline,treatment} as `git archive` exports of
# <commit> (no .git, so agents can't read answers from history), builds
# okf-rs and okf-mcp from the checkout into <work dir>/bin, generates the
# bundle in the treatment copy only, installs the okf-explore skill there,
# and writes <work dir>/mcp.json for the okf server.
#
# The checkout's crates/ and Cargo files must match <commit> (the binaries
# are built from them). Both copies drop the pack's own .claude/ folder (a nested skill
# could otherwise be discovered in the baseline) and the pack README (it
# maps each question to the files holding its answer).
set -euo pipefail

repo="$(cd "${1:?okf-rs checkout}" && pwd)"
commit="${2:?commit}"
work="${3:?work dir}"
mkdir -p "$work"
work="$(cd "$work" && pwd)"

if [ -e "$work/baseline" ] || [ -e "$work/treatment" ]; then
  echo "error: $work already holds a run; use an empty directory" >&2
  exit 1
fi

cd "$repo"
# Only the code the binaries are built from has to match <commit>.
git diff --quiet "$commit" -- crates Cargo.toml Cargo.lock \
  || { echo "error: crates/ or Cargo files differ from $commit" >&2; exit 1; }
cargo build -q --release -p okf-cli -p okf-mcp
mkdir -p "$work/bin" "$work/runs" "$work/baseline" "$work/treatment"
ext=""; [ -f target/release/okf-rs.exe ] && ext=".exe"
cp "target/release/okf-rs$ext" "target/release/okf-mcp$ext" "$work/bin/"

for v in baseline treatment; do
  git archive "$commit" | tar -x -C "$work/$v"
  rm -rf "${work:?}/${v:?}/examples/agent-workflow/.claude"
  rm -f "${work:?}/${v:?}/examples/agent-workflow/README.md" \
        "${work:?}/${v:?}/docs/benchmarks/agent-integration-p1.md"
done

mkdir -p "$work/treatment/.claude/skills"
cp -r "$repo/examples/agent-workflow/.claude/skills/okf-explore" "$work/treatment/.claude/skills/"
(cd "$work/treatment" && "$work/bin/okf-rs$ext" generate . >/dev/null)

# `claude` is a native program: on Windows (Git Bash/MSYS) it needs C:/...
# paths, not the shell's /tmp/... view of them.
native="$(cd "$work" && (pwd -W 2>/dev/null || pwd))"
cat > "$work/mcp.json" <<EOF
{ "mcpServers": { "okf": { "command": "$native/bin/okf-mcp$ext", "args": ["$native/treatment"] } } }
EOF
cp "$repo/examples/agent-workflow/bench/run.sh" "$repo/examples/agent-workflow/bench/run-all.sh" \
   "$repo/examples/agent-workflow/bench/analyze.js" "$work/"
echo "ready: $work  (next: $work/run-all.sh, then node $work/analyze.js $work)"
