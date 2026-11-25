#!/usr/bin/env node
// Scores one P2 task (docs/benchmarks/agent-integration-p2.md): matches the
// concepts plan-change predicted against the pre-registered ground truth.
//
// usage: node score-p2.js <bundle dir> <plan-change.json> <truth.json>
//
// truth.json: { "A": [{ "file", "name", "line", "end", "from": "edit"|"test" }],
//               "broken": [{ "file", "name" }] }
// A concept matches a ground-truth function when the file and the name are
// the same and the line ranges overlap (the concept's range is read from the
// `resource: <file>#L<a>-L<b>` line of its bundle file). Every pair printed
// is checked by hand.
'use strict';
const fs = require('fs');
const path = require('path');

const [bundle, planPath, truthPath] = process.argv.slice(2);
if (!bundle || !planPath || !truthPath) { console.error('usage: node score-p2.js <bundle dir> <plan-change.json> <truth.json>'); process.exit(2); }
const plan = JSON.parse(fs.readFileSync(planPath, 'utf8'));
const truth = JSON.parse(fs.readFileSync(truthPath, 'utf8'));

function locate(id) {
  const file = path.join(bundle, `${id}.md`);
  if (!fs.existsSync(file)) return null;
  const text = fs.readFileSync(file, 'utf8');
  const res = /^resource:\s*(.+?)#L(\d+)-L(\d+)\s*$/m.exec(text);
  const title = /^title:\s*(.+?)\s*$/m.exec(text);
  if (!res || !title) return null;
  return { id, file: res[1], line: +res[2], end: +res[3], name: title[1].replace(/^['"]|['"]$/g, '') };
}

const P = [plan.target.id, ...plan.affected_concepts].map((id) => locate(id) || { id, missing: true });
const tests = plan.tests.map((id) => locate(id) || { id, missing: true });
const same = (c, f) => !c.missing && c.file === f.file && c.name === f.name && c.line <= f.end && f.line <= c.end;
const sameTest = (c, t) => !c.missing && c.file === t.file && c.name === t.name;

let hit = 0, hitEdit = 0, nEdit = 0, hitTest = 0, nTest = 0;
console.log(`target: ${plan.target.id}`);
console.log(`|P| = ${P.length} (target + ${plan.affected_concepts.length} affected); tests field: ${plan.tests.length}`);
for (const f of truth.A) {
  const m = P.find((c) => same(c, f));
  if (f.from === 'edit') { nEdit++; if (m) hitEdit++; } else { nTest++; if (m) hitTest++; }
  if (m) hit++;
  console.log(`  ${m ? 'HIT ' : 'MISS'} [${f.from}] ${f.file}:${f.line}-${f.end} ${f.name}${m ? '  <- ' + m.id + ` (L${m.line}-L${m.end})` : ''}`);
}
const recall = truth.A.length ? hit / truth.A.length : NaN;
console.log(`recall = ${hit}/${truth.A.length} = ${recall.toFixed(3)}  (edit ${hitEdit}/${nEdit}, test callees ${hitTest}/${nTest})`);
let unpredicted = 0;
for (const t of truth.broken || []) {
  const m = tests.find((c) => sameTest(c, t));
  if (!m) unpredicted++;
  console.log(`  broken test ${m ? 'PREDICTED  ' : 'UNPREDICTED'} ${t.file} ${t.name}${m ? '  <- ' + m.id : ''}`);
}
console.log(`unpredicted broken tests = ${unpredicted} of ${(truth.broken || []).length}`);
const missing = P.filter((c) => c.missing).map((c) => c.id);
if (missing.length) console.log(`warning: no bundle file or resource line for ${missing.join(', ')}`);
console.log(JSON.stringify({ recall, hit, size: truth.A.length, hitEdit, nEdit, hitTest, nTest, unpredicted, broken: (truth.broken || []).length }));
