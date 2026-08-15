//! Shared data model for OKF concepts, produced by language extractors
//! (`okf-tree-sitter`) and consumed by `okf-analyzer`, `okf-generator`,
//! `okf-validator`, and `okf-search`.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fmt;

mod bundle;
pub use bundle::{is_concept_file, read_bundle};

/// A programming language recognized by okf-rs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Language {
    Rust,
    Python,
    TypeScript,
    JavaScript,
    Go,
    Java,
    CSharp,
    Php,
    Kotlin,
    Cpp,
    Swift,
    /// Not a programming language — the tag `okf-dita`'s importer gives a
    /// `Document` concept read from a DITA XML topic, so a mixed
    /// code+docs bundle's frontmatter `type` still reads as `<origin>
    /// <kind>` (e.g. `DITA Document`) the same way every code concept's
    /// does (e.g. `Rust Function`), rather than needing a special case.
    Dita,
}

impl Language {
    /// Human-readable name used as the frontmatter `type` prefix (e.g. "Rust Function").
    pub fn display_name(&self) -> &'static str {
        match self {
            Language::Rust => "Rust",
            Language::Python => "Python",
            Language::TypeScript => "TypeScript",
            Language::JavaScript => "JavaScript",
            Language::Go => "Go",
            Language::Java => "Java",
            Language::CSharp => "C#",
            Language::Php => "PHP",
            Language::Kotlin => "Kotlin",
            Language::Cpp => "C++",
            Language::Swift => "Swift",
            Language::Dita => "DITA",
        }
    }

    /// Maps a file extension (without the leading dot) to a language, if
    /// recognized. `c`/`h` map to [`Language::Cpp`] too (see the module
    /// docs on `okf-tree-sitter`'s `cpp` extractor for why one grammar
    /// covers both).
    pub fn from_extension(ext: &str) -> Option<Self> {
        match ext {
            "rs" => Some(Language::Rust),
            "py" | "pyi" => Some(Language::Python),
            "ts" | "tsx" => Some(Language::TypeScript),
            "js" | "jsx" | "mjs" | "cjs" => Some(Language::JavaScript),
            "go" => Some(Language::Go),
            "java" => Some(Language::Java),
            "cs" => Some(Language::CSharp),
            "php" => Some(Language::Php),
            "kt" | "kts" => Some(Language::Kotlin),
            "c" | "h" | "cpp" | "cc" | "cxx" | "c++" | "hpp" | "hh" | "hxx" | "h++" => {
                Some(Language::Cpp)
            }
            "swift" => Some(Language::Swift),
            _ => None,
        }
    }

    /// Reverses [`Language::display_name`], for parsing a bundle's
    /// frontmatter `type` field back into a `Concept` (see [`crate::read_bundle`]).
    pub fn from_display_name(name: &str) -> Option<Self> {
        match name {
            "Rust" => Some(Language::Rust),
            "Python" => Some(Language::Python),
            "TypeScript" => Some(Language::TypeScript),
            "JavaScript" => Some(Language::JavaScript),
            "Go" => Some(Language::Go),
            "Java" => Some(Language::Java),
            "C#" => Some(Language::CSharp),
            "PHP" => Some(Language::Php),
            "Kotlin" => Some(Language::Kotlin),
            "C++" => Some(Language::Cpp),
            "Swift" => Some(Language::Swift),
            "DITA" => Some(Language::Dita),
            _ => None,
        }
    }
}

impl fmt::Display for Language {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.display_name())
    }
}

/// The kind of a concept, independent of source language.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum ConceptKind {
    Package,
    Module,
    Class,
    Trait,
    Interface,
    Struct,
    Enum,
    Function,
    Method,
    Variable,
    Constant,
    /// A documentation topic imported from a non-code source (currently
    /// only `okf-dita`'s DITA importer produces these) rather than
    /// extracted from source code. Structural like `Module`/`Package` in
    /// the sense that it's not a call-graph participant or API surface —
    /// see `Graph::public_api`/`Graph::isolated_concepts`, which exclude
    /// it the same way — but it's a leaf concept, not a container: it has
    /// no `# Contains` section of its own.
    Document,
}

impl ConceptKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            ConceptKind::Package => "Package",
            ConceptKind::Module => "Module",
            ConceptKind::Class => "Class",
            ConceptKind::Trait => "Trait",
            ConceptKind::Interface => "Interface",
            ConceptKind::Struct => "Struct",
            ConceptKind::Enum => "Enum",
            ConceptKind::Function => "Function",
            ConceptKind::Method => "Method",
            ConceptKind::Variable => "Variable",
            ConceptKind::Constant => "Constant",
            ConceptKind::Document => "Document",
        }
    }

    /// The directory a concept of this kind is grouped under in a bundle.
    pub fn bundle_dir(&self) -> &'static str {
        match self {
            ConceptKind::Package => "packages",
            ConceptKind::Module => "modules",
            ConceptKind::Class | ConceptKind::Struct | ConceptKind::Enum => "classes",
            ConceptKind::Trait | ConceptKind::Interface => "interfaces",
            ConceptKind::Function | ConceptKind::Method => "functions",
            ConceptKind::Variable | ConceptKind::Constant => "variables",
            ConceptKind::Document => "documents",
        }
    }

    /// Reverses [`ConceptKind::as_str`], for parsing a bundle's
    /// frontmatter `type` field back into a `Concept` (see [`crate::read_bundle`]).
    pub fn parse(name: &str) -> Option<Self> {
        match name {
            "Package" => Some(ConceptKind::Package),
            "Module" => Some(ConceptKind::Module),
            "Class" => Some(ConceptKind::Class),
            "Trait" => Some(ConceptKind::Trait),
            "Interface" => Some(ConceptKind::Interface),
            "Struct" => Some(ConceptKind::Struct),
            "Enum" => Some(ConceptKind::Enum),
            "Function" => Some(ConceptKind::Function),
            "Method" => Some(ConceptKind::Method),
            "Variable" => Some(ConceptKind::Variable),
            "Constant" => Some(ConceptKind::Constant),
            "Document" => Some(ConceptKind::Document),
            _ => None,
        }
    }
}

impl fmt::Display for ConceptKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

/// The kind of relationship between two concepts.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum RelationKind {
    Imports,
    Calls,
    CalledBy,
    Implements,
    Inherits,
    DependsOn,
    MemberOf,
}

impl RelationKind {
    pub fn label(&self) -> &'static str {
        match self {
            RelationKind::Imports => "Imports",
            RelationKind::Calls => "Calls",
            RelationKind::CalledBy => "Called by",
            RelationKind::Implements => "Implements",
            RelationKind::Inherits => "Inherits",
            RelationKind::DependsOn => "Depends on",
            RelationKind::MemberOf => "Member of",
        }
    }

    /// The snake_case key this relation kind is grouped under in a
    /// bundle's `relationships` frontmatter field (see `okf-generator`
    /// and [`crate::read_bundle`]), e.g. `called_by`.
    pub fn frontmatter_key(&self) -> &'static str {
        match self {
            RelationKind::Imports => "imports",
            RelationKind::Calls => "calls",
            RelationKind::CalledBy => "called_by",
            RelationKind::Implements => "implements",
            RelationKind::Inherits => "inherits",
            RelationKind::DependsOn => "depends_on",
            RelationKind::MemberOf => "member_of",
        }
    }

    /// Reverses [`RelationKind::frontmatter_key`].
    pub fn from_frontmatter_key(key: &str) -> Option<Self> {
        match key {
            "imports" => Some(RelationKind::Imports),
            "calls" => Some(RelationKind::Calls),
            "called_by" => Some(RelationKind::CalledBy),
            "implements" => Some(RelationKind::Implements),
            "inherits" => Some(RelationKind::Inherits),
            "depends_on" => Some(RelationKind::DependsOn),
            "member_of" => Some(RelationKind::MemberOf),
            _ => None,
        }
    }
}

/// How confidently a relationship's target was determined — lets a
/// consumer (an AI agent, a CI check) prioritize or filter edges by how
/// much to trust them, instead of treating every edge as equally certain.
///
/// Only two variants are populated by anything in this workspace today:
/// [`Confidence::Exact`] (Tree-sitter's unambiguous, project-wide name
/// match — deterministic, syntax-only) and [`Confidence::Semantic`]
/// (`--lsp`: a real language server confirmed which definition an
/// ambiguous call site resolves to). The roadmap's other two named
/// values, `inferred` (an AI-suggested edge, not yet written back into a
/// bundle — see `okf_enrich::suggest_missing_links`) and `unresolved`
/// (a call site that couldn't be resolved to *any* target isn't
/// represented as an edge at all today, so there's nothing for a
/// `Relationship` — which by definition already has a target — to carry
/// that confidence on) aren't modeled yet: adding an enum variant nothing
/// produces would be exactly the kind of unpopulated field this project
/// avoids elsewhere (see `okf-enrich`'s own documented scope limits).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Confidence {
    Exact,
    Semantic,
}

impl Confidence {
    pub fn as_str(&self) -> &'static str {
        match self {
            Confidence::Exact => "exact",
            Confidence::Semantic => "semantic",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "exact" => Some(Confidence::Exact),
            "semantic" => Some(Confidence::Semantic),
            _ => None,
        }
    }
}

impl fmt::Display for Confidence {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// A directed edge from the owning concept to `target`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Relationship {
    pub kind: RelationKind,
    /// Stable identifier of the target concept (see [`Concept::id`]).
    pub target: String,
    /// Human-readable label for the target, used when rendering links.
    pub target_display: String,
    /// What produced this edge: `tree-sitter`, or the binary name of the
    /// language server that resolved it (`rust-analyzer`,
    /// `pyright-langserver`, ...). Always populated — every relationship
    /// has *some* resolver, even the plain structural ones.
    pub resolved_by: String,
    pub confidence: Confidence,
    /// The resolver's own reported version (LSP's `serverInfo.version`),
    /// when `resolved_by` names a real language server and that server
    /// reported one — `1.88.0` for a `rust-analyzer`-resolved edge, for
    /// instance. `None` for every `tree-sitter` edge (nothing to version:
    /// Tree-sitter's own unambiguous name match doesn't depend on a
    /// resolver release) and for a server that didn't report a version in
    /// its `initialize` response (the LSP spec doesn't require one).
    /// Lets two edges naming the same resolver at different versions be
    /// told apart without re-running the resolver to find out — see
    /// `okf_analyzer::diff`'s `RelationshipChangeKind::ResolverChange`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resolver_version: Option<String>,
}

impl Relationship {
    /// The common case: an edge produced directly by Tree-sitter's
    /// structural/syntax extraction, with no ambiguity to resolve —
    /// `tree-sitter`/[`Confidence::Exact`]. Every relationship kind besides
    /// `okf-analyzer`'s own LSP-backed ambiguous-call resolution goes
    /// through this constructor; that one path builds a `Relationship`
    /// with a real resolver name and [`Confidence::Semantic`] directly.
    pub fn new(
        kind: RelationKind,
        target: impl Into<String>,
        target_display: impl Into<String>,
    ) -> Self {
        Relationship {
            kind,
            target: target.into(),
            target_display: target_display.into(),
            resolved_by: "tree-sitter".to_string(),
            confidence: Confidence::Exact,
            resolver_version: None,
        }
    }

    /// A human-readable explanation of how this edge was determined —
    /// the "Explainability" roadmap item, built directly on the
    /// `resolved_by`/`confidence` provenance above. Purely derived from
    /// this relationship's own fields, so the phrasing stays consistent
    /// everywhere a reason is shown (CLI, MCP) rather than being
    /// duplicated per caller.
    pub fn reason(&self) -> String {
        match (self.resolved_by.as_str(), self.confidence) {
            ("tree-sitter", Confidence::Exact) => {
                "Resolved via Tree-sitter's unambiguous, project-wide name match — exactly one candidate in the project shared this name, so no further resolution was needed.".to_string()
            }
            (server, Confidence::Semantic) => {
                format!(
                    "Resolved by asking {server} which definition this call site actually resolves to, since more than one candidate shared this name and Tree-sitter's own name-matching alone couldn't disambiguate it."
                )
            }
            (resolver, confidence) => {
                format!("Resolved by {resolver} ({confidence} confidence).")
            }
        }
    }
}

/// The location of a concept in the source tree.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Location {
    /// Path relative to the project root, using `/` separators.
    pub file: String,
    pub start_line: usize,
    pub end_line: usize,
}

impl fmt::Display for Location {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.start_line == self.end_line {
            write!(f, "{}#L{}", self.file, self.start_line)
        } else {
            write!(f, "{}#L{}-L{}", self.file, self.start_line, self.end_line)
        }
    }
}

impl Location {
    /// Reverses [`Location`]'s `Display` impl, parsing a frontmatter
    /// `resource` value like `src/main.rs#L4-L6` or `src/main.rs#L4` back
    /// into a `Location` (see [`crate::read_bundle`]).
    pub fn parse(resource: &str) -> Option<Self> {
        let (file, lines) = resource.rsplit_once('#')?;
        let lines = lines.strip_prefix('L')?;
        let (start, end) = match lines.split_once("-L") {
            Some((start, end)) => (start, end),
            None => (lines, lines),
        };
        Some(Location {
            file: file.to_string(),
            start_line: start.parse().ok()?,
            end_line: end.parse().ok()?,
        })
    }
}

/// A single unit of extracted knowledge: one OKF concept, backed by one
/// markdown file with YAML frontmatter once written by `okf-generator`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Concept {
    /// Stable identifier, e.g. `functions/verify_token` or `modules/auth`.
    /// Derived from the concept's kind and fully-qualified name.
    pub id: String,
    pub kind: ConceptKind,
    pub language: Language,
    pub name: String,
    pub qualified_name: String,
    pub description: Option<String>,
    pub location: Location,
    pub signature: Option<String>,
    pub tags: Vec<String>,
    /// Whether this concept is part of the project's public API surface.
    /// Detected per language: Rust's `pub` modifier (any `pub`/`pub(...)`
    /// variant); Go's exported-identifier convention (leading uppercase);
    /// Python/JavaScript/TypeScript's leading-underscore-is-private
    /// convention. The JS/TS and Python rules are conventions, not
    /// language guarantees (unlike Rust's `pub` or Go's capitalization),
    /// so treat this as a heuristic for those three until real `export`
    /// tracking lands. `Module` and `Package` concepts are always public
    /// — they're structural, not something a language marks private.
    pub is_public: bool,
    /// The content's last meaningful change, if it can be derived
    /// deterministically from source control (e.g. the git commit date of
    /// the file). Rendered into the OKF v0.2 `generated.at` frontmatter
    /// field (`okf-generator` always fills in the sibling `generated.by`
    /// with the `okf-rs/<version>` actor, since the producer is always
    /// known). `None` when no such source is available — okf-rs never
    /// stamps concepts with the wall-clock extraction time, since that
    /// would make the bundle non-reproducible for identical source,
    /// violating the project's determinism principle.
    pub generated_at: Option<DateTime<Utc>>,
    pub relationships: Vec<Relationship>,
}

impl Concept {
    /// The frontmatter `type` value, e.g. "Rust Function".
    pub fn frontmatter_type(&self) -> String {
        format!("{} {}", self.language.display_name(), self.kind.as_str())
    }

    /// Reverses [`Concept::frontmatter_type`], splitting a frontmatter
    /// `type` value like `Rust Function` back into its language and kind
    /// (see [`crate::read_bundle`]).
    pub fn parse_frontmatter_type(type_: &str) -> Option<(Language, ConceptKind)> {
        let (language, kind) = type_.split_once(' ')?;
        Some((
            Language::from_display_name(language)?,
            ConceptKind::parse(kind)?,
        ))
    }

    /// Builds the stable identifier used for cross-linking, e.g.
    /// `functions/verify_token` for a function named `verify_token`.
    pub fn make_id(kind: ConceptKind, qualified_name: &str) -> String {
        let slug = qualified_name.replace("::", "/").replace('.', "/");
        format!("{}/{}", kind.bundle_dir(), slug)
    }

    /// Assigns a unique id to every concept in `concepts` after the first
    /// occurrence of a given id (compared case-insensitively, since ids
    /// become file paths on filesystems that may not distinguish case),
    /// by appending `-2`, `-3`, ... to each repeat, in place.
    ///
    /// Common for conditionally-compiled definitions that share one name
    /// in one file but are never actually compiled together — e.g. Rust's
    /// `#[cfg(feature = "x")]` / `#[cfg(not(feature = "x"))]` stub pair —
    /// which Tree-sitter (unlike `rustc`) has no way to tell apart, since
    /// it doesn't evaluate `cfg` attributes.
    ///
    /// Must run before anything indexes concepts by id — `okf-analyzer`
    /// calls this right after collecting every file's concepts, before
    /// building its id-to-index map for relationship resolution;
    /// otherwise two same-id concepts stay indistinguishable there and a
    /// resolved edge gets attributed to an arbitrary one of the pair
    /// instead of the one whose body actually made the call.
    /// `okf-generator::write_bundle` also calls this itself as a final
    /// safety net, so nothing that reaches it can ever silently overwrite
    /// another concept's file on disk.
    ///
    /// Ordered by location (`file`, then `start_line`) rather than
    /// `concepts`' incoming order, so which occurrence is considered
    /// "first" (and so keeps the unsuffixed id) is deterministic
    /// regardless of extraction order.
    pub fn disambiguate_ids(concepts: &mut [Concept]) {
        let mut order: Vec<usize> = (0..concepts.len()).collect();
        order.sort_by(|&a, &b| {
            concepts[a]
                .location
                .file
                .cmp(&concepts[b].location.file)
                .then(
                    concepts[a]
                        .location
                        .start_line
                        .cmp(&concepts[b].location.start_line),
                )
        });

        let mut seen: HashMap<String, usize> = HashMap::new();
        // `.../index.md` is a reserved filename: `okf-generator` writes
        // one per top-level kind directory (`functions/index.md`,
        // `modules/index.md`, ...) as a plain navigational listing with
        // no frontmatter, and the OKF spec names `index.md` generally as
        // the convention for progressive disclosure. A concept whose own
        // id happens to end in `/index` — a method literally named
        // `index` (common for a Rust `impl Index for ...`), or a
        // module/function literally named `index` (extremely common for
        // a web framework's entry-point handler) — collides with that
        // reserved name. Found by real-world benchmarking (August 2026)
        // against ripgrep: a `HiArgs::index()` method produced a concept
        // file whose path collided with the `index.md` convention,
        // rejected by `okf-validator`'s "only the bundle-root index.md
        // may carry frontmatter" check — confirmed, and reproduced
        // independently against a small Python fixture too. Nothing in
        // the current generator actually overwrites a *sibling*
        // navigation page for a nested directory like `HiArgs/` (only
        // kind-root directories get one), but the same id shape at a
        // kind-root itself (a bare top-level function/module literally
        // named `index`) would collide directly with `functions/index.md`
        // itself, written *after* concept files in `write_bundle` — the
        // concept silently overwritten by the generic listing, with
        // nothing to detect it. Pre-seeding `seen` with one occurrence
        // for every id ending in `/index` reuses the exact "-2", "-3",
        // ... bump logic below for this reserved slot, so the first real
        // `index`-named concept anywhere in the bundle becomes
        // `.../index-2`, leaving every `.../index.md` itself as the
        // navigation page it's meant to be. Matched case-insensitively,
        // same as the duplicate-id check below, since ids become file
        // paths on filesystems that may not distinguish case either way.
        for concept in concepts.iter() {
            if concept
                .id
                .rsplit('/')
                .next()
                .is_some_and(|last| last.eq_ignore_ascii_case("index"))
            {
                seen.entry(concept.id.to_ascii_lowercase()).or_insert(1);
            }
        }

        for idx in order {
            let count = seen
                .entry(concepts[idx].id.to_ascii_lowercase())
                .or_insert(0);
            *count += 1;
            if *count > 1 {
                concepts[idx].id = format!("{}-{}", concepts[idx].id, count);
            }
        }
    }
}

#[cfg(test)]
mod disambiguate_tests {
    use super::*;

    fn concept(id: &str, file: &str, line: usize) -> Concept {
        Concept {
            id: id.to_string(),
            kind: ConceptKind::Function,
            language: Language::Rust,
            name: id.rsplit('/').next().unwrap_or(id).to_string(),
            qualified_name: id.to_string(),
            description: None,
            location: Location {
                file: file.to_string(),
                start_line: line,
                end_line: line,
            },
            signature: None,
            tags: Vec::new(),
            is_public: true,
            generated_at: None,
            relationships: Vec::new(),
        }
    }

    /// Found by real-world benchmarking (August 2026) against ripgrep: a
    /// `HiArgs::index()` method's own concept produced a file at
    /// `HiArgs/index.md`, colliding with the reserved `index.md`
    /// navigation-page convention (`okf-validator` rejects it: "only the
    /// bundle-root index.md may carry frontmatter").
    #[test]
    fn a_concept_literally_named_index_is_bumped_to_avoid_the_navigation_page() {
        let mut concepts = vec![concept("functions/HiArgs/index", "a.rs", 1)];
        Concept::disambiguate_ids(&mut concepts);
        assert_eq!(concepts[0].id, "functions/HiArgs/index-2");
    }

    #[test]
    fn a_concept_not_named_index_is_left_alone() {
        let mut concepts = vec![concept("functions/HiArgs/matcher", "a.rs", 1)];
        Concept::disambiguate_ids(&mut concepts);
        assert_eq!(concepts[0].id, "functions/HiArgs/matcher");
    }

    /// Ids become file paths on filesystems that may not distinguish
    /// case (the same reasoning the genuine-duplicate-id check below
    /// already applies) -- a concept named `Index` collides with
    /// `index.md` just as surely as one named `index` would.
    #[test]
    fn the_reserved_index_name_is_matched_case_insensitively() {
        let mut concepts = vec![concept("functions/HiArgs/Index", "a.rs", 1)];
        Concept::disambiguate_ids(&mut concepts);
        assert_eq!(concepts[0].id, "functions/HiArgs/Index-2");
    }

    /// Two concepts genuinely named `index` in the same directory chain
    /// through the reserved slot correctly: the first becomes `-2` (the
    /// navigation page itself is the implicit "occurrence 1"), the
    /// second `-3` -- the existing duplicate-id bump logic, unmodified.
    #[test]
    fn two_concepts_both_named_index_chain_past_the_reserved_slot() {
        let mut concepts = vec![
            concept("functions/HiArgs/index", "a.rs", 1),
            concept("functions/HiArgs/index", "a.rs", 5),
        ];
        Concept::disambiguate_ids(&mut concepts);
        assert_eq!(concepts[0].id, "functions/HiArgs/index-2");
        assert_eq!(concepts[1].id, "functions/HiArgs/index-3");
    }

    /// `index` in a *different* directory is an unrelated reserved slot
    /// -- disambiguating one doesn't touch the other.
    #[test]
    fn index_collisions_in_different_directories_are_independent() {
        let mut concepts = vec![
            concept("functions/HiArgs/index", "a.rs", 1),
            concept("functions/OtherType/index", "b.rs", 1),
        ];
        Concept::disambiguate_ids(&mut concepts);
        assert_eq!(concepts[0].id, "functions/HiArgs/index-2");
        assert_eq!(concepts[1].id, "functions/OtherType/index-2");
    }
}

#[cfg(test)]
mod relationship_tests {
    use super::*;

    #[test]
    fn confidence_as_str_and_parse_round_trip() {
        for c in [Confidence::Exact, Confidence::Semantic] {
            assert_eq!(Confidence::parse(c.as_str()), Some(c));
        }
    }

    #[test]
    fn confidence_parse_rejects_an_unrecognized_value() {
        assert_eq!(Confidence::parse("inferred"), None);
        assert_eq!(Confidence::parse(""), None);
    }

    #[test]
    fn confidence_display_matches_as_str() {
        assert_eq!(Confidence::Exact.to_string(), "exact");
        assert_eq!(Confidence::Semantic.to_string(), "semantic");
    }

    #[test]
    fn relationship_new_defaults_to_tree_sitter_exact() {
        let rel = Relationship::new(RelationKind::Calls, "functions/b", "b");
        assert_eq!(rel.kind, RelationKind::Calls);
        assert_eq!(rel.target, "functions/b");
        assert_eq!(rel.target_display, "b");
        assert_eq!(rel.resolved_by, "tree-sitter");
        assert_eq!(rel.confidence, Confidence::Exact);
    }

    #[test]
    fn reason_for_a_tree_sitter_resolved_edge_names_the_resolver() {
        let rel = Relationship::new(RelationKind::Calls, "functions/b", "b");
        let reason = rel.reason();
        assert!(reason.contains("Tree-sitter"));
        assert!(reason.contains("unambiguous"));
    }

    #[test]
    fn reason_for_an_lsp_resolved_edge_names_the_real_server() {
        let rel = Relationship {
            kind: RelationKind::Calls,
            target: "functions/b".to_string(),
            target_display: "b".to_string(),
            resolved_by: "rust-analyzer".to_string(),
            confidence: Confidence::Semantic,
            resolver_version: Some("1.88.0".to_string()),
        };
        let reason = rel.reason();
        assert!(reason.contains("rust-analyzer"));
        assert!(reason.contains("more than one candidate"));
    }

    #[test]
    fn reason_falls_back_to_a_generic_sentence_for_an_unexpected_combination() {
        // Not a combination anything in this workspace actually produces
        // today (every resolver is either "tree-sitter"/Exact or a real
        // LSP server name/Semantic) -- but a hand-edited bundle could
        // carry one, and `reason()` should still say something sensible
        // rather than panic or return an empty string.
        let rel = Relationship {
            kind: RelationKind::Calls,
            target: "functions/b".to_string(),
            target_display: "b".to_string(),
            resolved_by: "hand-edited".to_string(),
            confidence: Confidence::Exact,
            resolver_version: None,
        };
        let reason = rel.reason();
        assert!(reason.contains("hand-edited"));
        assert!(reason.contains("exact"));
    }
}
