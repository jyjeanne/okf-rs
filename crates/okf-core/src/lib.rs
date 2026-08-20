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

/// Reads a source file's contents, tolerating files that aren't valid
/// UTF-8.
///
/// Legacy single-byte encodings are still common in codebases that
/// predate UTF-8 adoption -- Windows-1254 (Turkish) in particular, per
/// <https://github.com/jyjeanne/okf-rs/issues/35> -- and a single such
/// file used to abort analysis of the entire project. Bytes are decoded
/// incrementally rather than validated as UTF-8 all-or-nothing: each
/// maximal run of valid UTF-8 is kept verbatim, and only the byte(s)
/// that actually fail UTF-8 validation are decoded as Windows-1254 and
/// spliced in. That matters for a file that's genuinely UTF-8 apart from
/// a handful of stray legacy bytes (a pasted smart quote, say) -- naively
/// re-decoding the *whole* file as Windows-1254 the moment any byte
/// fails validation would mangle every other multi-byte UTF-8 sequence
/// in it (e.g. "café" becoming "cafÃ©"). A genuinely Windows-1254-encoded
/// file still comes out right: none of its non-ASCII bytes validate as
/// UTF-8 in the first place, so the whole file effectively goes through
/// the Windows-1254 path one invalid run at a time, recovering real
/// characters like `ş`/`ğ`/`ı` instead of replacing them with U+FFFD.
/// Every byte value has some mapping under Windows-1254 (unassigned
/// bytes pass through to their C1 control code point), so the fallback
/// always succeeds -- it never itself produces U+FFFD. A warning naming
/// the file is printed once (not per invalid run) so the fallback isn't
/// silent.
pub fn read_source_lossy(path: &Path) -> std::io::Result<String> {
    let bytes = std::fs::read(path)?;
    if std::str::from_utf8(&bytes).is_ok() {
        // Just proved valid above, so this can't fail -- avoids cloning
        // the whole buffer the way `String::from_utf8(bytes.clone())`
        // would just to attempt the conversion.
        return Ok(String::from_utf8(bytes).unwrap());
    }

    let mut content = String::with_capacity(bytes.len());
    let mut rest = &bytes[..];
    while !rest.is_empty() {
        match std::str::from_utf8(rest) {
            Ok(valid) => {
                content.push_str(valid);
                break;
            }
            Err(e) => {
                let valid_up_to = e.valid_up_to();
                // Safe: `valid_up_to` is exactly the length of the valid
                // UTF-8 prefix `from_utf8` just found.
                content.push_str(std::str::from_utf8(&rest[..valid_up_to]).unwrap());

                // The invalid run: a malformed sequence has a known
                // length (`error_len`); an incomplete one trailing off
                // the end of the buffer (`error_len` is `None`) runs to
                // the end of what's left.
                let bad_len = e.error_len().unwrap_or(rest.len() - valid_up_to);
                let bad_bytes = &rest[valid_up_to..valid_up_to + bad_len];
                let (decoded, _, _) = encoding_rs::WINDOWS_1254.decode(bad_bytes);
                content.push_str(&decoded);

                rest = &rest[valid_up_to + bad_len..];
            }
        }
    }
    eprintln!(
        "warning: {} is not valid UTF-8; decoded the non-UTF-8 portions as Windows-1254",
        path.display()
    );
    Ok(content)
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

    #[test]
    fn read_source_lossy_passes_through_valid_utf8_unchanged() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("main.rs");
        fs::write(&path, "fn main() { println!(\"héllo\"); }").unwrap();

        let content = read_source_lossy(&path).unwrap();

        assert_eq!(content, "fn main() { println!(\"héllo\"); }");
    }

    #[test]
    fn read_source_lossy_decodes_windows_1254_instead_of_mangling_it() {
        // 0xFD is a valid Windows-1254 byte (it maps to 'ı', the Turkish
        // dotless i) but not a valid standalone UTF-8 lead byte -- exactly
        // the kind of file https://github.com/jyjeanne/okf-rs/issues/35
        // reported. It should decode to its real character, not U+FFFD.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("legacy.cs");
        let mut bytes = b"// yaz\xfd\n".to_vec();
        bytes.extend_from_slice(b"class Program {}");
        fs::write(&path, &bytes).unwrap();

        let content = read_source_lossy(&path).unwrap();

        assert_eq!(content, "// yazı\nclass Program {}");
    }

    #[test]
    fn read_source_lossy_never_produces_replacement_characters_for_windows_1254() {
        // Every byte value 0x00-0xFF has some mapping under Windows-1254
        // (unassigned bytes like 0x81 pass through to their C1 control
        // code point rather than erroring), so the fallback never itself
        // introduces U+FFFD the way a blind lossy UTF-8 read would.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("legacy.cs");
        let mut bytes = b"// caf\x81\n".to_vec();
        bytes.extend_from_slice(b"class Program {}");
        fs::write(&path, &bytes).unwrap();

        let content = read_source_lossy(&path).unwrap();

        assert!(
            !content.contains('\u{FFFD}'),
            "Windows-1254 decoding should never introduce U+FFFD, got: {content:?}"
        );
        assert_eq!(content, "// caf\u{81}\nclass Program {}");
    }

    #[test]
    fn read_source_lossy_preserves_valid_utf8_around_a_stray_bad_byte() {
        // A file that's genuinely UTF-8 throughout except for one stray
        // non-UTF-8 byte (e.g. a pasted Windows-1252 smart quote) must
        // keep its real multi-byte UTF-8 text intact -- re-decoding the
        // *whole* file as Windows-1254 the moment any byte fails
        // validation would mangle "café" (0x63 0x61 0x66 0xc3 0xa9) into
        // "cafÃ©" even though those bytes were valid UTF-8 all along.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("mixed.rs");
        let mut bytes = b"// caf\xc3\xa9 note ".to_vec();
        bytes.push(0x92); // lone Windows-1252 right single quote: invalid UTF-8 on its own
        bytes.extend_from_slice(b" done\nfn main() {}");
        fs::write(&path, &bytes).unwrap();

        let content = read_source_lossy(&path).unwrap();

        assert_eq!(content, "// café note \u{2019} done\nfn main() {}");
    }
}
