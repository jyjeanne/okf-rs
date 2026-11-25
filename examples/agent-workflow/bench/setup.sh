#!/usr/bin/env bash
# Prepares an A/B exploration benchmark (docs/benchmarks/agent-integration-p1*.md).
#
# usage: setup.sh <okf-rs checkout> <target repo> <target commit> <work dir>
#
# Creates <work dir>/{baseline,treatment} as `git archive` exports of
# <target commit> from <target repo> (no .git, so agents can't read answers
# from history), builds okf-rs and okf-mcp from the okf-rs checkout's working
# tree into <work dir>/bin, generates the bundle in the treatment copy only,
# installs the okf-explore skill there, and writes <work dir>/mcp.json for the
# okf server. For the self-benchmark, <target repo> is the okf-rs checkout.
#
# Both copies lose every `.claude/` folder before the skill is installed: a
# nested skill could otherwise be discovered in the baseline, and an
# untrusted repository's project settings (hooks) must not load. The okf-rs
# pack README (it maps each question to its answer's files) and the P1
# results doc are removed too when present.
set -euo pipefail

okf="$(cd "${1:?okf-rs checkout}" && pwd)"
target="$(cd "${2:?target repo}" && pwd)"
commit="${3:?target commit}"
work="${4:?work dir}"
mkdir -p "$work"
work="$(cd "$work" && pwd)"

if [ -e "$work/baseline" ] || [ -e "$work/treatment" ]; then
  echo "error: $work already holds a run; use an empty directory" >&2
  exit 1
fi

(cd "$okf" && cargo build -q --release -p okf-cli -p okf-mcp)
mkdir -p "$work/bin" "$work/runs" "$work/baseline" "$work/treatment"
ext=""; [ -f "$okf/target/release/okf-rs.exe" ] && ext=".exe"
cp "$okf/target/release/okf-rs$ext" "$okf/target/release/okf-mcp$ext" "$work/bin/"

for v in baseline treatment; do
  git -C "$target" archive "$commit" | tar -x -C "$work/$v"
  find "${work:?}/${v:?}" -type d -name .claude -prune -exec rm -rf {} +
  rm -f "${work:?}/${v:?}/examples/agent-workflow/README.md" \
        "${work:?}/${v:?}/docs/benchmarks/agent-integration-p1.md"
done

mkdir -p "$work/treatment/.claude/skills"
cp -r "$okf/examples/agent-workflow/.claude/skills/okf-explore" "$work/treatment/.claude/skills/"
generated="$(cd "$work/treatment" && "$work/bin/okf-rs$ext" generate . | head -1)"

# `claude` is a native program: on Windows (Git Bash/MSYS) it needs C:/...
# paths, not the shell's /tmp/... view of them.
native="$(cd "$work" && (pwd -W 2>/dev/null || pwd))"
cat > "$work/mcp.json" <<EOF
{ "mcpServers": { "okf": { "command": "$native/bin/okf-mcp$ext", "args": ["$native/treatment"] } } }
EOF

bench="$okf/examples/agent-workflow/bench"
cp "$bench/run.sh" "$bench/run-all.sh" "$bench/analyze.js" "$work/"
cp -r "$bench/tasks" "$work/"
{
  echo "target: $target @ $(git -C "$target" rev-parse "$commit")"
  echo "okf-rs: $(git -C "$okf" rev-parse HEAD)$(git -C "$okf" diff --quiet -- crates Cargo.toml Cargo.lock || echo ' (+ uncommitted crate changes)')"
  echo "bundle: $generated"
} > "$work/run-info.txt"
cat "$work/run-info.txt"
echo "ready: next $work/run-all.sh <tasks/name.tsv>, then node $work/analyze.js $work"
