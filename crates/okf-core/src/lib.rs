//! Repository scanning: recursive, `.gitignore`-aware file discovery and
//! source-language detection, feeding `okf-tree-sitter` and `okf-analyzer`.

use anyhow::{Context, Result};
use ignore::WalkBuilder;
use okf_parser::Language;
use std::path::{Path, PathBuf};

pub mod config;
pub mod git;

/// A single recognized source file within a [`Project`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceFile {
    /// Path relative to the project root, using `/` separators.
    pub relative_path: String,
    pub absolute_path: PathBuf,
    pub language: Language,
}

/// Well-known manifest files used to detect the kind of package at a
/// project directory. A single project can contain more than one — a
/// Cargo/npm/monorepo workspace with several member packages — see
/// [`Project::packages`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ManifestKind {
    Cargo,
    Npm,
    PyProject,
    GoModule,
}

impl ManifestKind {
    pub fn file_name(&self) -> &'static str {
        match self {
            ManifestKind::Cargo => "Cargo.toml",
            ManifestKind::Npm => "package.json",
            ManifestKind::PyProject => "pyproject.toml",
            ManifestKind::GoModule => "go.mod",
        }
    }

    /// A short, filesystem/id-safe tag identifying this manifest kind,
    /// used to disambiguate two `Package` concepts that would otherwise
    /// collide on the same directory (e.g. a Rust crate with an npm-based
    /// docs build alongside it).
    pub fn short_tag(&self) -> &'static str {
        match self {
            ManifestKind::Cargo => "cargo",
            ManifestKind::Npm => "npm",
            ManifestKind::PyProject => "pyproject",
            ManifestKind::GoModule => "gomod",
        }
    }

    const ALL: [ManifestKind; 4] = [
        ManifestKind::Cargo,
        ManifestKind::Npm,
        ManifestKind::PyProject,
        ManifestKind::GoModule,
    ];
}

/// A manifest file found somewhere under a project root, identifying one
/// member package of a (possibly multi-package) workspace or monorepo.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PackageRoot {
    /// Directory containing the manifest, relative to the project root,
    /// `/`-separated with no trailing slash. Empty for the project root
    /// itself.
    pub relative_dir: String,
    pub manifest: ManifestKind,
}

/// Directory names that are never treated as containing a first-party
/// package even if a manifest-shaped file happens to live there — vendored
/// or generated trees that would otherwise be mistaken for real workspace
/// members. This is a backstop, not the primary defense: a project's own
/// `.gitignore` (honored by the scan itself) already excludes most of
/// these in any well-configured repository.
const IGNORED_PACKAGE_DIRS: [&str; 5] = ["node_modules", "vendor", "target", "dist", "build"];

/// A loaded, scanned project: the root directory plus every recognized
/// source file found under it, in deterministic (sorted) order.
#[derive(Debug, Clone)]
pub struct Project {
    pub root: PathBuf,
    pub files: Vec<SourceFile>,
    /// The project root's own manifest, if it has one directly (not one
    /// belonging to a member package elsewhere in the tree) — the
    /// single-package case every project had before multi-package support.
    pub manifest: Option<ManifestKind>,
    /// Every manifest found anywhere under the project root, in
    /// deterministic order (by directory, then by manifest precedence for
    /// the rare case of two manifest kinds in the same directory). Always
    /// includes the root's own manifest (as an empty-`relative_dir`
    /// entry) when [`Project::manifest`] is `Some`.
    pub packages: Vec<PackageRoot>,
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
        let mut packages = Vec::new();
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
            let relative = path
                .strip_prefix(&root)
                .unwrap_or(path)
                .to_string_lossy()
                .replace('\\', "/");

            if let Some(file_name) = path.file_name().and_then(|n| n.to_str()) {
                if let Some(kind) = ManifestKind::ALL
                    .into_iter()
                    .find(|k| k.file_name() == file_name)
                {
                    let dir = relative.rsplit_once('/').map(|(d, _)| d).unwrap_or("");
                    if !is_ignored_package_dir(dir) {
                        packages.push(PackageRoot {
                            relative_dir: dir.to_string(),
                            manifest: kind,
                        });
                    }
                }
            }

            let Some(language) = path
                .extension()
                .and_then(|e| e.to_str())
                .and_then(Language::from_extension)
            else {
                continue;
            };
            files.push(SourceFile {
                relative_path: relative,
                absolute_path: path.to_path_buf(),
                language,
            });
        }
        files.sort_by(|a, b| a.relative_path.cmp(&b.relative_path));
        packages.sort_by_key(|p| (p.relative_dir.clone(), manifest_ordinal(p.manifest)));

        let manifest = ManifestKind::ALL
            .into_iter()
            .find(|kind| root.join(kind.file_name()).is_file());

        Ok(Project {
            root,
            files,
            manifest,
            packages,
        })
    }

    /// Files matching a specific language, in deterministic order.
    pub fn files_for(&self, language: Language) -> impl Iterator<Item = &SourceFile> {
        self.files.iter().filter(move |f| f.language == language)
    }
}

fn is_ignored_package_dir(dir: &str) -> bool {
    dir.split('/')
        .any(|part| IGNORED_PACKAGE_DIRS.contains(&part))
}

/// `ManifestKind::ALL`'s index, used only to break ties deterministically
/// when two manifest kinds are found in the very same directory (e.g. a
/// Rust crate with an npm-based docs build alongside it) — without this,
/// [`Project::packages`]' sort (by directory) would leave same-directory
/// entries in non-deterministic (filesystem walk) order relative to each
/// other.
fn manifest_ordinal(kind: ManifestKind) -> usize {
    ManifestKind::ALL
        .iter()
        .position(|k| *k == kind)
        .unwrap_or(usize::MAX)
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
        assert_eq!(
            project.packages,
            vec![PackageRoot {
                relative_dir: String::new(),
                manifest: ManifestKind::Cargo,
            }]
        );
    }

    #[test]
    fn discovers_every_member_package_in_a_workspace() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(
            dir.path().join("Cargo.toml"),
            "[workspace]\nmembers = [\"crates/*\"]\n",
        )
        .unwrap();
        for member in ["crates/a", "crates/b"] {
            fs::create_dir_all(dir.path().join(member).join("src")).unwrap();
            fs::write(
                dir.path().join(member).join("Cargo.toml"),
                format!("[package]\nname = \"{member}\"\nversion = \"0.1.0\"\n"),
            )
            .unwrap();
            fs::write(dir.path().join(member).join("src/lib.rs"), "").unwrap();
        }

        let project = Project::load(dir.path()).unwrap();

        // The workspace root itself has a Cargo.toml but no `[package]`
        // table (a "virtual manifest"), which is still a real manifest
        // file as far as discovery is concerned — `okf-analyzer` is the
        // one that later decides there's no package *name* to emit a
        // concept for.
        assert_eq!(
            project.packages,
            vec![
                PackageRoot {
                    relative_dir: String::new(),
                    manifest: ManifestKind::Cargo,
                },
                PackageRoot {
                    relative_dir: "crates/a".to_string(),
                    manifest: ManifestKind::Cargo,
                },
                PackageRoot {
                    relative_dir: "crates/b".to_string(),
                    manifest: ManifestKind::Cargo,
                },
            ]
        );
    }

    #[test]
    fn ignores_manifests_under_vendored_dependency_directories() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(
            dir.path().join("package.json"),
            "{\"name\": \"root\", \"version\": \"0.1.0\"}",
        )
        .unwrap();
        fs::create_dir_all(dir.path().join("node_modules/some-dep")).unwrap();
        fs::write(
            dir.path().join("node_modules/some-dep/package.json"),
            "{\"name\": \"some-dep\", \"version\": \"1.0.0\"}",
        )
        .unwrap();

        let project = Project::load(dir.path()).unwrap();

        assert_eq!(
            project.packages,
            vec![PackageRoot {
                relative_dir: String::new(),
                manifest: ManifestKind::Npm,
            }],
            "the vendored dependency's package.json should not be discovered as a project package"
        );
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
