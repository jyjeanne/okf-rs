//! Repository scanning: recursive, `.gitignore`-aware file discovery and
//! source-language detection, feeding `okf-tree-sitter` and `okf-analyzer`.

use anyhow::{Context, Result};
use ignore::WalkBuilder;
use okf_parser::Language;
use std::path::{Path, PathBuf};

pub mod config;

/// A single recognized source file within a [`Project`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceFile {
    /// Path relative to the project root, using `/` separators.
    pub relative_path: String,
    pub absolute_path: PathBuf,
    pub language: Language,
}

/// Well-known manifest files used to detect the kind of package at the
/// project root. Used for single-package workspace support in Phase 1;
/// multi-package/monorepo aggregation is a Phase 2 feature.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ManifestKind {
    Cargo,
    Npm,
    PyProject,
    GoModule,
}

impl ManifestKind {
    fn file_name(&self) -> &'static str {
        match self {
            ManifestKind::Cargo => "Cargo.toml",
            ManifestKind::Npm => "package.json",
            ManifestKind::PyProject => "pyproject.toml",
            ManifestKind::GoModule => "go.mod",
        }
    }

    const ALL: [ManifestKind; 4] = [
        ManifestKind::Cargo,
        ManifestKind::Npm,
        ManifestKind::PyProject,
        ManifestKind::GoModule,
    ];
}

/// A loaded, scanned project: the root directory plus every recognized
/// source file found under it, in deterministic (sorted) order.
#[derive(Debug, Clone)]
pub struct Project {
    pub root: PathBuf,
    pub files: Vec<SourceFile>,
    pub manifest: Option<ManifestKind>,
}

impl Project {
    /// Recursively scans `root`, honoring `.gitignore` (and other
    /// `ignore`-crate-recognized ignore files) plus hidden-file
    /// conventions, and collects every file whose extension maps to a
    /// supported [`Language`].
    pub fn load(root: impl AsRef<Path>) -> Result<Self> {
        let root = root
            .as_ref()
            .canonicalize()
            .with_context(|| format!("failed to resolve project root {:?}", root.as_ref()))?;

        let mut files = Vec::new();
        // `require_git(false)` makes `.gitignore` files honored even when
        // `root` isn't itself inside a `.git` working directory (e.g. a
        // subdirectory scan, or a repo checked out without its `.git`
        // folder) — the file is still an intentional ignore list either way.
        let walker = WalkBuilder::new(&root).require_git(false).build();
        for entry in walker {
            let entry = entry.context("failed to walk project directory")?;
            let is_file = entry.file_type().map(|t| t.is_file()).unwrap_or(false);
            if !is_file {
                continue;
            }
            let path = entry.path();
            let Some(language) = path
                .extension()
                .and_then(|e| e.to_str())
                .and_then(Language::from_extension)
            else {
                continue;
            };
            let relative = path
                .strip_prefix(&root)
                .unwrap_or(path)
                .to_string_lossy()
                .replace('\\', "/");
            files.push(SourceFile {
                relative_path: relative,
                absolute_path: path.to_path_buf(),
                language,
            });
        }
        files.sort_by(|a, b| a.relative_path.cmp(&b.relative_path));

        let manifest = ManifestKind::ALL
            .into_iter()
            .find(|kind| root.join(kind.file_name()).is_file());

        Ok(Project {
            root,
            files,
            manifest,
        })
    }

    /// Files matching a specific language, in deterministic order.
    pub fn files_for(&self, language: Language) -> impl Iterator<Item = &SourceFile> {
        self.files.iter().filter(move |f| f.language == language)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn scans_recognized_files_and_skips_ignored() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("Cargo.toml"), "[package]\nname=\"x\"").unwrap();
        fs::write(dir.path().join(".gitignore"), "target/\n").unwrap();
        fs::create_dir_all(dir.path().join("src")).unwrap();
        fs::write(dir.path().join("src/main.rs"), "fn main() {}").unwrap();
        fs::create_dir_all(dir.path().join("target")).unwrap();
        fs::write(dir.path().join("target/generated.rs"), "// generated").unwrap();
        fs::write(dir.path().join("README.md"), "# readme").unwrap();

        let project = Project::load(dir.path()).unwrap();

        assert_eq!(project.manifest, Some(ManifestKind::Cargo));
        let paths: Vec<_> = project
            .files
            .iter()
            .map(|f| f.relative_path.as_str())
            .collect();
        assert_eq!(paths, vec!["src/main.rs"]);
        assert_eq!(project.files[0].language, Language::Rust);
    }

    #[test]
    fn detects_multiple_languages() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("a.py"), "def f(): pass").unwrap();
        fs::write(dir.path().join("b.go"), "package main").unwrap();
        fs::write(dir.path().join("c.ts"), "export const x = 1;").unwrap();

        let project = Project::load(dir.path()).unwrap();
        assert_eq!(project.files.len(), 3);
        assert_eq!(project.files_for(Language::Go).count(), 1);
    }
}
