# Resolver-stability benchmark

**Question:** how much of a diff's relationship-level churn between two resolver versions (or any
two independently generated bundles of the same source) is resolver-only metadata versus a genuine
structural rewire? If that rate stays near zero across a real resolver-version bump, resolver-class
changes can safely default to `ignore` in CI instead of `warn` — and if it isn't near zero, that's
a resolver finding worth reporting on its own.

## Code

- [`okf_analyzer::resolver_only_rate(&DiffReport)`](../../crates/okf-analyzer/src/lib.rs) — the
  classifier: the share of relationship-*pair* changes in a diff that are `ResolverChange`/
  `ProvenanceChange` (same target, only resolver identity/version/confidence differs) rather than a
  genuine `SourceChange` (the target itself differs). Computed directly from `DiffReport`, not from
  `CiSummary`'s aggregate counts, so unrelated concept-level churn elsewhere in the same diff can't
  dilute it.
- [`okf-rs diff --ci`](../../crates/okf-cli/src/main.rs) — surfaces the rate on every run with a
  nonzero `RESOLVER CHANGES` count, comparing two git refs.
- [`okf-rs diff-bundles <bundle-a> <bundle-b>`](../../crates/okf-cli/src/main.rs) — the same
  classified report, but for two bundle directories already on disk instead of two git refs. This
  is the one that actually makes the resolver-version measurement runnable: `diff`'s two-git-ref
  comparison has nothing to check out when the two bundles came from the *identical* source
  snapshot, analyzed under two different resolver versions.

## Run it

```sh
# Generate the same source snapshot under two resolver versions (PATH controls
# which `rust-analyzer` binary `--lsp` spawns):
PATH=/path/to/rust-analyzer-1.90.0/bin:$PATH okf-rs generate . --lsp --no-cache --output /tmp/bundle-old
PATH=/path/to/rust-analyzer-1.94.1/bin:$PATH okf-rs generate . --lsp --no-cache --output /tmp/bundle-new

okf-rs diff-bundles /tmp/bundle-old /tmp/bundle-new
```

Two toolchains can be installed side by side via `rustup`, without needing GitHub release downloads
(useful in a network-restricted environment): `rustup toolchain install <version>` then
`rustup component add rust-analyzer --toolchain <version>`.

**Control experiment, before trusting any cross-version delta**: `--lsp` resolution isn't
guaranteed deterministic run-to-run even under one fixed resolver version (see Results below). Run
`okf-rs generate . --lsp --check-determinism` (twice, independently, same version) first, so a
cross-version difference isn't mistaken for a real version-caused one when it's actually baseline
indexing noise.

## Results so far

Run for real against this repository's own source (1153 concepts, 54 files), comparing
`rust-analyzer` 1.90.0 (1159e78, 2025-09-14) against 1.94.1 (e408947, 2026-03-25):

```
❌ SOURCE CHANGES: 2
⚠️  RESOLVER CHANGES: 1292
   (99.8% of relationship-level changes in this diff were resolver-only)
```

1292 of 1294 relationship-level changes were purely `resolver_version` metadata — real evidence
that `resolver_changes: "ignore"` is a defensible policy choice for a project willing to accept the
confound below. The 2 remaining `SourceChange` entries were both directions of one edge
(`cmd_check_determinism → Project::load`) and looked, at first, like a genuine cross-version
resolution disagreement — until the control experiment above
(`generate --lsp --check-determinism`) showed `--lsp` resolution on this codebase isn't fully
deterministic run-to-run even holding the resolver version fixed: 1.94.1 disagreed with itself on
2/2 repeated runs (different call sites each time), and 1.90.0 disagreed with itself on 1/3. That
means the one real `SourceChange` found between versions can't be confidently attributed to the
version bump at all — it's at least as well explained by that baseline noise. **The more solidly
reproducible finding here is the non-determinism itself, present in both tested versions, not a
1.90.0-vs-1.94.1 semantic disagreement.**

This same run caught a real, now-fixed display bug: `render_ci_report`'s resolver-only-rate line
originally rounded at zero decimal places, so this genuine 99.8454...% displayed as a bare "100%" —
indistinguishable from an actually-clean diff. See
[`docs/improvement-plan-provenance-diff.md`](../../docs/improvement-plan-provenance-diff.md)'s
Phase G, "`diff-bundles`, and the real measurement it made possible," for the full writeup,
including the exact commands run and the regression test that reproduces this ratio.

**Open**: this measurement hasn't been run on a corpus other than okf-rs's own source, or across a
wider resolver-version gap. A different codebase's ambiguous-call density could plausibly show a
different rate — that's exactly the kind of project-specific number this benchmark exists to let a
team collect for itself rather than assume from one worked example.
