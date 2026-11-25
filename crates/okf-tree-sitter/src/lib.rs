//! Tree-sitter based symbol and relationship extraction.
//!
//! For each source file, [`extract_file`] returns every concept declared in
//! it (one `Module` concept for the file itself, plus its types and
//! functions/methods) and the raw call-expression candidates found in
//! function/method bodies. Import relationships are resolved eagerly (they
//! don't need project-wide information); call relationships are left
//! unresolved here because knowing whether `bar()` refers to a real concept
//! requires seeing the whole project — that resolution happens in
//! `okf-analyzer`.

mod common;
mod cpp;
mod csharp;
mod go;
mod java;
mod javascript;
mod jsish;
mod kotlin;
mod php;
mod python;
mod rust;
mod rust_types;
mod swift;
mod typescript;

use anyhow::{Context, Result};
use okf_core::SourceFile;
use okf_parser::{Concept, Language};
use serde::{Deserialize, Serialize};

/// A call expression's position, 0-based in UTF-16 code units the way the
/// Language Server Protocol's `Position` is — not tree-sitter's own byte-
/// offset convention — so `okf-lsp` can query `textDocument/definition`
/// at this exact spot without a second conversion step. See
/// `common::lsp_position`.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct CallSite {
    pub line: u32,
    pub character: u32,
}

/// A call expression found in a function/method body, not yet resolved to
/// a target concept id.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CallCandidate {
    /// Concept id of the enclosing function/method.
    pub caller_id: String,
    /// Bare (unqualified) name of the called function.
    pub callee_name: String,
    /// Where the call itself appears — used only for LSP-based
    /// disambiguation of calls whose name is ambiguous project-wide (see
    /// `okf_lsp`); Tree-sitter's own unambiguous-name resolution in
    /// `okf-analyzer` doesn't need it.
    pub call_site: CallSite,
    /// What the extractor could tell about the call's receiver — the `x` in
    /// `x.method()`, or the `Type` in `Type::method()` — for `okf-analyzer`
    /// to resolve to a type project-wide and pick the method that type
    /// actually has. `None` for a plain `function()` call, for a receiver
    /// the extractor can't describe, and for every language but Rust so
    /// far.
    #[serde(default)]
    pub receiver: Option<TypeHint>,
    /// The call's syntactic form, which decides whether a call from a
    /// function to a same-named function can be trusted as recursion (see
    /// [`CallForm`]). [`CallForm::Unknown`] for languages whose extractor
    /// doesn't record it.
    #[serde(default)]
    pub form: CallForm,
}

/// How a call is written.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CallForm {
    /// Not recorded (every extractor but Rust's, so far).
    #[default]
    Unknown,
    /// `f(..)`, where `f` isn't imported by a `use` inside the calling
    /// function (such a call is recorded as [`CallForm::Path`]).
    Plain,
    /// `self.f(..)`.
    SelfMethod,
    /// `x.f(..)` on anything but `self`.
    Method,
    /// `Self::f(..)`, or `Type::f(..)` with the enclosing impl's own type
    /// as a single-segment path.
    SelfPath,
    /// Any other path call (`a::b::f(..)`, `OtherType::f(..)`), or a plain
    /// call to a name imported inside the calling function.
    Path,
}

/// A description of an expression's type, as far as one file's syntax
/// can tell. Resolving it to a type name needs facts from the whole
/// project (a field's declared type, a method's return type), which is
/// why it's recorded here and resolved in `okf-analyzer` (see
/// [`TypeFacts`]). Type names are bare (last path segment, generics and
/// references removed; `Arc`/`Box`/`Option`/`Result`-style wrappers
/// unwrapped).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TypeHint {
    /// A known type, e.g. from a parameter's or `let`'s annotation, `self`,
    /// a struct literal, or a `Type::` qualifier.
    Named(String),
    /// The type of field `field` on whatever type `base` resolves to.
    Field { base: Box<TypeHint>, field: String },
    /// The return type of method `method` called on whatever `base`
    /// resolves to.
    MethodReturn { base: Box<TypeHint>, method: String },
    /// The return type of `Type::function(...)` (`owner` set) or of a free
    /// function `function(...)`.
    FnReturn {
        owner: Option<String>,
        function: String,
    },
}

/// Per-file type facts the analyzer combines project-wide to resolve
/// [`TypeHint`]s. Only the Rust extractor fills these in so far.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TypeFacts {
    /// Named struct fields and their (normalized) types.
    #[serde(default)]
    pub fields: Vec<FieldFact>,
    /// Methods and associated functions, including trait method
    /// signatures that have no body (and so no concept).
    #[serde(default)]
    pub methods: Vec<MethodFact>,
    /// Free functions' return types.
    #[serde(default)]
    pub functions: Vec<FunctionFact>,
    /// `type Name = Target;` aliases.
    #[serde(default)]
    pub aliases: Vec<AliasFact>,
}

/// `type name = target;`, with `target` normalized (`None` when it isn't a
/// nameable type, e.g. a tuple).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AliasFact {
    pub name: String,
    pub target: Option<String>,
}

/// `owner.field: ty`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FieldFact {
    pub owner: String,
    pub field: String,
    pub ty: String,
}

/// A method or associated function declared on `owner` (an `impl` block's
/// type or a trait's name), optionally implementing `trait_name`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MethodFact {
    /// The method's concept id; `None` for a trait method signature with
    /// no body, which contributes only its return type.
    pub concept_id: Option<String>,
    pub owner: String,
    pub trait_name: Option<String>,
    pub name: String,
    /// Normalized return type (`Self` already replaced by `owner`).
    pub returns: Option<String>,
}

/// A free function's normalized return type.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FunctionFact {
    pub name: String,
    pub returns: Option<String>,
}

/// Everything extracted from a single source file.
///
/// `Serialize`/`Deserialize` so `okf-analyzer` can cache it to disk, keyed
/// by the source file's content hash, and skip re-parsing files that
/// haven't changed since the last run (see `okf_analyzer::AnalysisCache`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileExtraction {
    /// The file's own `Module` concept, followed by its types and
    /// functions/methods, in source order.
    pub concepts: Vec<Concept>,
    pub calls: Vec<CallCandidate>,
    /// Type facts for receiver-type resolution (Rust only so far).
    #[serde(default)]
    pub types: TypeFacts,
}

/// Parses `file` and extracts its concepts and call candidates, reading its
/// contents from disk itself. Prefer [`extract_source`] when the caller
/// already has the file's contents in hand (e.g. to compute a content
/// hash) — reading it again here would cost a redundant disk read, and
/// risks the two reads observing different content if the file is
/// modified concurrently.
pub fn extract_file(file: &SourceFile) -> Result<FileExtraction> {
    let source = okf_core::read_source_lossy(&file.absolute_path)
        .with_context(|| format!("failed to read {}", file.relative_path))?;
    extract_source(&source, file)
}

/// Extracts concepts and call candidates from `source`, already read by the
/// caller — see [`extract_file`] for the disk-reading convenience wrapper.
pub fn extract_source(source: &str, file: &SourceFile) -> Result<FileExtraction> {
    match file.language {
        Language::Rust => rust::extract(source, &file.relative_path),
        Language::Python => python::extract(source, &file.relative_path),
        Language::TypeScript => typescript::extract(source, &file.relative_path),
        Language::JavaScript => javascript::extract(source, &file.relative_path),
        Language::Go => go::extract(source, &file.relative_path),
        Language::Java => java::extract(source, &file.relative_path),
        Language::CSharp => csharp::extract(source, &file.relative_path),
        Language::Php => php::extract(source, &file.relative_path),
        Language::Kotlin => kotlin::extract(source, &file.relative_path),
        Language::Cpp => cpp::extract(source, &file.relative_path),
        Language::Swift => swift::extract(source, &file.relative_path),
        // Unreachable in normal operation: `Language::from_extension`
        // deliberately never maps a `.dita` file to `Language::Dita`, so
        // `okf_core::Project::load`'s source-file scan never produces a
        // `SourceFile` with this language in the first place -- DITA XML
        // goes through `okf-dita`'s own importer, a separate path from
        // this source-code extraction dispatch entirely. Still needs an
        // arm here since `Language` is otherwise exhaustively matched.
        Language::Dita => Err(anyhow::anyhow!(
            "{}: DITA files aren't extracted as source code -- use okf-dita's importer instead",
            file.relative_path
        )),
    }
}
