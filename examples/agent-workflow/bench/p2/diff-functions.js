#!/usr/bin/env node
// Text-only ground truth for the P2 modification measurement
// (docs/benchmarks/agent-integration-p2.md, spec §9.2 as amended by D-12).
//
// Maps the hunks of a commit's diff to the Rust functions they change in the
// parent tree. It reads git only: no okf-rs code or output is involved, so
// the result can serve as ground truth for okf-rs's own predictions.
//
// usage: node diff-functions.js <repo> <commit> [--json]
//
// Method: comments, strings and char literals are blanked (offsets and line
// breaks kept), then `fn` items and `impl`/`trait`/`mod` blocks are found by
// matching braces. A changed line belongs to the innermost `fn` whose span
// (from the `fn` keyword to its closing brace, or to `;` without a body)
// contains it; a pure insertion belongs to the function it lands strictly
// inside. Hunks whose changed lines are all blank or comments are dropped.
// Results are a starting point: every mapping is checked by reading the code.
'use strict';
const { execFileSync } = require('child_process');

const main = require.main === module;
const repo = main ? process.argv[2] : null;
const commit = main ? process.argv[3] : null;
const asJson = process.argv.includes('--json');
if (main && (!repo || !commit)) {
  console.error('usage: node diff-functions.js <repo> <commit> [--json]');
  process.exit(2);
}

function git(args) {
  return execFileSync('git', ['-C', repo, ...args], { encoding: 'utf8', maxBuffer: 1 << 28 });
}

const ident = /[A-Za-z0-9_]/;

/** Replaces comments, strings and char literals with spaces (newlines kept). */
function blank(src) {
  const out = src.split('');
  const n = src.length;
  const wipe = (a, b) => { for (let k = a; k < Math.min(b, n); k++) if (out[k] !== '\n') out[k] = ' '; };
  let i = 0;
  while (i < n) {
    const c = src[i], d = src[i + 1];
    if (c === '/' && d === '/') {
      let j = src.indexOf('\n', i); if (j < 0) j = n;
      wipe(i, j); i = j; continue;
    }
    if (c === '/' && d === '*') {
      let depth = 1, j = i + 2;
      while (j < n && depth) {
        if (src[j] === '/' && src[j + 1] === '*') { depth++; j += 2; }
        else if (src[j] === '*' && src[j + 1] === '/') { depth--; j += 2; }
        else j++;
      }
      wipe(i, j); i = j; continue;
    }
    const prevIdent = i > 0 && ident.test(src[i - 1]);
    if (!prevIdent && (c === 'r' || (c === 'b' && d === 'r'))) {
      const m = /^b?r(#*)"/.exec(src.slice(i, i + 64));
      if (m) {
        const close = '"' + m[1];
        let j = src.indexOf(close, i + m[0].length);
        j = j < 0 ? n : j + close.length;
        wipe(i, j); i = j; continue;
      }
    }
    if (c === '"' || (c === 'b' && d === '"' && !prevIdent)) {
      let j = c === '"' ? i + 1 : i + 2;
      while (j < n && src[j] !== '"') j += src[j] === '\\' ? 2 : 1;
      j++;
      wipe(i, j); i = j; continue;
    }
    if (c === '\'') {
      if (d === '\\') {
        let j = i + 2; while (j < n && src[j] !== '\'' && src[j] !== '\n') j++;
        wipe(i, j + 1); i = j + 1; continue;
      }
      // 'x' (one UTF-16 unit) or a surrogate pair; otherwise a lifetime.
      const hi = d && d.charCodeAt(0) >= 0xd800 && d.charCodeAt(0) <= 0xdbff;
      const endQuote = hi ? i + 3 : i + 2;
      if (src[endQuote] === '\'' && d !== '\n') { wipe(i, endQuote + 1); i = endQuote + 1; continue; }
    }
    i++;
  }
  return out.join('');
}

function lineStarts(text) {
  const starts = [0];
  for (let i = 0; i < text.length; i++) if (text[i] === '\n') starts.push(i + 1);
  return starts;
}
function lineOf(starts, off) {
  let lo = 0, hi = starts.length - 1;
  while (lo < hi) { const mid = (lo + hi + 1) >> 1; if (starts[mid] <= off) lo = mid; else hi = mid - 1; }
  return lo + 1; // 1-based
}
/** Offset of the brace matching the `{` at `open`, or text.length. */
function matchBrace(b, open) {
  let depth = 0;
  for (let i = open; i < b.length; i++) {
    if (b[i] === '{') depth++;
    else if (b[i] === '}') { depth--; if (depth === 0) return i; }
  }
  return b.length;
}

function lastSegment(type) {
  let t = type.replace(/\bwhere\b[\s\S]*$/, '').trim();
  t = t.replace(/^(&\s*('\w+\s+)?(mut\s+)?)+/, '').replace(/^dyn\s+/, '');
  // strip generic arguments
  let out = '', depth = 0;
  for (const ch of t) { if (ch === '<') depth++; else if (ch === '>') depth--; else if (depth === 0) out += ch; }
  const segs = out.trim().split('::');
  return segs[segs.length - 1].trim().split(/\s+/)[0];
}

/** Parses one Rust file into fn items and impl/trait/mod blocks. */
function parse(src, path) {
  const b = blank(src);
  const starts = lineStarts(src);
  const srcLines = src.split('\n');
  const blankLines = b.split('\n');
  const blocks = [];
  // impl / trait / mod blocks, item-level (start of line)
  const blockRe = /^[ \t]*(?:pub(?:\([^)]*\))?\s+)?(?:unsafe\s+)?(impl|trait|mod)\b([^{;]*)([{;])/gm;
  let m;
  while ((m = blockRe.exec(b))) {
    if (m[3] !== '{') continue;
    const open = m.index + m[0].length - 1;
    const close = matchBrace(b, open);
    const header = m[2];
    let name;
    if (m[1] === 'impl') {
      let h = header.trim();
      if (h.startsWith('<')) { let depth = 0, k = 0; for (; k < h.length; k++) { if (h[k] === '<') depth++; else if (h[k] === '>') { depth--; if (depth === 0) break; } } h = h.slice(k + 1); }
      // split on ` for ` at angle depth 0
      let depth = 0, forAt = -1;
      for (let k = 0; k < h.length; k++) {
        if (h[k] === '<') depth++; else if (h[k] === '>') depth--;
        else if (depth === 0 && /\sfor\s/.test(h.slice(k, k + 5)) ) { forAt = k; break; }
      }
      name = lastSegment(forAt >= 0 ? h.slice(forAt + 4) : h);
    } else {
      name = (header.trim().match(/^([A-Za-z_][A-Za-z0-9_]*)/) || [])[1] || '?';
    }
    const line = lineOf(starts, m.index + m[0].search(/\S/));
    let cfgTest = false;
    if (m[1] === 'mod') {
      for (let k = line - 2; k >= 0; k--) {
        const t = srcLines[k].trim();
        if (t.startsWith('#[') || t.startsWith('///') || t.startsWith('//')) { if (/#\[cfg\(test\)\]/.test(t)) cfgTest = true; continue; }
        break;
      }
    }
    blocks.push({ kind: m[1], name, line, end: lineOf(starts, close), cfgTest });
  }
  const fns = [];
  const fnRe = /\bfn\s+([A-Za-z_][A-Za-z0-9_]*)/g;
  while ((m = fnRe.exec(b))) {
    let i = m.index + m[0].length, paren = 0, bracket = 0, endOff = -1, hasBody = false;
    for (; i < b.length; i++) {
      const ch = b[i];
      if (ch === '(') paren++; else if (ch === ')') paren--;
      else if (ch === '[') bracket++; else if (ch === ']') bracket--;
      else if (paren === 0 && bracket === 0 && ch === ';') { endOff = i; break; }
      else if (paren === 0 && bracket === 0 && ch === '{') { endOff = matchBrace(b, i); hasBody = true; break; }
    }
    if (endOff < 0) endOff = b.length - 1;
    const line = lineOf(starts, m.index);
    let isTest = false;
    for (let k = line - 2; k >= 0; k--) {
      const t = srcLines[k].trim();
      if (t.startsWith('#[') || t.startsWith('///') || t.startsWith('//')) {
        if (/^#\[(?:[A-Za-z_][A-Za-z0-9_]*::)*test\b/.test(t)) isTest = true;
        continue;
      }
      break;
    }
    fns.push({ name: m[1], line, end: lineOf(starts, endOff), hasBody, isTest });
  }
  const testFile = /(^|\/)tests\//.test(path) || /(^|\/)tests\.rs$/.test(path);
  for (const f of fns) {
    const owners = blocks.filter((x) => x.kind !== 'mod' && x.line <= f.line && f.end <= x.end);
    owners.sort((x, y) => (y.line - x.line));
    f.owner = owners.length ? owners[0].name : '';
    const inTestMod = blocks.some((x) => x.kind === 'mod' && x.cfgTest && x.line <= f.line && f.end <= x.end);
    f.testCode = testFile || inTestMod || f.isTest;
  }
  const isTestLine = (ln) =>
    testFile ||
    blocks.some((x) => x.kind === 'mod' && x.cfgTest && x.line <= ln && ln <= x.end) ||
    fns.some((f) => f.isTest && f.line <= ln && ln <= f.end);
  const isCodeLine = (ln) => (blankLines[ln - 1] || '').trim() !== '';
  /** Innermost fn strictly containing a line (inclusive span). */
  const fnAt = (ln) => {
    let best = null;
    for (const f of fns) if (f.line <= ln && ln <= f.end && (!best || f.line >= best.line)) best = f;
    return best;
  };
  return { fns, blocks, isTestLine, isCodeLine, fnAt, testFile };
}

function show(rev, path) {
  try { return git(['show', `${rev}:${path}`]).replace(/\r\n/g, '\n'); } catch { return null; }
}

const inScope = (p) => p && p.startsWith('crates/') && p.endsWith('.rs') && !/(^|\/)fixtures\//.test(p);

function analyze() {
  const parent = `${commit}^`;
  const diff = git(['diff', '-U0', '-w', '-M', '--no-color', parent, commit, '--', '*.rs']).replace(/\r\n/g, '\n');
  const files = [];
  let cur = null, hunk = null;
  for (const line of diff.split('\n')) {
    if (line.startsWith('diff --git ')) { cur = { pre: null, post: null, hunks: [] }; files.push(cur); hunk = null; continue; }
    if (!cur) continue;
    if (line.startsWith('--- ')) { cur.pre = line === '--- /dev/null' ? null : line.slice(6); continue; }
    if (line.startsWith('+++ ')) { cur.post = line === '+++ /dev/null' ? null : line.slice(6); continue; }
    const h = /^@@ -(\d+)(?:,(\d+))? \+(\d+)(?:,(\d+))? @@/.exec(line);
    if (h) {
      hunk = { preStart: +h[1], preCount: h[2] === undefined ? 1 : +h[2], postStart: +h[3], postCount: h[4] === undefined ? 1 : +h[4], removed: [], added: [] };
      hunk.preLine = hunk.preStart; hunk.postLine = hunk.postStart;
      cur.hunks.push(hunk); continue;
    }
    if (!hunk) continue;
    if (line.startsWith('-')) hunk.removed.push(hunk.preLine++);
    else if (line.startsWith('+')) hunk.added.push(hunk.postLine++);
  }

  const edited = new Map(); // key -> record
  const added = [];
  const unmapped = [];
  let nonTestCodeLines = 0;
  const nonTestFiles = new Set();
  const testFnsTouched = new Set();

  for (const f of files) {
    if (!inScope(f.pre) && !inScope(f.post)) continue;
    const preSrc = f.pre ? show(parent, f.pre) : null;
    const postSrc = f.post ? show(commit, f.post) : null;
    const pre = preSrc !== null ? parse(preSrc, f.pre) : null;
    const post = postSrc !== null ? parse(postSrc, f.post) : null;
    for (const h of f.hunks) {
      const remCode = pre ? h.removed.filter((l) => pre.isCodeLine(l)) : [];
      const addCode = post ? h.added.filter((l) => post.isCodeLine(l)) : [];
      if (remCode.length + addCode.length === 0) continue; // whitespace / comments only
      for (const l of remCode) if (!pre.isTestLine(l)) { nonTestCodeLines++; nonTestFiles.add(f.pre); }
      for (const l of addCode) if (!post.isTestLine(l)) { nonTestCodeLines++; nonTestFiles.add(f.post); }
      // attribute to pre-tree functions
      const targets = new Map();
      if (pre) {
        if (h.removed.length) {
          for (const l of h.removed) { const fn = pre.fnAt(l); if (fn) targets.set(fn, (targets.get(fn) || 0) + (pre.isCodeLine(l) ? 1 : 0)); }
        } else {
          // pure insertion after line preStart: strictly inside fn means line <= preStart < end
          let best = null;
          for (const fn of pre.fns) if (fn.line <= h.preStart && h.preStart < fn.end && (!best || fn.line >= best.line)) best = fn;
          if (best) targets.set(best, 0);
        }
      }
      if (targets.size === 0) {
        unmapped.push({ file: f.pre || f.post, pre: `${h.preStart},${h.preCount}`, post: `${h.postStart},${h.postCount}`,
          text: (postSrc ? addCode.slice(0, 2).map((l) => postSrc.split('\n')[l - 1].trim()) : []).concat(preSrc ? remCode.slice(0, 2).map((l) => '-' + preSrc.split('\n')[l - 1].trim()) : []) });
        continue;
      }
      // added code lines are credited to the (first) function the hunk maps to
      let first = true;
      for (const [fn, n] of targets) {
        const key = `${f.pre}#${fn.owner}::${fn.name}@${fn.line}`;
        const rec = edited.get(key) || { file: f.pre, owner: fn.owner, name: fn.name, line: fn.line, end: fn.end, testCode: fn.testCode, isTest: fn.isTest, changed: 0 };
        rec.changed += n + (first ? addCode.length : 0);
        first = false;
        edited.set(key, rec);
        if (fn.isTest) testFnsTouched.add(key);
      }
    }
    // functions the commit adds (by file, owner, name)
    if (post) {
      const preKeys = new Set(pre ? pre.fns.map((x) => `${x.owner}::${x.name}`) : []);
      for (const fn of post.fns) {
        if (!preKeys.has(`${fn.owner}::${fn.name}`)) {
          added.push({ file: f.post, owner: fn.owner, name: fn.name, line: fn.line, testCode: fn.testCode, isTest: fn.isTest });
          if (fn.isTest) testFnsTouched.add(`+${f.post}#${fn.owner}::${fn.name}`);
        }
      }
    }
  }
  const editedList = [...edited.values()];
  const nonTestEdited = editedList.filter((e) => !e.testCode);
  let target = null;
  for (const e of nonTestEdited) if (!target || e.changed > target.changed) target = e;
  return {
    commit: git(['rev-parse', '--short', commit]).trim(),
    subject: git(['log', '-1', '--format=%s', commit]).trim(),
    edited: editedList,
    added,
    unmapped,
    nonTestCodeLines,
    nonTestFiles: [...nonTestFiles],
    testFunctionsTouched: testFnsTouched.size,
    target,
    eligible: {
      changesExistingNonTestFn: nonTestEdited.length > 0,
      hasTest: testFnsTouched.size > 0,
      fitsOneSession: nonTestCodeLines <= 200 && nonTestFiles.size <= 6,
    },
  };
}

module.exports = { blank, parse, inScope };
if (main) {
  const r = analyze();
  if (asJson) { console.log(JSON.stringify(r, null, 2)); process.exit(0); }
  const fmt = (e) => `${e.file}:${e.line}-${e.end}  ${e.owner ? e.owner + '::' : ''}${e.name}${e.isTest ? ' [test]' : e.testCode ? ' [test code]' : ''}  (${e.changed} lines)`;
  console.log(`${r.commit} ${r.subject}`);
  console.log(`eligible: ${JSON.stringify(r.eligible)}  non-test code lines ${r.nonTestCodeLines} in ${r.nonTestFiles.length} files; test functions added/changed ${r.testFunctionsTouched}`);
  console.log(`target: ${r.target ? fmt(r.target) : '-'}`);
  console.log('edited (existing functions):'); for (const e of r.edited) console.log('  ' + fmt(e));
  console.log('added functions (listed, not scored):'); for (const a of r.added) console.log(`  ${a.file}:${a.line}  ${a.owner ? a.owner + '::' : ''}${a.name}${a.isTest ? ' [test]' : a.testCode ? ' [test code]' : ''}`);
  console.log('hunks outside any function (listed, not scored):'); for (const u of r.unmapped) console.log(`  ${u.file} -${u.pre} +${u.post}  ${u.text.join(' | ')}`);
}
