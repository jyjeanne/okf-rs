#!/usr/bin/env node
// Splits a commit's diff into its test-code hunks and its other hunks, so the
// P2 ground truth can run the commit's new tests against the old code
// (failing tests) and the old tests against the new code (broken tests).
// Test code is classified as in diff-functions.js: a `tests/` directory, a
// `tests.rs` file, a `#[cfg(test)]` module, a function with a test
// attribute, or anything under `fixtures/`. A hunk mixing test and non-test
// lines is reported on stderr and goes to neither patch.
//
// usage: node split-patch.js <repo> <commit> <out dir>
//        writes <out dir>/tests.patch and <out dir>/source.patch
//        (apply with `git apply --unidiff-zero`)
'use strict';
const { execFileSync } = require('child_process');
const fs = require('fs');
const path = require('path');
const { parse } = require('./diff-functions.js');

const [repo, commit, out] = process.argv.slice(2);
if (!repo || !commit || !out) { console.error('usage: node split-patch.js <repo> <commit> <out dir>'); process.exit(2); }
const git = (args) => execFileSync('git', ['-C', repo, ...args], { encoding: 'utf8', maxBuffer: 1 << 28 });
const show = (rev, p) => { try { return git(['show', `${rev}:${p}`]).replace(/\r\n/g, '\n'); } catch { return null; } };

const diff = git(['diff', '-U0', '--no-color', '--no-renames', `${commit}^`, commit]);
const files = [];
let cur = null, hunk = null;
for (const line of diff.split('\n')) {
  if (line.startsWith('diff --git ')) { cur = { header: [line], hunks: [], pre: null, post: null }; files.push(cur); hunk = null; continue; }
  if (!cur) continue;
  const h = /^@@ -(\d+)(?:,(\d+))? \+(\d+)(?:,(\d+))? @@/.exec(line);
  if (h) { hunk = { lines: [line], preLine: +h[1], postLine: +h[3], removed: [], added: [] }; cur.hunks.push(hunk); continue; }
  if (!hunk) {
    cur.header.push(line);
    if (line.startsWith('--- ')) cur.pre = line === '--- /dev/null' ? null : line.slice(6);
    if (line.startsWith('+++ ')) cur.post = line === '+++ /dev/null' ? null : line.slice(6);
    continue;
  }
  if (line === '') continue;
  hunk.lines.push(line);
  if (line.startsWith('-')) hunk.removed.push(hunk.preLine++);
  else if (line.startsWith('+')) hunk.added.push(hunk.postLine++);
}

const tests = [], source = [];
let mixed = 0;
for (const f of files) {
  const p = f.post || f.pre;
  const isRs = p.endsWith('.rs');
  const fixture = /(^|\/)fixtures\//.test(p);
  if (!isRs || fixture) {
    (fixture ? tests : source).push({ header: f.header, hunks: f.hunks });
    continue;
  }
  const pre = f.pre ? parse(show(`${commit}^`, f.pre), f.pre) : null;
  const post = f.post ? parse(show(commit, f.post), f.post) : null;
  const t = [], s = [];
  for (const h of f.hunks) {
    const flags = [...h.removed.map((l) => pre.isTestLine(l)), ...h.added.map((l) => post.isTestLine(l))];
    if (flags.every(Boolean)) t.push(h);
    else if (!flags.some(Boolean)) s.push(h);
    else { mixed++; console.error(`mixed hunk: ${p} ${h.lines[0]}`); }
  }
  if (t.length) tests.push({ header: f.header, hunks: t });
  if (s.length) source.push({ header: f.header, hunks: s });
}

const render = (list) => list.map((f) => [...f.header, ...f.hunks.flatMap((h) => h.lines)].join('\n')).join('\n') + (list.length ? '\n' : '');
fs.mkdirSync(out, { recursive: true });
fs.writeFileSync(path.join(out, 'tests.patch'), render(tests));
fs.writeFileSync(path.join(out, 'source.patch'), render(source));
console.log(`tests.patch: ${tests.length} files, ${tests.reduce((n, f) => n + f.hunks.length, 0)} hunks; source.patch: ${source.length} files, ${source.reduce((n, f) => n + f.hunks.length, 0)} hunks; mixed: ${mixed}`);
