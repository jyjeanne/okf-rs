//! Shared data model for OKF concepts, produced by language extractors
//! (`okf-tree-sitter`) and consumed by `okf-analyzer`, `okf-generator`,
//! `okf-validator`, and `okf-search`.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::fmt;

/// A programming language recognized by okf-rs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Language {
    Rust,
    Python,
    TypeScript,
    JavaScript,
    Go,
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
        }
    }

    /// Maps a file extension (without the leading dot) to a language, if recognized.
    pub fn from_extension(ext: &str) -> Option<Self> {
        match ext {
            "rs" => Some(Language::Rust),
            "py" | "pyi" => Some(Language::Python),
            "ts" | "tsx" => Some(Language::TypeScript),
            "js" | "jsx" | "mjs" | "cjs" => Some(Language::JavaScript),
            "go" => Some(Language::Go),
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
}

impl fmt::Display for ConceptKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

/// The kind of relationship between two concepts.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
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

    /// Builds the stable identifier used for cross-linking, e.g.
    /// `functions/verify_token` for a function named `verify_token`.
    pub fn make_id(kind: ConceptKind, qualified_name: &str) -> String {
        let slug = qualified_name.replace("::", "/").replace('.', "/");
        format!("{}/{}", kind.bundle_dir(), slug)
    }
}
