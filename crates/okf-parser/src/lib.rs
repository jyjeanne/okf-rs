//! Shared data model for OKF concepts, produced by language extractors
//! (`okf-tree-sitter`) and consumed by `okf-analyzer`, `okf-generator`,
//! `okf-validator`, and `okf-search`.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::fmt;

mod bundle;
pub use bundle::read_bundle;

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

/// A directed edge from the owning concept to `target`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Relationship {
    pub kind: RelationKind,
    /// Stable identifier of the target concept (see [`Concept::id`]).
    pub target: String,
    /// Human-readable label for the target, used when rendering links.
    pub target_display: String,
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
    /// Last-modified time, if it can be derived deterministically from
    /// source control (e.g. the git commit date of the file). `None` when
    /// no such source is available — okf-rs never stamps concepts with the
    /// wall-clock extraction time, since that would make the bundle
    /// non-reproducible for identical source, violating the project's
    /// determinism principle.
    pub timestamp: Option<DateTime<Utc>>,
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
}
