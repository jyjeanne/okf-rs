// Applies spec §9.2 to each transcript: distinct files opened (Read; bundle
// files under knowledge/ reported separately), skill invoked before the first
// Read, source-first violation (a Read before the first explore call),
// tool-call counts, tokens, cost. Saves each final answer for grading.
const fs = require("fs");
const path = require("path");
const dir = path.join(process.argv[2], "runs");

function rel(p) {
  const s = String(p || "").split("\\").join("/");
  for (const marker of ["/baseline/", "/treatment/"]) {
    const i = s.toLowerCase().lastIndexOf(marker);
    if (i >= 0) return s.slice(i + marker.length);
  }
  return s;
}

const rows = [];
for (const t of ["E1", "E2", "E3", "E4", "E5"]) {
  for (const v of ["baseline", "treatment"]) {
    const id = `${t}-${v}`;
    const calls = [];
    let result = null;
    let init = null;
    for (const line of fs.readFileSync(path.join(dir, id + ".jsonl"), "utf8").split("\n")) {
      if (!line.trim()) continue;
      const m = JSON.parse(line);
      if (m.type === "system" && m.subtype === "init") init = m;
      if (m.type === "assistant") {
        for (const b of m.message.content || []) if (b.type === "tool_use") calls.push(b);
      }
      if (m.type === "result") result = m;
    }
    const indexed = calls.map((c, i) => ({ c, i }));
    const reads = indexed.filter(({ c }) => c.name === "Read");
    const isBundle = ({ c }) => rel(c.input.file_path).startsWith("knowledge/");
    const project = reads.filter((r) => !isBundle(r));
    const bundle = reads.filter(isBundle);
    const files = [...new Set(project.map(({ c }) => rel(c.input.file_path)))];
    const firstRead = project.length ? project[0].i : Infinity;
    const skillIdx = calls.findIndex((c) => c.name === "Skill" && JSON.stringify(c.input).includes("okf-explore"));
    const exploreIdx = calls.findIndex((c) => c.name === "mcp__okf__explore");
    const count = (n) => calls.filter((c) => c.name === n).length;
    const mcp = calls
      .filter((c) => c.name.startsWith("mcp__okf__"))
      .map((c) => c.name.slice("mcp__okf__".length) + (c.input.relation ? "/" + c.input.relation : ""));
    const u = result.usage || {};
    const tokens =
      (u.input_tokens || 0) + (u.cache_creation_input_tokens || 0) + (u.cache_read_input_tokens || 0) + (u.output_tokens || 0);
    fs.writeFileSync(path.join(dir, id + ".answer.md"), result.result || "");
    rows.push({
      id,
      files: files.length,
      fileList: files,
      bundleReads: bundle.length,
      grep: count("Grep"),
      glob: count("Glob"),
      mcp,
      skillBeforeRead: v === "treatment" ? skillIdx >= 0 && skillIdx < firstRead : null,
      sourceFirst: v === "treatment" ? firstRead < (exploreIdx >= 0 ? exploreIdx : Infinity) : null,
      tokens,
      output: u.output_tokens,
      cost: +(result.total_cost_usd || 0).toFixed(3),
      turns: result.num_turns,
      secs: Math.round((result.duration_ms || 0) / 1000),
      model: init && init.model,
    });
  }
}
fs.writeFileSync(path.join(dir, "metrics.json"), JSON.stringify(rows, null, 2));
for (const r of rows) {
  console.log(
    `${r.id.padEnd(13)} files=${String(r.files).padStart(2)} bundleReads=${r.bundleReads} grep=${r.grep} glob=${r.glob} ` +
      `skillBeforeRead=${r.skillBeforeRead} sourceFirst=${r.sourceFirst} tokens=${r.tokens} cost=$${r.cost} turns=${r.turns} ${r.secs}s ${r.model}`
  );
  console.log(`    files: ${r.fileList.join(", ")}`);
  console.log(`    mcp: ${r.mcp.join(", ")}`);
}
const sum = (v, k) => rows.filter((r) => r.id.endsWith(v)).reduce((a, r) => a + r[k], 0);
const fb = sum("baseline", "files");
const ft = sum("treatment", "files");
console.log(`\nTOTAL files baseline=${fb} treatment=${ft} ratio=${(ft / fb).toFixed(2)}`);
console.log(
  `TOTAL tokens baseline=${sum("baseline", "tokens")} treatment=${sum("treatment", "tokens")} | ` +
    `cost baseline=$${sum("baseline", "cost").toFixed(2)} treatment=$${sum("treatment", "cost").toFixed(2)}`
);
