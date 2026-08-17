# Improvement plan: resolver versioning, provenance-aware graph diff, and MCP tool-selection benchmarking

A technical implementation plan for the second round of external review feedback recorded
verbatim in
[`docs/feedback/2026-08-provenance-graph-diff-review.md`](feedback/2026-08-provenance-graph-diff-review.md).
Structured the same way [`docs/improvement-plan.md`](improvement-plan.md) is — phases with a
concrete proposed model, unit tests, and acceptance criteria, plus a cost/benefit GO/no-go table
— because the point of that review was to make this progressively implementable and
automatically verifiable, not another prose roadmap.

**Delivery status:** tracked in
[`ROADMAP.md`](../ROADMAP.md#improvement-plan--provenance-depth-graph-diff--mcp-tool-selection).
All six phases have now shipped in full: A (resolver version), B (provenance-aware graph diff), C
(`diff --ci` policy), D (reproducibility metadata), E (tool-selection benchmark, offline harness
*and* live-endpoint wiring), and F (golden fixture dataset).

## 0. What's already shipped, and what's actually new here

The reviewer's phases 1-3 and 9 largely restate work this project already delivered in the
["AI-native platform maturity" improvement plan](../ROADMAP.md#improvement-plan--ai-native-platform-maturity-community-feedback)
(the reviewer's own *first* round of feedback, from one week earlier). Re-proposing it here would
duplicate, not extend, shipped work — so this plan opens with the gap check that keeps it honest,
then only phases the genuinely new remainder.

| Proposal item | Status | Where |
|---|---|---|
| Edge-level provenance (`origin`/`resolver`) | ✅ Shipped | `Relationship::{resolved_by, confidence}` — [`crates/okf-parser/src/lib.rs:290`](../crates/okf-parser/src/lib.rs) |
| Distinguish syntactic vs. semantic edges | ✅ Shipped | `Confidence::{Exact, Semantic}` — same struct; `exact` *is* "syntax-derived", `semantic` *is* "LSP-resolved" (ROADMAP explicitly rejected a second, redundant `kind` field) |
| Provenance survives serialization/deserialization, backward-compatible | ✅ Shipped | `okf_parser::bundle::parse_relationship_entry` accepts both the old bare-string target and the new `{target, resolved_by, confidence}` mapping — every bundle ever generated stays valid unmodified |
| Provenance-metadata validation | ✅ Shipped | `okf_validator::check_relationship_provenance` — an unrecognized `confidence` or empty `resolved_by` is a validator error |
| Resolver identity tracking | ✅ Shipped | `resolved_by` carries the resolver's own binary name (`rust-analyzer`, `pyright-langserver`) for an `--lsp`-resolved edge, `tree-sitter` otherwise |
| **Resolver *version* tracking** | ✅ Shipped (this plan's Phase A) | `Relationship::resolver_version`, from `okf_lsp::LspClient::server_version()` — see `ROADMAP.md` |
| Consolidate specialized `graph_*` tools | ✅ Shipped | 13 tools collapsed into one `graph(relation=...)` tool, `okf-mcp` 0.3.0 — [`crates/okf-mcp/src/tools.rs`](../crates/okf-mcp/src/tools.rs) |
| MCP fixed-cost / break-even benchmark | ✅ Shipped | `okf-mcp --benchmark` — schema tokens, naive-vs-MCP token comparison, break-even query count — [`crates/okf-mcp/src/benchmark.rs`](../crates/okf-mcp/src/benchmark.rs) |
| CI validation mode | ✅ Shipped | `okf-rs validate --ci`, `generate --check-determinism`, `generate --check-fresh` |
| **Provenance-aware graph diff** (source vs. resolver vs. semantic change classes) | ✅ Shipped (this plan's Phase B) | `okf_analyzer::{RelationshipChangeKind, diff_relationships}` — see `ROADMAP.md` |
| **`okf-rs diff --ci` policy with source/resolver/metadata classification** | ✅ Shipped (this plan's Phase C) | `okf-rs diff --ci`, `okf_analyzer::ci_summary`, `okf_core::config::DiffPolicy` — see `ROADMAP.md` |
| **Artifact-level reproducibility metadata** (generator name/version, source revision) | ✅ Shipped (this plan's Phase D) | `okf_core::git::head_revision`, `okf_generator::write_root_index`'s `generator_name`/`generator_version`/`source_revision` — see `ROADMAP.md` |
| **Specialized-vs-consolidated tool-*selection-accuracy*** benchmark (real model calls) | ✅ Shipped (this plan's Phase E) | `okf_mcp::tool_selection_benchmark` (offline question set/fixture/scoring) + `okf_mcp::tool_selection_live` (live OpenAI-compatible tool-calling runner, `okf-mcp --benchmark-tool-selection`) — see `ROADMAP.md` |
| Golden fixture dataset for provenance/diff/MCP | ✅ Shipped (this plan's Phase F) | `tests/fixtures/{provenance,diff,mcp}/` — see `ROADMAP.md` |

Everything below phases only the six rows this table originally marked ❌ (two have since shipped
— see the phase headings below and `ROADMAP.md` for current status). A `ProvenanceOrigin` enum
(`TreeSitter`/`Lsp`/
`Manual`/`Derived`) as the reviewer's Phase 1 proposes is **deliberately not built**: `resolved_by`
is already a free-form string (a hand-edited bundle can carry `resolved_by: hand-edited` today,
exercised by an existing test at `crates/okf-parser/src/lib.rs:692`), and the ROADMAP already
rejected adding enum variants nothing produces (`inferred`/`unresolved` confidence values) as
"exactly the kind of unpopulated field this project avoids elsewhere." `Manual`/`Derived` origins
have no producer in this codebase today; introducing them speculatively would be the same mistake
in a new enum instead of an old one.

## 1. Guiding principle carried over

Provenance is already first-class for *what* produced an edge (`resolved_by`) and *how certain*
it is (`confidence`). What's missing is *which version* of that resolver produced it, and — the
higher-leverage half — a diff/CI layer that actually *uses* provenance to separate "the code
changed" from "the tool that reads the code changed," which is the whole reason the reviewer's
worked example (`rust-analyzer` 1.88 → 1.89 producing the same edge) matters.

---

## 2. Phase A — Resolver version ✅ Shipped

### Objective

Record which version of a resolver produced a `--lsp`-resolved edge, so two edges naming the same
resolver at different versions are distinguishable — the concrete, missing piece behind the
reviewer's `rust-analyzer 1.88 → 1.89` example.

### Proposed model

Extend the existing `Relationship` struct (not a new type — this is one more optional field
alongside `resolved_by`/`confidence`, the same shape those two already took):

```rust
// crates/okf-parser/src/lib.rs
pub struct Relationship {
    pub kind: RelationKind,
    pub target: String,
    pub target_display: String,       // unchanged
    pub resolved_by: String,          // unchanged
    pub confidence: Confidence,       // unchanged
    pub resolver_version: Option<String>,  // new
}
```

Rendered only when present (omitted entirely for `tree-sitter` edges and for any bundle that
predates this field, exactly like `resolved_by`/`confidence` were introduced as optional-shaped
additions to a previously bare-string target):

```yaml
relationships:
  calls:
    - target: functions/decode_jwt
      resolved_by: rust-analyzer
      confidence: semantic
      resolver_version: 1.88.0
```

Sourced from `okf-lsp`: the `initialize` handshake's response already carries the server's own
`serverInfo.version` (part of the LSP spec, distinct from `okf_lsp::is_available`'s existence
check) — `okf-lsp` currently discards it after the handshake; this phase captures it and threads
it the same path `resolved_by` already travels, from `okf-lsp`'s client through
`okf-analyzer`'s `ResolvedEdge` to the `Relationship` pushed onto both concepts.

### Tests

Unit tests:
- `okf-lsp`: capturing `serverInfo.version` from a real `initialize` response (both a server that
  reports it and one that omits the field — LSP doesn't mandate `serverInfo`).
- `okf-parser`: `parse_relationship_entry` round-trips a `resolver_version` key; a bundle with no
  `resolver_version` key parses with `None` (not an error) — same backward-compatibility shape
  `resolved_by`/`confidence` already have three fields deep into this struct now.
- `okf-generator`: the existing round-trip test (`crates/okf-generator`, the one that already
  asserts a `rust-analyzer`/`Confidence::Semantic` edge survives write-then-read-back) gains a
  `resolver_version: Some("1.88.0".into())` case.
- `okf-validator`: a `resolver_version` present without `resolved_by` naming a real resolver (i.e.
  `resolved_by: tree-sitter` with a version set) is a new validator warning — versioning a
  resolver that was never actually invoked is a real inconsistency to flag, the same posture
  `check_relationship_provenance` already takes toward a malformed `confidence`.

Integration:
- Two `--lsp` runs of the same fixture project against two different installed `rust-analyzer`
  binaries (or a stubbed `okf-lsp` client in tests, since pinning two real toolchain versions in
  CI is its own cost) produce edges whose `resolver_version` differs while `target`/`resolved_by`/
  `confidence` are identical — this is the fixture Phase B's diff classification tests build on.

### Acceptance criteria

- `resolver_version` is optional, omitted for `tree-sitter` edges, and every bundle generated
  before this phase remains valid and unaffected (`okf-rs validate` on an old bundle: unchanged).
- `okf-rs generate --check-determinism` still reports deterministic on the tree-sitter-only path
  (this field is never populated there, so nothing new to compare).
- Two edges naming the same resolver at different versions are structurally distinguishable by
  reading the bundle alone, with no need to re-run the resolver to find out.

---

## 3. Phase B — Provenance-aware graph diff ✅ Shipped

### Objective

Make `okf_analyzer::diff` (and `okf-rs diff`) distinguish source-level, resolver-level, and
confidence-level relationship changes instead of one undifferentiated "changed," using data
Phase A now makes available.

### Why this is the real gap, precisely

`relationship_set` (`crates/okf-analyzer/src/lib.rs:391-397`, as of Phase A — line numbers here
drift with every phase that touches these files; treat them as pointers at time of writing, not
a promise) already exists specifically to make
`diff` ignore relationship *ordering* — but it does that by projecting each relationship down to
`(RelationKind, target)`, which also erases `resolved_by`/`confidence`/`resolver_version`
entirely. That's the right behavior for *today's* `diff` (a plain concept-level added/removed/
changed report has no provenance concept to preserve), but it means:

1. A resolver-version bump that changes *which* target an ambiguous call resolves to is
   indistinguishable from a genuine source-level rewire — both just show up as `Changed`.
2. A resolver-version bump that changes *only* `resolved_by`/`resolver_version` on an
   already-resolved edge (same target, same kind) is invisible to `diff` today — it isn't even
   detected as a change, because `relationship_set` never looks at those fields.

Both directions matter for CI trust: (1) is a false-negative risk (an agent's blast-radius/impact
analysis, built on `diff`, could miss that the actual call target changed), and (2) is exactly
the reviewer's "you don't know whether the resolver changed" scenario, currently unobservable at
all from `okf-rs diff`'s output.

### Proposed model

Add a richer change classification alongside (not replacing) the existing `ChangeKind`:

```rust
// crates/okf-analyzer/src/lib.rs
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RelationshipChangeKind {
    /// Same (kind, target) set on both sides — no change at all.
    Unchanged,
    /// The resolved target (or relation kind) actually differs — a real
    /// structural rewire, regardless of what produced either side.
    SourceChange,
    /// Same (kind, target) set; `resolved_by` and/or `resolver_version`
    /// differ, `confidence` does not — the pure "same tool, different
    /// version" case.
    ResolverChange,
    /// Same (kind, target) set; `confidence` differs, `resolved_by`/
    /// `resolver_version` do not — e.g. an edge `--lsp` disambiguation
    /// touched without changing which server resolved it.
    ConfidenceChange,
    /// Same (kind, target) set; `resolved_by`/`resolver_version` *and*
    /// `confidence` both differ together — the case a source-level change
    /// elsewhere in the project causes (a previously-unambiguous call
    /// becoming ambiguous, or vice versa, flips a call between
    /// `tree-sitter`/`exact` and a real resolver/`semantic` even though
    /// *this* edge's own target never moved). Kept distinct from
    /// `ResolverChange`/`ConfidenceChange` rather than folded into either
    /// — a CI policy that only asks "did resolver metadata change" can
    /// still treat this the same way it treats `ResolverChange` (see
    /// Phase C), but the classification itself shouldn't hide that two
    /// fields moved together, not one.
    ProvenanceChange,
}
```

`ChangedConcept` (already exists at `crates/okf-analyzer/src/lib.rs`) gains a
`relationship_changes: Vec<(RelationKind, String /* target */, RelationshipChangeKind)>`-shaped
field (owned `String`, not a borrowed `&str` — `ChangedConcept`/`DiffReport`/`ImpactReport`/
`ImpactedConcept` and every consumer of them, `impact()`/`review()`/`explain()`/the CLI/MCP
rendering, are fully owned and lifetime-free today; a borrowed field here would force a lifetime
parameter through all of them for no real benefit, since `Relationship::target` is already an
owned `String` one clone away) — computed by a new `diff_relationships(before: &Concept, after:
&Concept)` helper that pairs relationships by `(kind, target)` (the same key `relationship_set`
already uses to detect presence) and classifies each pair, plus any relationship present on only
one side as `SourceChange` (an add/remove is unambiguously structural, provenance or not).
`Unchanged` pairs are computed but filtered out before being stored in `relationship_changes` —
the field only ever holds entries worth reporting.

A concept's overall `ChangeKind` stays exactly as it is today (`Added`/`Removed`/`Changed` —
`diff`'s existing top-level classification, which callers like `impact()`/`review` already
depend on, is untouched); `relationship_changes` is *additional* detail attached to a `Changed`
concept, not a replacement axis. This keeps every existing consumer (`impact`, `review`,
`explain`) source-compatible — they can ignore the new field entirely and see identical behavior.

### Tests

Mirror the reviewer's five worked scenarios directly as fixtures, since they're exactly the
right minimal cases:

1. **Added edge** — `A -> B` becomes `A -> B, A -> C` → `RelationshipChangeKind::SourceChange` for
   the new `(Calls, C)` pair, concept-level `ChangeKind::Changed`.
2. **Removed edge** — inverse of (1) → `SourceChange` for the removed pair.
3. **Source-level rewire, same resolver** — `Foo -> Bar` (`tree-sitter`) becomes `Foo -> Baz`
   (`tree-sitter`) → the `Bar` pair is `SourceChange` (removed), the `Baz` pair is `SourceChange`
   (added) — never conflated into one "resolver changed" report, since the target genuinely
   changed.
4. **Resolver-only change** — `Foo -> Bar` (`rust-analyzer` 1.88) becomes `Foo -> Bar`
   (`rust-analyzer` 1.89) → exactly one `ResolverChange` entry for the `(Calls, Bar)` pair, **not**
   a remove+add pair — this is the test that would fail today (the edge doesn't appear in `diff`'s
   output at all, since `relationship_set` never notices).
5. **Resolver changes which target resolves** — `Foo -> Bar` (`rust-analyzer` 1.88) becomes
   `Foo -> Baz` (`rust-analyzer` 1.89) → `SourceChange` for both the removed `Bar` and added `Baz`
   pairs (target genuinely differs — this is scenario 3's shape, not scenario 4's, even though the
   resolver version also happens to differ; a diff that reported this as only a resolver change
   would hide a real call-graph rewire).
6. **Ambiguity newly introduced, same real target** — `Foo -> Bar` (`tree-sitter`/`exact`) becomes
   `Foo -> Bar` (`rust-analyzer`/`semantic`, some `resolver_version`) — the shape a same-named
   function being added elsewhere in the project produces: the target didn't move, but the call
   that used to be unambiguous now needs `--lsp` to resolve. → exactly one `ProvenanceChange` entry
   for the `(Calls, Bar)` pair — not `ResolverChange` (confidence also moved) and not
   `ConfidenceChange` (resolved_by also moved). This is the scenario the plan's first draft had no
   variant for at all.

Plus:
- Order-independence: reuses the existing `diff_ignores_relationship_order` test's fixture shape,
  now also asserting classification is insensitive to relationship-list order.
- A concept `Changed` purely by signature (no relationship difference at all) still reports an
  empty `relationship_changes` — unaffected by this phase.

### Acceptance criteria

- A resolver-version-only change on an otherwise-identical edge is classified as
  `ResolverChange`, never as a remove-then-add pair.
- A genuine target rewire is always `SourceChange`, even when the resolver/version also happens
  to differ alongside it (scenario 5) — resolver metadata never masks a real structural change.
- `resolved_by`/`resolver_version` and `confidence` changing *together* on an unchanged target
  (scenario 6) is always `ProvenanceChange`, never silently absorbed into `ResolverChange` or
  `ConfidenceChange` — every `(kind, target)` pair present on both sides lands in exactly one of
  the five variants, with no combination of field changes left unclassified.
- Every existing `okf_analyzer::diff`/`impact`/`review` test passes unchanged — this phase adds
  detail, it doesn't change what `Added`/`Removed`/`Changed`/blast-radius/risk scoring report.

---

## 4. Phase C — `okf-rs diff --ci` policy ✅ Shipped

### Objective

Surface Phase B's classification as a CI-usable command with deterministic exit codes,
distinguishing failure-worthy source/semantic changes from warn-worthy resolver-only changes —
the reviewer's worked CLI example.

### Design

Extends `okf-rs diff <ref-a> <ref-b> [path]` (`cmd_diff`, `crates/okf-cli/src/main.rs:1482`) with
a `--ci` flag, following the exact pattern `cmd_validate`'s `ci: bool` already established for
`okf-rs validate` rather than inventing a new flag convention:

```
$ okf-rs diff previous-ref current-ref --ci

❌ SOURCE CHANGES: 3
⚠️  RESOLVER CHANGES: 1
ℹ️  CONFIDENCE CHANGES: 2

exit code: 1
```

```
$ okf-rs diff previous-ref current-ref --ci
⚠️  RESOLVER CHANGES: 4
exit code: 0
```

Policy is configurable via `okf.toml` (extending the existing minimal `Config` struct in
`crates/okf-core/src/config.rs`, which today only has `output`), not hardcoded, since the
reviewer's own spec calls this out explicitly ("the policy should be configurable"):

```toml
# okf.toml
output = "knowledge"

[diff]
resolver_changes = "warn"   # "warn" (default) | "fail" | "ignore"
confidence_changes = "ignore"  # "warn" | "fail" | "ignore" (default)
```

`ProvenanceChange` (Phase B) follows `resolver_changes`' policy, not a third setting of its own —
it's a resolver-provenance change (that it also touches `confidence` doesn't make it less of one),
and `--ci` users shouldn't need a fourth knob for a combination case.

**Counting unit, spelled out precisely** (the thing the reviewer's own worked example leaves
implicit): each category counts *edges*, not concepts, and a concept's own addition/removal counts
as edges too, one per relationship it carries — never as "1" regardless of how many relationships
that concept has:

- `SOURCE CHANGES` = (for each added/removed *concept*, `max(1, relationships.len())` — a concept
  with no relationships of its own still counts as `1`, since its own existence is the change and
  there's no edge count to fall back on; a concept with `N ≥ 1` relationships counts as `N`, not
  `N + 1`) + (every `Changed` concept whose signature differs, counted as `1` — **implemented, not
  in the original spec**: a pure signature-only change has no relationship difference at all, so
  it was invisible to the formula as first drafted here, which would have let `--ci` silently pass
  a PR that only changed a function's signature; `okf_analyzer::ci_summary` closes this) + (every
  `RelationshipChangeKind::SourceChange` entry within a `Changed` concept).
- `RESOLVER CHANGES` = every `RelationshipChangeKind::ResolverChange` **and** `ProvenanceChange`
  entry within a `Changed` concept (added/removed concepts have no "resolver changed" — either the
  target changed too, which is `SOURCE CHANGES`' job, or the concept doesn't exist to compare).
- `CONFIDENCE CHANGES` = every `RelationshipChangeKind::ConfidenceChange` entry within a `Changed`
  concept.

This means the worked example above (`❌ SOURCE CHANGES: 3`) could be one added concept with three
relationships (3, from the `max(1, N)` rule), or a signature change on one `Changed` concept (1)
plus a two-relationship source rewire on another (2), or any other combination summing to 3 — the
number is a total, not a count of distinct concepts, and the CLI's non-`--ci` human-readable
output (unaffected by this phase, see
below) is where a reader goes to see which.

Added/removed concepts and `RelationshipChangeKind::SourceChange` are **always** failures under
`--ci` — this isn't configurable, matching the reviewer's own table ("Added source edge → 1",
non-negotiable) and this project's existing stance that `validate --ci` treats warnings as errors
by default rather than offering to silence real problems.

### Tests

CLI end-to-end tests (`crates/okf-cli/tests/e2e.rs`), asserting real exit codes through the
compiled binary against two real git refs of a synthetic fixture repo — the reviewer's own table,
verbatim:

| Scenario | Expected exit code |
|---|---|
| No changes | 0 |
| Metadata-only (`confidence_changes = "ignore"`, default) | 0 |
| Resolver-only (`resolver_changes = "warn"`, default) | 0 |
| Added source edge | 1 |
| Removed source edge | 1 |
| Semantic (target) change | 1 |
| Mixed source + resolver changes | 1 |
| Resolver-only with `resolver_changes = "fail"` in `okf.toml` | 1 |

Plus: `--ci` without a `okf.toml` `[diff]` section uses the documented defaults (warn on resolver,
ignore on confidence-only) — the absence of config is a valid, tested state, not an error.

### Acceptance criteria

- Exit codes are deterministic and match the table above exactly, verified through the real
  compiled binary against real git refs (not just the classification unit tests from Phase B).
- The policy is genuinely configurable (`okf.toml`), and a project with no config gets sane,
  documented defaults rather than silently picking a policy no one can see.
- Human-readable (non-`--ci`) `okf-rs diff` output is unaffected for anyone not passing the flag —
  additive, matching this project's established pattern for every prior `--ci`/`--check-*` flag.

---

## 5. Phase D — Artifact-level reproducibility metadata ✅ Shipped

### Objective

Make a generated bundle self-describing (generator version, source revision) — deliberately
scoped down from the reviewer's proposal to avoid the determinism trap `Concept::generated_at`
already documents and avoids.

### The determinism tension, and how this phase resolves it

`Concept::generated_at` exists in the schema today and is **always `None`** — the doc comment at
`crates/okf-parser/src/lib.rs:427-433` (as of Phase A) is explicit that stamping it with
wall-clock time "would make the bundle non-reproducible for identical source, violating the
project's determinism principle." The reviewer's Phase 8 proposes exactly that field, at the
artifact level (`generated_at: "2026-08-14T12:00:00Z"`) plus a generator name/version and source
revision. Adding it naively would break `generate --check-determinism` (two runs a second apart
would "disagree") and `generate --check-fresh` (every re-run would report staleness from the
timestamp alone, regardless of whether the source changed) — a real regression on two features
this project already shipped specifically to *guarantee* determinism.

This phase ships the parts that are genuinely reproducible and skips the one that isn't — and
does it by extending a mechanism that already exists rather than inventing a new one.
**`okf-generator::write_bundle` already writes an `okf_version` YAML frontmatter block into the
bundle-root `index.md`** (`crates/okf-generator/src/lib.rs:310-314`), and `okf_validator::
check_index_frontmatter` (`crates/okf-validator/src/lib.rs:471`) already permits — and is the
*only* place that permits — an arbitrary YAML mapping there, validating just the `okf_version` key
and ignoring anything else present. That's precisely artifact-level metadata's natural home,
already shipped and already validated; a separate `manifest.md` file was this plan's first draft
and is deliberately **not** what ships, because every other `.md` file in a bundle (anything that
isn't `index.md`) is required by `okf_validator::check_frontmatter`
(`crates/okf-validator/src/lib.rs:145`) to carry a recognized concept `type:`, which a
metadata-only file doesn't have — a `manifest.md` would be a validator error under the very
tooling this plan wants to keep clean:

```rust
// crates/okf-generator/src/lib.rs — extending the existing root-index frontmatter writer
struct RootIndexFrontmatter {
    okf_version: &'static str,       // unchanged, already written today
    generator_name: &'static str,    // new: "okf-rs"
    generator_version: &'static str, // new: env!("CARGO_PKG_VERSION")
    #[serde(skip_serializing_if = "Option::is_none")]
    source_revision: Option<String>, // new: `git rev-parse HEAD` when known — see below
}
```

Deliberately **no `generated_at` timestamp field at all**. `generator_version` and `okf_version`
are exactly reproducible across two runs on identical source; `source_revision` is exactly
reproducible given the same `HEAD` (and is genuinely useful CI-audit information the reviewer's
example also names). A timestamp is not reproducible by definition and isn't needed for the two
stated use cases (auditability, "which okf-rs built this") — CI's own build metadata (job
timestamp, commit SHA) already covers "when," and duplicating it inside the artifact only invites
exactly the determinism regression this project has twice now (`--check-determinism`,
`--check-fresh`) built dedicated tooling to prevent.

**`source_revision`'s "dirty working tree" case needs new logic, not an existing pattern to
match.** An earlier draft of this phase claimed this "matches `okf-rs diff`'s own existing
git-optional posture" — checked against the code, that's not accurate: every git interaction
`diff`/`impact`/`review` make (`crates/okf-cli/src/main.rs:1389-1447`, `WorktreeCheckout`/
`git_repo_root`) is either "no git repo at all" (a real existing fallback) or a fresh
`git worktree add --detach <ref>` checkout of a named ref, which is clean by construction — there
is no "is the working tree dirty" check anywhere in this codebase to match. `generate` (what this
phase actually instruments) commonly runs against the live working tree, which genuinely can be
dirty, so this phase needs to add that check itself — `git diff --quiet HEAD` (exit code 0 =
clean) alongside the existing `git rev-parse HEAD` — rather than pointing at nonexistent
precedent. `source_revision` is `None` for "not a git repo" (existing precedent) and — the new
part — the commit SHA is still recorded even when the tree is dirty, since "generated from commit
`X`, possibly with local modifications" is more useful to a CI reader than silently omitting the
revision entirely; a bundle generated from a dirty tree is exactly the case CI-audit tooling most
wants a `source_revision` to still point at *something*.

### Tests

- `okf-generator`: the extended root-`index.md` frontmatter round-trips through a parser the same
  way `read_bundle`/`check_index_frontmatter` already read it back — `okf_version`/
  `generator_name`/`generator_version`/`source_revision` all survive write-then-read, and
  `okf-validator` reports no new issues on the result (confirming this doesn't reopen the
  `manifest.md` collision the design section above ruled out).
- `okf-cli`: `generate --check-determinism` still reports "Deterministic" on a fixture repo after
  this phase ships (the root `index.md`'s new fields are either excluded from the byte-diff, or —
  the stronger, preferred guarantee — genuinely identical across both runs since nothing in them
  varies run to run for the same `HEAD`/binary).
- `okf-cli`: two clean-checkout `generate` runs of the same commit, in two separate temp
  directories, produce a byte-identical root `index.md` (not just byte-identical concept files) —
  this is the actual reproducibility claim, tested directly rather than assumed.
- `source_revision` is `None` (not an error) only when run outside a git repository — the new
  `git diff --quiet HEAD` dirty check (see above) still records `HEAD`'s SHA when the tree is
  dirty, it doesn't fall back to `None`; a dedicated test covers the dirty-but-still-recorded case
  specifically, since it's the one behavior this phase can't borrow from existing precedent.

### Acceptance criteria

- The root `index.md`'s frontmatter carries `okf_version` (unchanged) plus the new
  `generator_name`/`generator_version`/`source_revision`, survives serialization, and
  `source_revision` is exactly the commit `generate` actually ran against (dirty tree or not).
- No second bundle-root file is introduced — `check_frontmatter`'s "every non-index `.md` file
  needs a concept `type:`" rule is never at risk of firing on generated metadata, because there's
  no new file for it to fire on.
- `generate --check-determinism` and `generate --check-fresh` are both unaffected — no new false
  positive from this phase, verified directly rather than assumed given the risk called out above.
- No wall-clock timestamp is added to the bundle anywhere this phase touches — this is a
  deliberate, documented scope cut from the reviewer's proposal, not an oversight, and is called
  out as such in the frontmatter-writing code's own doc comment so a future contributor doesn't
  "helpfully" add one back.

---

## 6. Phase E — Specialized-vs-consolidated MCP tool-*selection* benchmark ✅ Shipped

### Objective

The reviewer's Phase 10/11 asks for an empirical comparison between many specialized `graph_*`
tools and one consolidated tool. **The design decision itself already shipped** — `okf-mcp` 0.3.0
collapsed 13 tools into `graph(relation=...)`, as a breaking change, based on the *schema-size*
argument alone (documented in `ROADMAP.md`: 18→6 tools, ~22% schema-byte reduction). What was
never measured is the reviewer's actual concern: whether a model picks the right `relation` value
inside one consolidated tool as reliably as it previously picked the right tool name — the
tradeoff the reviewer explicitly flagged as a cost of consolidation, not just a benefit.

This phase is a **retrospective validation** of a shipped decision, not a design experiment
deciding between two live options — which changes its shape: there's no "old" tool surface still
running to A/B against live, so the comparison has to reconstruct it.

### Design

A new, standalone benchmark harness (`okf-mcp --benchmark-tool-selection`, the same main-binary
one-shot side door `--benchmark` already uses — no separate `benches/` script needed) that:

1. Defines a fixed set of representative natural-language questions, one per `graph` relation
   (mirroring the reviewer's own examples): "Who calls `Foo`?", "What does `Foo` call?", "What's
   the shortest path from `Foo` to `Bar`?", "Is `Foo` part of the public API?", "Does the call
   graph have any cycles?", etc. — **14** questions, one per relation the consolidated tool exposes
   today (`callers`, `callees`, `path`, `explain`, `api`, `cycles`, `modules`, `isolated`, `stats`,
   `layers`, `domains`, `communities`, `patterns`, `features` — see `crates/okf-mcp/src/tools.rs`'s
   `relation` enum), not 13: `explain` (the "Explainability" roadmap item) shipped *after* the
   `graph_*` consolidation, so it's a genuinely new relation, not one of the 13 that got merged.
2. For the **consolidated** design (what's actually shipped): feeds each question, plus the real
   `graph` tool's schema from `tools/list`, to a model and records which `relation` value it
   chose and the final answer's correctness against the bundle's known-correct answer. Single-shot
   only (`tool_choice: "auto"`, one request per question) — no multi-turn retry loop: a model that
   picks wrong or declines to call anything is scored as wrong/errored on that one attempt, not
   re-prompted. Simpler than the "follow-up/retry" tracking this section originally sketched, and
   an honest one: a retry loop would need its own scoring policy (does a correct-on-retry count as
   correct?) this plan never specified, so shipping a well-defined single-shot measurement beat
   shipping an underspecified multi-shot one.
3. For the **specialized** design (reconstructed, not live): feeds the same question against a
   hand-built schema list of the 13 old `graph_*` tool names/descriptions (parameters reconstructed
   from the consolidated `graph` tool's own per-relation shape; descriptions lifted from that same
   tool's per-relation bullet points, not invented, so a specialized-design loss can't be an
   artifact of a worse description) to the same model, recording which tool name it picked — **with
   one explicit carve-out**: `explain` has no historical specialized-tool counterpart to
   reconstruct (no `graph_explain` ever existed), so the 14th question is scored for the
   consolidated design only, excluded from the specialized design's 13-question sample rather than
   either dropped silently or answered against an invented tool name.
4. Reports, per design: tool/relation-selection accuracy, final-answer accuracy, total tokens
   (prompt + completion, from the endpoint's own `usage` object), and latency — 13 comparable
   questions plus 1 consolidated-only question, not 14 directly comparable ones.

### Live-endpoint configuration (resolved)

Every other benchmark in this codebase (`okf-mcp --benchmark`, real-world-project benchmarking)
is deliberately **offline and LLM-free** — a stated design choice, not an oversight (see
`benchmark.rs`'s own doc comment: "no LLM call or tokenizer dependency involved"). A real
tool-selection-accuracy measurement, unlike schema-size accounting, *cannot* be done that way —
"which tool would a model pick" is not decidable without actually calling one. This phase resolved
that tension as follows:

- **Not run in normal CI** (no API key available, non-deterministic, real cost per run) — an
  opt-in `--benchmark-tool-selection` flag, documented as such, the same way `--enrich`'s network
  dependency is already opt-in and never exercised by default test runs.
- **Pluggable model endpoint via environment variables**, not CLI flags —
  `OKF_BENCHMARK_MODEL_BASE_URL`/`OKF_BENCHMARK_MODEL`/`OKF_BENCHMARK_MODEL_API_KEY` — mirroring
  `--enrich`'s existing `OKF_ENRICH_*` pattern (any OpenAI-compatible `chat/completions` endpoint:
  Ollama, LM Studio, OpenAI, or a compatible cloud provider), but a **separate** set of variables:
  the model worth measuring for tool-selection accuracy need not be the model this server would
  otherwise use for description enrichment or semantic search.
- **Reports results with the same honesty** `okf-mcp --benchmark`'s existing RAG-comparison gap
  already models: sample size, which model was used, and that a single model's tool-selection
  behavior is not a universal claim about "LLMs in general" — see `LiveReport::render`.

### Tests

- The question set and expected-relation mapping are themselves unit-tested (a question intended
  to map to `callers` really does have a known-correct `graph_callers`-shaped answer against the
  benchmark fixture bundle) — this is checkable without any model call, and guards the benchmark's
  own fixtures from silently drifting out of sync with what `graph`'s relations actually do.
- The harness's scoring logic (did the model's tool call match the expected relation; did the
  final answer match the known-correct value) is unit-tested against canned/mocked model
  responses — both a correct and an incorrect response, so the scoring code itself is verified
  independent of any real model's actual behavior.
- An end-to-end run against a real (or mocked, for CI) endpoint is a separate, explicitly
  non-default target — not part of `cargo test --workspace`.

### Acceptance criteria

**The harness half:**
- The 14-question set, one per `graph` relation, exists with a known-correct `relation`/answer
  each — verified directly against a real fixture bundle through the actual `tools::call`
  dispatch, with zero model calls (`okf_mcp::tool_selection_benchmark`).
- The question set's relations are checked against the `graph` tool's own live schema, not a
  hardcoded copy — a future relation addition/rename can't leave it silently stale.
- The pre-0.3.0 specialized-tool-name mapping is reconstructed (`graph_<relation>`), with
  `explain`'s missing counterpart handled as an explicit, tested case, not a silent gap.
- Scoring logic (`scores_correctly`) is a real, independently unit-tested function — not inlined
  logic a future live-endpoint runner would have to duplicate or guess at.

**The live-endpoint half — `okf_mcp::tool_selection_live`, `okf-mcp --benchmark-tool-selection`:**
- A real OpenAI-compatible tool-calling client (`tools`/`tool_choice`, not plain chat completion —
  see that module's doc comment on why `okf_enrich::EnrichClient` doesn't fit), configured through
  `OKF_BENCHMARK_MODEL_BASE_URL`/`OKF_BENCHMARK_MODEL`/`OKF_BENCHMARK_MODEL_API_KEY` — a deliberately
  separate set of variables from `--enrich`'s `OKF_ENRICH_*`, so the benchmarked model and the
  enrichment model can be configured independently.
- Both designs actually run: the consolidated design against the real `graph` tool's live schema
  (`tools::list()`), the specialized design against 13 reconstructed `graph_<relation>` schemas
  (`explain` excluded, no pre-0.3.0 counterpart) — reusing the harness's own
  `questions()`/`scores_correctly()`/`specialized_tool_name()`, not a second copy of them.
- Tool-selection accuracy, final-answer accuracy, total tokens, and total latency reported per
  design (`DesignReport`), plus a per-question `[WRONG]`/`[ERROR]` breakdown — with the model name
  and sample size stated up front and an explicit "not a universal claim about LLMs in general"
  caveat, per this phase's own honesty requirement.
- Never run by `cargo test --workspace`: this module's own tests cover the HTTP request/response
  parsing layer against a hand-rolled mock server (mirroring `okf_enrich::test_support`), not real
  model behavior. Verified end to end against a real, separate-process HTTP server (not just the
  in-process Rust mock) three ways: `--benchmark-tool-selection` with no env vars set reports a
  clear `OKF_BENCHMARK_MODEL_BASE_URL` error; against a server that answers every question
  correctly, both designs score 100%/100% (14/14 consolidated, 13/13 specialized); against a
  server deliberately wrong on one question and silent (no tool call) on another, the report
  correctly drops to 12/14 and 11/13 with both a `[WRONG]` and an `[ERROR]` line naming the exact
  question and reason — proving the scoring, dispatch, and error-reporting paths all work against
  a real socket, not just canned unit-test data.

---

## 7. Phase F — Golden fixture dataset ✅ Shipped

### Objective

Give Phases A-E (and the existing provenance/diff/CI tests they extend) a shared, discoverable
fixture location, per the reviewer's proposed layout — genuinely useful for regression detection
independent of whether every phase above ships in full.

### Layout

```
tests/
├── fixtures/
│   ├── provenance/
│   │   ├── tree-sitter.md       # a bundle concept with a tree-sitter-resolved edge
│   │   ├── lsp.md                # ...with an --lsp-resolved edge, resolver_version set
│   │   └── mixed.md              # both kinds of edge on one concept
│   ├── diff/
│   │   ├── unchanged/            # {before,after}/ git-worktree-shaped concept sets
│   │   ├── added-edge/
│   │   ├── removed-edge/
│   │   ├── resolver-change/      # Phase B scenario 4
│   │   └── semantic-change/      # Phase B scenario 5
│   └── mcp/
│       ├── specialized/          # reconstructed pre-0.3.0 tools/list schema + question set
│       └── consolidated/         # current graph(relation=...) schema + same question set
```

This is a **relocation and consolidation** of fixtures the test suites for Phases A-E already
need to construct inline (as `Concept`/`Relationship` builders in Rust test code today) — not new
scope on its own. Rust unit tests can still build fixtures inline where a hand-written struct is
clearer than a file (most of Phase B's classification tests, for instance); this directory is for
fixtures that are more naturally data than code — real two-ref diff scenarios, and the MCP
question/schema sets Phase E needs in a form a non-Rust benchmark script could also load.

### Tests / acceptance criteria

- Every fixture under `tests/fixtures/diff/` is exercised by at least one `okf-analyzer` or
  `okf-cli` e2e test — an unused fixture is worse than none (silent drift, nothing catches it
  going stale) — enforced by a small test asserting every subdirectory has a corresponding test
  reference (grep-based, not a runtime check).
- Fixtures are inputs to tests, not outputs golden-diffed against — this project's existing
  determinism guarantees (`--check-determinism`) are what make "the same fixture always produces
  the same result" trustworthy already; this phase doesn't need to invent a separate
  golden-file-diffing mechanism on top.

---

## 8. Integration test: the full pipeline

One end-to-end test (`crates/okf-cli/tests/e2e.rs`), extending the existing e2e suite rather than
opening a new test binary, exercising:

```
source (two commits, one with a resolver-version bump only)
  → okf-rs generate --lsp (both commits)
  → provenance (resolved_by/confidence/resolver_version present)
  → OKF serialization / deserialization (round-trip via read_bundle)
  → okf-rs diff --ci (Phase B/C classification: ResolverChange, not Added+Removed)
  → exit code 0 (default policy: resolver changes warn, don't fail)
  → okf-mcp graph relation=callers (still answers correctly against the new bundle)
```

Requires a real language server in the test environment (this repo's CI already has
`rust-analyzer` available — the same LSP disambiguation tests from Phase 2 of the main roadmap
already depend on it) or a stubbed `okf-lsp` client reporting two different `serverInfo.version`
values across two invocations, for environments without one.

**Acceptance criteria:** provenance survives every step of the pipeline, and the `--ci` exit code
at the end reflects the *resolver-only* nature of the only real difference between the two
commits — this is the single test that proves the whole plan's value proposition, not just each
phase's isolated pieces.

---

## 9. Backward compatibility

Every phase above is additive to an already-optional, already-backward-compatible schema:

- Phase A: `resolver_version` is `Option<String>`, omitted for every edge that doesn't have one
  (which is every edge in every bundle generated before this phase, and every `tree-sitter` edge
  after it).
- Phase B: `RelationshipChangeKind` is new output detail on `ChangedConcept`; nothing about
  `DiffReport`'s existing `added`/`removed`/`changed`/`ChangeKind` shape changes.
- Phase C: `--ci` is a new, opt-in flag on `okf-rs diff`; bare `okf-rs diff` output is unchanged.
- Phase D: the new frontmatter fields are additions to the root `index.md`'s existing, already-
  optional `okf_version` block; a bundle whose root `index.md` predates this phase (no
  `generator_name`/`generator_version`/`source_revision` keys) is still a completely valid bundle
  to every other command — `check_index_frontmatter` already tolerates unknown/missing keys there.
- Phase E/F: new tooling and fixtures, touching nothing existing consumes.

An `okf-rs validate` run against a pre-Phase-A bundle reports exactly what it reports today —
"absence of provenance/resolver-version/reproducibility metadata means unknown, never invalid" is
already this project's established posture (see the existing bare-string-target
backward-compatibility path in
`okf_parser::bundle::parse_relationship_entry`), and every phase here follows it, not just states it.

---

## 10. Cost/benefit and GO/no-go

Effort sizing follows the same convention `docs/improvement-plan.md` established: **S** = a few
focused hours to a day in one crate; **M** = multi-day, 2-3 crates and their tests; **L** = new
crate or real design risk. Benefit is scored against how directly the item closes a gap §0's
table marks ❌, versus how speculative it is.

| # | Item | Effort | Benefit | Risk | Decision |
|---|---|---|---|---|---|
| A | Resolver version (`Relationship.resolver_version`) | S | High — the one concrete missing fact behind the reviewer's own worked example; nothing downstream (diff, CI) works without it | Low — one more optional field on a struct that's already grown this way twice | **GO** — ✅ shipped |
| B | Provenance-aware diff classification | M | High — the single biggest named gap; closes both the false-negative (rewire hidden by matching resolver names) and false-positive (invisible resolver-only change) directions at once | Medium — pairing relationships by `(kind, target)` across two snapshots needs care around duplicate targets under different kinds and concepts that both add and remove edges in the same diff; needs the six worked scenarios as real regression tests, not just the happy path | **GO**, sequenced after A — ✅ shipped |
| C | `okf-rs diff --ci` policy | S-M | High — this is what actually makes B usable in a pipeline, matching the reviewer's own CLI example line for line | Low — mirrors `validate --ci`'s already-shipped, already-tested flag pattern; the only new surface is the `okf.toml` `[diff]` section | **GO**, sequenced after B — ✅ shipped |
| D | Artifact-level reproducibility metadata (no timestamp) | S | Medium — genuinely useful for CI audit ("which okf-rs, which commit, built this bundle"), but scoped down from the reviewer's ask specifically to avoid the determinism regression a naive implementation would cause | Medium — the risk isn't the feature, it's a future contributor "fixing" the missing timestamp back in; mitigated by testing `--check-determinism`/`--check-fresh` directly against this phase and documenting the cut in the module itself | **GO**, with the no-timestamp scope cut as a hard constraint, not a suggestion — ✅ shipped |
| E | Specialized-vs-consolidated tool-*selection* benchmark | M-L | Medium — genuinely validates (or falsifies) a decision already shipped and already justified on schema-size grounds alone; real value is closing that specific "did we trade selection accuracy for schema size and never check" open question | Medium-High — the only item in this plan requiring a live LLM call, breaking this project's until-now-consistent "every benchmark is offline and deterministic" posture; must stay explicitly opt-in/non-CI to avoid becoming a flaky, costly, silently-skipped test | **CONDITIONAL GO** — build the harness and question set (fully testable without a model) first; only wire up a real endpoint call once someone is prepared to own interpreting a non-deterministic result, matching how `--enrich`'s own network dependency was scoped in from day one — ✅ both halves shipped: harness first, live-endpoint runner (`okf-mcp --benchmark-tool-selection`) once the endpoint/env-var decision was made |
| F | Golden fixture dataset | S | Low-Medium — organizational, not a new capability; mainly pays for itself by giving Phases B/C's own tests a less ad hoc home | Low — pure relocation/addition, no behavior change | **GO**, opportunistically alongside B/C rather than as a blocking prerequisite — ✅ shipped, and additively (not a relocation of existing tests, which stayed as-is) |
| G | Failure-mode split, requests-per-answered-question, resolver-only rate (Medium review follow-up) | S | High — directly fixes a real scoring blind spot in E (one accuracy number hiding two different-cost failure modes) and gives C's own policy knob an actual instrument to justify itself with, both from data already collected | Low — pure derived metrics/rendering on already-shipped structs, no new fields, no new network dependency | **GO** — ✅ shipped |

### Net recommendation

Shipped, in order: **A → B → C → D**, all unconditional **GO**s — this sequence alone delivers the
reviewer's core thesis (resolver identity *and version*, provenance-aware diff, CI policy) with
no unresolved conditions, at a combined cost of roughly S+M+(S-M)+S. **E**'s conditional-GO was
honored precisely as scoped, in two steps: the harness/question-set/scoring half (fully offline,
model-free) shipped first; the live-endpoint half — a real OpenAI-compatible tool-calling client,
configured via `OKF_BENCHMARK_MODEL_*` env vars, actually exercising both designs against a real
model — shipped once that deliberate decision (which endpoint, env vars vs. flags) was made,
closing the one item in this plan that broke with every existing benchmark's offline-and-
deterministic posture. **F** shipped in between, exactly as its own low-priority, non-blocking
scoring in this table always said it should: real fixture files for Phase A's provenance shapes and
Phase B's five diff scenarios, plus a portable JSON export of Phase E's question set (the one piece
that genuinely couldn't be used outside Rust before), each backed by a real test proving the
fixture round-trips through the actual code path it claims to exercise — additive alongside the existing
inline Rust-native tests from B/C/E, which stayed exactly where they were, not a relocation of
them.

**Where this plan stands:** every phase this document proposed has shipped in full — A, B, C, D,
E (both the harness and, once its own conditional verdict's deliberate decision was made, the
live-endpoint runner), and F. Phase G below is a follow-up driven by a later round of external
review of Phases B/C/E specifically, not part of the original proposal.

---

## 11. Phase G — Benchmark-scoring and CI-signal follow-up (Medium review, August 2026) ✅ Shipped

### Objective

A third round of external review (recorded verbatim in
[`docs/feedback/2026-08-tool-consolidation-benchmark-review.md`](feedback/2026-08-tool-consolidation-benchmark-review.md))
raised two concrete gaps in what Phases B/C/E shipped, not new features:

1. Phase E's live benchmark reports one selection-accuracy percentage, but the two designs fail in
   different registers — a specialized tool name a model hallucinates fails loudly (no matching
   tool, or a schema the model can't satisfy), while a wrong `relation` value inside the
   consolidated `graph` tool still produces a well-formed call that returns real, just-wrong, data.
   Collapsing both into one "wrong" count flatters whichever design happens to fail in the cheap
   (loud) register more often.
2. Phase E's own break-even reasoning (and `benchmark.rs`'s separate token-based break-even) never
   expressed cost in the unit the reviewer argues actually matters: tool schemas are re-serialized
   into every request in a session, so consolidation's savings scale predictably with request
   count — but the cost of a wrong selection doesn't scale with tokens at all, since one bad
   selection costs a full extra round trip regardless of how large the conversation prefix is.
3. Phase B/C's `RelationshipChangeKind::ResolverChange`/`CiSummary.resolver_changes` classification
   makes "did the resolver change something?" observable, but nothing computed the empirical rate
   a project would actually need to decide whether `DiffPolicy::resolver_changes` can safely
   default to `ignore` instead of `warn`.

### What shipped

**Failure-mode split** (`crates/okf-mcp/src/tool_selection_live.rs`): a `FailureMode` enum
(`Correct`/`LoudFailure`/`SilentWrong`) and `QuestionOutcome::failure_mode()`, derived from data
each outcome already carried (`error.is_some()` is exactly the loud/silent boundary — no new field
needed, since every path that fails to produce a usable call already set `error`, and a
well-formed call to the wrong tool/relation never does). `DesignReport` gained
`loud_failures()`/`silent_wrong()` counts, and `render()`'s per-question lines are now tagged
`[LOUD-FAIL]`/`[SILENT-WRONG]` instead of one undifferentiated `[WRONG]`/`[ERROR]` pair, plus a
breakdown line under the headline selection-accuracy percentage.

**`requests_per_answered_question()`** (same module): `1 / final_answer_accuracy`, i.e. how many
requests this design spent per question it actually answered correctly — the reviewer's proposed
unit, computed from data the benchmark already collects rather than a new measurement. `None` when
no question in the sample was answered correctly (a rate has nothing meaningful to report there;
the failure-mode breakdown above is the more useful number in that case). Rendered alongside the
existing token/latency totals.

**`okf_analyzer::resolver_only_rate(&DiffReport)`** (`crates/okf-analyzer/src/lib.rs`): the share
of relationship-*pair* changes in a diff that were resolver-only (`ResolverChange`/
`ProvenanceChange`). `None` on an empty diff or one with no `Changed` concepts carrying a
relationship-level change at all. `okf-rs diff --ci` prints this rate alongside the
`RESOLVER CHANGES` count (`crates/okf-cli/src/main.rs`'s `render_ci_report`) whenever
`resolver_changes > 0`, so a project gets the exact number the reviewer's "measure on your own
corpus" ask calls for on every CI run that has any resolver-only changes to report — rather than
needing a bespoke script to compute it.

**Correction during review** (a further round of feedback on this same Phase G, also recorded in
`docs/feedback/2026-08-tool-consolidation-benchmark-review.md`): this function originally shipped
as a `CiSummary` method, deriving its rate from `CiSummary`'s own three aggregate counters
(`resolver_changes / (source_changes + resolver_changes + confidence_changes)`). That's a real bug,
not just a documentation gap — `CiSummary::source_changes` also folds in whole-concept adds/removes
and signature-only changes (see `ci_summary`), none of which are relationship-*pair* changes at
all. Any concept churn elsewhere in the same diff inflated that denominator and understated the
resolver-only rate for reasons that have nothing to do with resolver behavior — exactly the kind of
silent bias that would lead a project to keep `resolver_changes: warn` when the real, relationship-
level rate was actually near 100%. Fixed by reading `report.changed[..].relationship_changes`
directly (the same source `ci_summary` itself reduces from) instead of going through `CiSummary` at
all: `okf_analyzer::resolver_only_rate` is now a free function taking the full `&DiffReport`, and
`okf-cli`'s `cmd_diff_ci` (which already has the full report in scope before reducing it to a
`CiSummary`) computes it there and threads it into `render_ci_report` as a plain `Option<f64>`
parameter, keeping that function exactly as unit-testable against hand-built values as it was
before.

This project still hasn't run the actual measurement across a real `rust-analyzer` minor-version
bump on its own corpus (that needs two pinned toolchain installs, the same cost noted against
Phase A's own integration-test scenario) — `resolver_only_rate` is the instrument, not the
measurement itself. `DiffPolicy::resolver_changes`'s own default stays `Warn` (not `Ignore`) until
a project actually collects that number and finds it consistently near zero.

**Percentages on the failure-mode breakdown**: `LiveReport::render()`'s loud-failure/silent-wrong
counts are now also rendered as a percentage of the design's sample size (e.g. `1 (7%)`), not just
a bare count — the accuracy lines already reported percentages, and the failure-mode breakdown
existed specifically so the two failure costs could be compared at a glance, which a bare count
doesn't support as directly across sample sizes of different size.

### Tests

- `tool_selection_live::tests`: `failure_mode_distinguishes_silent_wrong_from_loud_failure` (all
  three `FailureMode` variants against hand-built outcomes),
  `design_report_counts_failure_modes_and_requests_per_answered_question` (a 4-outcome design —
  two correct, one silent-wrong, one loud-failure — asserts both counts and the resulting `2.0`
  requests-per-answered-question), `requests_per_answered_question_is_none_when_nothing_was_answered_correctly`.
  The pre-existing `live_report_render_includes_the_model_and_both_designs` test is updated to
  assert the new `[SILENT-WRONG]` tag rather than the old undifferentiated `[WRONG]`.
- `okf-analyzer::tests`: `resolver_only_rate_is_none_on_an_empty_diff`,
  `resolver_only_rate_reports_the_share_of_relationship_level_changes` (a mixed diff — one genuine
  rewire, one resolver-only pair — asserts the rate is exactly `1/3`, not just "some resolver
  changes happened"), `resolver_only_rate_is_not_diluted_by_unrelated_whole_concept_churn` (the
  regression test for the bug described above — a diff with an added whole concept *and* a
  resolver-only relationship pair still reports `Some(1.0)`, not diluted by the added concept),
  plus an existing resolver-only-change test extended to assert `Some(1.0)`.
- `okf-cli::diff_ci_tests`: `resolver_only_change_reports_its_share_of_relationship_level_changes`,
  `resolver_only_rate_is_below_100_percent_when_source_changes_also_present`,
  `no_rate_line_is_rendered_when_resolver_only_rate_is_none` — `render_ci_report` now takes the
  rate as an explicit parameter, so these lock in only the rendering, not the (now separately
  tested) rate computation itself.

### Acceptance criteria

- A silent-wrong outcome and a loud-failure outcome are never counted in the same bucket, in either
  the live benchmark's aggregate counts or its per-question rendered lines.
- `requests_per_answered_question()` is derivable from data the benchmark already collects (no new
  network calls or fields), and reports `None` rather than dividing by zero when nothing was
  answered correctly.
- `resolver_only_rate` is visible on every `okf-rs diff --ci` run with a nonzero resolver-change
  count, with no extra flag needed to see it, and is unaffected by concept-level churn (adds,
  removes, signature-only changes) elsewhere in the same diff.
- Every existing Phase A-F test keeps passing unchanged — this phase adds detail and derived
  metrics to already-shipped surfaces, the same additive posture every earlier phase in this
  document took (§9).

### `diff-bundles`, and the real measurement it made possible

`resolver_only_rate` and `okf-rs diff --ci` are the *instrument* Phase G shipped; running the actual
measurement across two real resolver versions on a real corpus — the reviewer's explicit ask — needs
a way to compare two bundles generated from the identical source snapshot under two different
resolver versions, which `okf-rs diff`'s two-git-ref comparison can't express at all (the source
never changed; there's no second ref to check out). `okf-rs diff-bundles <bundle-a> <bundle-b>`
closes that gap: it reads two bundle directories directly off disk (`okf_parser::read_bundle` on
each side) and renders the exact same `--ci`-style classified report `diff --ci` does — reusing
`render_ci_report` unchanged, just fed `Option<f64>` computed from a `DiffReport` built from two
already-generated bundles instead of two freshly-analyzed git refs. Not limited to the
resolver-version use case: any two independently generated bundles of conceptually the same project
(`--lsp` on vs. off, two different `okf-rs` versions, ...) compare the same way.

**The measurement, run for real** (not just described): two real `rust-analyzer` installs via
`rustup toolchain install`/`rustup component add rust-analyzer` (1.90.0 and 1.94.1 — GitHub release
downloads are blocked by this environment's egress policy, but `static.rust-lang.org`, which `rustup`
itself uses, isn't), `okf-rs generate . --lsp --no-cache` against this repository's own 1153-concept
source snapshot under each, then `okf-rs diff-bundles` between the two resulting bundles:

```
❌ SOURCE CHANGES: 2
⚠️  RESOLVER CHANGES: 1292
   (99.8% of relationship-level changes in this diff were resolver-only)

exit code: 1
```

1292 of 1294 relationship-level changes between the two versions were purely `resolver_version`
metadata — real, if modest, evidence that `resolver_changes: "ignore"` is a defensible policy choice
for a project willing to accept the confound described next. The 2 remaining `SourceChange` entries
(both directions of one edge: `cmd_check_determinism → Project::load`) looked, at first, like a
genuine cross-version resolution disagreement — until the obvious control experiment
(`okf-rs generate --lsp --check-determinism`, which re-runs analysis twice *independently under the
same resolver version* and diffs byte-for-byte) showed `--lsp` resolution on this codebase isn't
fully deterministic run-to-run even holding the resolver version fixed: 1.94.1 disagreed with itself
on 2/2 repeated runs (on two different call sites each time), and 1.90.0 disagreed with itself on
1/3. That means the one real `SourceChange` found between versions can't be confidently attributed to
the version bump at all — it's at least as well explained by this baseline indexing noise, already
named as a known limitation in this ROADMAP's Phase 2 section, now quantified rather than just
asserted. The more solidly reproducible finding here is the non-determinism itself, present in both
tested versions, not a 1.90.0-vs-1.94.1 semantic disagreement.

**A real bug this run caught**: `render_ci_report`'s resolver-only-rate line originally formatted at
zero decimal places (`{:.0}%`), which rounds a genuine 99.8454...% up to a bare "100%" —
indistinguishable from an actually-clean diff with zero source changes, exactly the distinction this
number exists to preserve. Found only by running the real tool against real data, not by any of the
hand-picked round-number test cases (100%, 50%) already in place; fixed to one decimal place, with a
regression test (`resolver_only_rate_renders_at_one_decimal_place_not_rounded_to_a_bare_100_percent`)
that reproduces the exact 1292/1294 ratio from this run.

### Tests (continued)

- `okf-cli::diff_ci_tests::resolver_only_rate_renders_at_one_decimal_place_not_rounded_to_a_bare_100_percent`
  — the regression test for the rounding bug above.
- `okf-cli` e2e (`tests/e2e.rs`): `standalone_binary_diff_bundles_reports_the_resolver_only_rate_from_a_real_fixture_pair`
  runs the real compiled binary against Phase F's own checked-in
  `tests/fixtures/diff/resolver-change/{before,after}/` fixture (no live resolver install needed —
  the fixture already encodes a resolver-version-only change);
  `standalone_binary_diff_bundles_reports_no_changes_for_identical_bundles` covers the
  no-difference-at-all case. The two-real-resolver-versions measurement itself isn't a repository
  test (no more feasible to pin two `rust-analyzer` installs in CI than Phase A's own integration
  scenario already noted) — it was run once, by hand, exactly as this section describes, and
  `diff-bundles` is what a project would run to repeat it.

## References

- [`docs/feedback/2026-08-provenance-graph-diff-review.md`](feedback/2026-08-provenance-graph-diff-review.md) — the raw feedback this plan distills
- [`docs/feedback/2026-08-community-roadmap-review.md`](feedback/2026-08-community-roadmap-review.md) — the reviewer's first-round feedback, already delivered
- [`docs/feedback/2026-08-tool-consolidation-benchmark-review.md`](feedback/2026-08-tool-consolidation-benchmark-review.md) — the reviewer's third-round feedback (Medium), driving Phase G
- [`ROADMAP.md` — Improvement Plan (AI-native platform maturity)](../ROADMAP.md#improvement-plan--ai-native-platform-maturity-community-feedback) — what's already shipped from the first round
- [`docs/improvement-plan.md`](improvement-plan.md) — the competitive gap-analysis plan this document's phase/test/acceptance-criteria structure follows
