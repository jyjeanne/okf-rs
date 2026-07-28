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
mod swift;
mod typescript;

use anyhow::{Context, Result};
use okf_core::SourceFile;
use okf_parser::{Concept, Language};
use serde::{Deserialize, Serialize};
use std::fs;

/// A call expression found in a function/method body, not yet resolved to
/// a target concept id.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CallCandidate {
    /// Concept id of the enclosing function/method.
    pub caller_id: String,
    /// Bare (unqualified) name of the called function.
    pub callee_name: String,
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
}

/// Parses `file` and extracts its concepts and call candidates.
pub fn extract_file(file: &SourceFile) -> Result<FileExtraction> {
    let source = fs::read_to_string(&file.absolute_path)
        .with_context(|| format!("failed to read {}", file.relative_path))?;

    match file.language {
        Language::Rust => rust::extract(&source, &file.relative_path),
        Language::Python => python::extract(&source, &file.relative_path),
        Language::TypeScript => typescript::extract(&source, &file.relative_path),
        Language::JavaScript => javascript::extract(&source, &file.relative_path),
        Language::Go => go::extract(&source, &file.relative_path),
        Language::Java => java::extract(&source, &file.relative_path),
        Language::CSharp => csharp::extract(&source, &file.relative_path),
        Language::Php => php::extract(&source, &file.relative_path),
        Language::Kotlin => kotlin::extract(&source, &file.relative_path),
        Language::Cpp => cpp::extract(&source, &file.relative_path),
        Language::Swift => swift::extract(&source, &file.relative_path),
    }
}
