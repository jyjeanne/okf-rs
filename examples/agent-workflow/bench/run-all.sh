#!/usr/bin/env bash
# Runs every task in a tasks file twice (baseline and treatment), all in
# parallel. usage: run-all.sh <tasks file>
#
# A tasks file has one task per line: `<id><TAB><question>`. Lines starting
# with `#` are ignored. The question is passed to the agent verbatim.
P="$(cd "$(dirname "$0")" && pwd)"
tasks="${1:?tasks file, e.g. tasks/okf-rs.tsv}"
[ -f "$tasks" ] || tasks="$P/$tasks"
: > "$P/runs/status.txt"
while IFS=$'\t' read -r id question; do
  question="${question%$'\r'}"   # tolerate a CRLF checkout
  case "$id" in ''|'#'*) continue ;; esac
  for v in baseline treatment; do
    "$P/run.sh" "$v" "$id" "$question" &
  done
done < "$tasks"
wait
cat "$P/runs/status.txt"
