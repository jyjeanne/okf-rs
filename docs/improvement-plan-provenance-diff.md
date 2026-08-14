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
Phase A (resolver version) has shipped; Phases B-F are still as proposed below.

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
| **Provenance-aware graph diff** (source vs. resolver vs. semantic change classes) | ❌ Not shipped | `okf_analyzer::diff`'s `relationship_set` *deliberately strips* `resolved_by`/`confidence` before comparing (`crates/okf-analyzer/src/lib.rs:383-389`) — today's diff is provenance-*blind* in both directions: it can't flag a resolver-only change, and a provenance change alone never appears in a diff at all |
| **`okf-rs diff --ci` policy with source/resolver/metadata classification** | ❌ Not shipped | `okf-rs diff` has no `--ci` flag or exit-code policy today (only `validate --ci` does) |
| **Artifact-level reproducibility metadata** (generator name/version, source revision) | ❌ Not shipped | `Concept::generated_at` exists but is *always* `None` by deliberate design (see `crates/okf-parser/src/lib.rs`'s doc comment on that field: stamping it "would make the bundle non-reproducible... violating the project's determinism principle") — the bundle-root `index.md`'s existing `okf_version` frontmatter has no generator/revision fields yet |
| **Specialized-vs-consolidated tool-*selection-accuracy*** benchmark (real model calls) | ❌ Not shipped | The consolidation above was already decided and shipped; nothing measured whether a model actually picks the right `relation` value as reliably as it picked the right tool name before |
| Golden fixture dataset for provenance/diff/MCP | ❌ Not shipped | No `tests/fixtures/` directory exists in this repo today |

Everything below phases only the six ❌ rows. A `ProvenanceOrigin` enum (`TreeSitter`/`Lsp`/
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

## 3. Phase B — Provenance-aware graph diff

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

## 4. Phase C — `okf-rs diff --ci` policy

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

- `SOURCE CHANGES` = (every relationship on an added or removed *concept*, since a concept that
  no longer exists can't have unaffected edges) + (every `RelationshipChangeKind::SourceChange`
  entry within a `Changed` concept). A concept added/removed with zero relationships of its own
  still counts as `1` toward `SOURCE CHANGES` — the concept's own existence is the change, not an
  edge — so the floor per added/removed concept is 1, plus one more per relationship it carries.
- `RESOLVER CHANGES` = every `RelationshipChangeKind::ResolverChange` **and** `ProvenanceChange`
  entry within a `Changed` concept (added/removed concepts have no "resolver changed" — either the
  target changed too, which is `SOURCE CHANGES`' job, or the concept doesn't exist to compare).
- `CONFIDENCE CHANGES` = every `RelationshipChangeKind::ConfidenceChange` entry within a `Changed`
  concept.

This means the worked example above (`❌ SOURCE CHANGES: 3`) could be one added concept with two
relationships (1 + 2 = 3), or three separate single-relationship source rewires across different
`Changed` concepts, or any other combination summing to 3 — the number is a total, not a count of
distinct concepts, and the CLI's non-`--ci` human-readable output (unaffected by this phase, see
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

## 5. Phase D — Artifact-level reproducibility metadata

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

## 6. Phase E — Specialized-vs-consolidated MCP tool-*selection* benchmark

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

A new, standalone benchmark harness (`okf-mcp --benchmark-tool-selection`, or a small script
under `crates/okf-mcp/benches/` if a live model call doesn't belong in the main binary — see
open question below) that:

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
   chose, whether it needed a follow-up/retry, and the final answer's correctness against the
   bundle's known-correct answer.
3. For the **specialized** design (reconstructed, not live): feeds the same question against a
   hand-built schema list of the 13 old `graph_*` tool names/descriptions (recoverable from git
   history — `okf-mcp` 0.2.x's `tools/list` output, or the ROADMAP's own enumeration of the old
   tool names) to the same model, recording which tool name it picked — **with one explicit
   carve-out**: `explain` has no historical specialized-tool counterpart to reconstruct (no
   `graph_explain` ever existed), so the 14th question is scored for the consolidated design only,
   footnoted as "no specialized-era equivalent" in the report rather than either dropped silently
   or answered against an invented tool name that would undermine the "reconstructed, not
   invented" methodology the rest of this phase relies on.
4. Reports, per design: tool/relation-selection accuracy, number of calls needed to reach a
   correct final answer, total tokens (schema + call + response), and latency — 13 comparable
   questions plus 1 consolidated-only question, not 14 directly comparable ones.

### Open question this phase should resolve, not assume

Every other benchmark in this codebase (`okf-mcp --benchmark`, real-world-project benchmarking)
is deliberately **offline and LLM-free** — a stated design choice, not an oversight (see
`benchmark.rs`'s own doc comment: "no LLM call or tokenizer dependency involved"). A real
tool-selection-accuracy measurement, unlike schema-size accounting, *cannot* be done that way —
"which tool would a model pick" is not decidable without actually calling one. This phase should
therefore:

- Be clearly scoped as **not run in normal CI** (no API key available, non-deterministic, real
  cost per run) — an opt-in benchmark script/command, documented as such, the same way
  `--enrich`'s network dependency is already opt-in and never exercised by default test runs.
- Support a pluggable model endpoint the same way `okf-enrich` already does (any OpenAI-compatible
  `chat/completions` endpoint via `--benchmark-model-base-url`/`--benchmark-model`), so this
  doesn't hard-depend on one vendor either.
- Report results with the same honesty `okf-mcp --benchmark`'s existing RAG-comparison gap
  already models: sample size, which model was used, and that a single model's tool-selection
  behavior is not a universal claim about "LLMs in general."

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

- Tool-selection accuracy, call count, tokens, and latency are reported for both designs, from
  the same fixed question set, run against the same bundle.
- The comparison is reproducible in the sense that matters here: same questions, same bundle,
  same model/endpoint in, comparable numbers out — not byte-identical output, which a live model
  call can never guarantee.
- The report explicitly states this validates (or doesn't) the consolidation already shipped,
  rather than presenting it as an open design decision still to be made.

---

## 7. Phase F — Golden fixture dataset

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
| B | Provenance-aware diff classification | M | High — the single biggest named gap; closes both the false-negative (rewire hidden by matching resolver names) and false-positive (invisible resolver-only change) directions at once | Medium — pairing relationships by `(kind, target)` across two snapshots needs care around duplicate targets under different kinds and concepts that both add and remove edges in the same diff; needs the five worked scenarios as real regression tests, not just the happy path | **GO**, sequenced after A (A supplies the data B classifies against) |
| C | `okf-rs diff --ci` policy | S-M | High — this is what actually makes B usable in a pipeline, matching the reviewer's own CLI example line for line | Low — mirrors `validate --ci`'s already-shipped, already-tested flag pattern; the only new surface is the `okf.toml` `[diff]` section | **GO**, sequenced after B |
| D | Artifact-level reproducibility metadata (no timestamp) | S | Medium — genuinely useful for CI audit ("which okf-rs, which commit, built this bundle"), but scoped down from the reviewer's ask specifically to avoid the determinism regression a naive implementation would cause | Medium — the risk isn't the feature, it's a future contributor "fixing" the missing timestamp back in; mitigated by testing `--check-determinism`/`--check-fresh` directly against this phase and documenting the cut in the module itself | **GO**, with the no-timestamp scope cut as a hard constraint, not a suggestion |
| E | Specialized-vs-consolidated tool-*selection* benchmark | M-L | Medium — genuinely validates (or falsifies) a decision already shipped and already justified on schema-size grounds alone; real value is closing that specific "did we trade selection accuracy for schema size and never check" open question | Medium-High — the only item in this plan requiring a live LLM call, breaking this project's until-now-consistent "every benchmark is offline and deterministic" posture; must stay explicitly opt-in/non-CI to avoid becoming a flaky, costly, silently-skipped test | **CONDITIONAL GO** — build the harness and question set (fully testable without a model) first; only wire up a real endpoint call once someone is prepared to own interpreting a non-deterministic result, matching how `--enrich`'s own network dependency was scoped in from day one |
| F | Golden fixture dataset | S | Low-Medium — organizational, not a new capability; mainly pays for itself by giving Phases B/C's own tests a less ad hoc home | Low — pure relocation/addition, no behavior change | **GO**, opportunistically alongside B/C rather than as a blocking prerequisite |

### Net recommendation

Ship, in order: **A → B → C → D**, all unconditional **GO**s — this sequence alone delivers the
reviewer's core thesis (resolver identity *and version*, provenance-aware diff, CI policy) with
no unresolved conditions, at a combined cost of roughly S+M+(S-M)+S. **F** rides alongside B/C
rather than blocking them (the fixtures phases B/C need can be written inline first, relocated
into `tests/fixtures/` as F lands). **E** is real and worth doing, but is explicitly the one item
in this plan that breaks with every existing benchmark's offline-and-deterministic posture — it's
sequenced last, and conditionally, specifically so that constraint gets a deliberate answer
(who runs it, against what endpoint, how often) rather than an accidental one.

## References

- [`docs/feedback/2026-08-provenance-graph-diff-review.md`](feedback/2026-08-provenance-graph-diff-review.md) — the raw feedback this plan distills
- [`docs/feedback/2026-08-community-roadmap-review.md`](feedback/2026-08-community-roadmap-review.md) — the reviewer's first-round feedback, already delivered
- [`ROADMAP.md` — Improvement Plan (AI-native platform maturity)](../ROADMAP.md#improvement-plan--ai-native-platform-maturity-community-feedback) — what's already shipped from the first round
- [`docs/improvement-plan.md`](improvement-plan.md) — the competitive gap-analysis plan this document's phase/test/acceptance-criteria structure follows
