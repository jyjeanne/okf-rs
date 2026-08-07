//! A process-local cache of parsed bundle concepts, so a long-lived
//! `okf-mcp` server session fielding many `tools/call` requests in a row
//! doesn't pay the full walk-and-parse cost of `okf_parser::read_bundle`
//! on every single one — the "Continued AI-agent optimization" roadmap
//! item, applied specifically to `okf-mcp` and nowhere else: `okf-cli`
//! is a fresh process per invocation, so there's nothing here for it to
//! amortize, and every `okf-query` function stays available in its
//! original `bundle: &Path`-taking form for exactly that reason (see
//! `okf-query`'s crate-level `# Caching` note).
//!
//! # Freshness, not staleness
//!
//! This cache never trades correctness for speed. Every [`BundleCache::get_or_load`]
//! call computes a cheap, content-free [`Fingerprint`] of the bundle
//! directory first — file count, total size, and the latest modification
//! time across every concept file `read_bundle` would itself walk — and
//! only reuses the cached `Vec<Concept>` when that fingerprint is
//! unchanged since the last load. A `generate` run between two tool
//! calls (adding, removing, or editing a concept file) changes the
//! fingerprint and forces a fresh parse, so a session never needs
//! restarting to see the latest bundle — matching the guarantee
//! `main.rs`'s module doc already makes.
//!
//! Computing the fingerprint still walks the bundle directory (one
//! `stat` per file), but never opens or reads a file's content — the
//! actual expensive part of `read_bundle` (UTF-8 read, YAML frontmatter
//! parse, cross-concept id disambiguation) is what gets skipped on a
//! cache hit.
//!
//! The one accepted gap, common to every mtime-based cache (`make`,
//! incremental compilers, ...): a file rewritten with the exact same
//! byte length and an explicitly backdated modification time, inside the
//! same second as an unrelated change, could in principle defeat the
//! fingerprint. `okf-rs generate` never does this — it writes fresh
//! content with the real current time — so this is a theoretical gap in
//! adversarial-editing scenarios, not a practical one for the normal
//! generate-then-query workflow this cache is built for.

use anyhow::Result;
use okf_parser::Concept;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::SystemTime;

/// A cheap, content-free snapshot of a bundle directory's concept files —
/// see the module docs' `# Freshness, not staleness` section for exactly
/// what this does and doesn't guard against.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Fingerprint {
    file_count: usize,
    total_bytes: u64,
    latest_mtime: Option<SystemTime>,
}

/// Walks `bundle` the same way `okf_parser::read_bundle` does — via the
/// same [`okf_parser::is_concept_file`] filter `read_bundle` itself uses,
/// not a re-encoded copy of its two conditions that could silently drift
/// out of sync with it — but only stats each file rather than reading it.
fn fingerprint(bundle: &Path) -> Fingerprint {
    let mut file_count = 0usize;
    let mut total_bytes = 0u64;
    let mut latest_mtime: Option<SystemTime> = None;

    for entry in walkdir::WalkDir::new(bundle)
        .into_iter()
        .filter_map(|e| e.ok())
    {
        if !entry.file_type().is_file() {
            continue;
        }
        let path = entry.path();
        if !okf_parser::is_concept_file(path) {
            continue;
        }
        let Ok(metadata) = entry.metadata() else {
            continue;
        };
        file_count += 1;
        total_bytes += metadata.len();
        if let Ok(mtime) = metadata.modified() {
            latest_mtime = Some(latest_mtime.map_or(mtime, |current| current.max(mtime)));
        }
    }

    Fingerprint {
        file_count,
        total_bytes,
        latest_mtime,
    }
}

struct CachedBundle {
    fingerprint: Fingerprint,
    concepts: Arc<Vec<Concept>>,
}

/// One instance lives for the lifetime of an `okf-mcp` server process
/// (see `main.rs`), shared across every `tools/call`. Keyed by bundle
/// path so a server started with an unusual setup that ends up querying
/// more than one bundle path in its lifetime (not the common case, but
/// not prevented either) still caches each independently rather than
/// thrashing a single slot.
#[derive(Default)]
pub struct BundleCache {
    entries: Mutex<HashMap<PathBuf, CachedBundle>>,
    /// How many times this cache has actually gone to disk to parse a
    /// bundle, as opposed to serving a cached `Vec` — exposed for tests
    /// to prove a cache hit really did skip the parse, not just that the
    /// two calls happened to agree on the result.
    loads: AtomicUsize,
}

impl BundleCache {
    /// A fresh, empty cache — the first call for any bundle path is
    /// always a real load.
    pub fn new() -> Self {
        Self::default()
    }

    /// Returns `bundle`'s concepts, reusing the last-loaded `Vec` (via a
    /// cheap `Arc` clone) if the bundle's [`Fingerprint`] hasn't changed
    /// since then, and re-parsing from disk otherwise. Errors the same
    /// way `okf_query::load_concepts` does when `bundle` doesn't exist.
    pub fn get_or_load(&self, bundle: &Path) -> Result<Arc<Vec<Concept>>> {
        okf_query::require_bundle(bundle)?;
        let fp = fingerprint(bundle);

        let mut entries = self.entries.lock().unwrap();
        if let Some(cached) = entries.get(bundle) {
            if cached.fingerprint == fp {
                return Ok(Arc::clone(&cached.concepts));
            }
        }

        let concepts = Arc::new(okf_parser::read_bundle(bundle)?);
        self.loads.fetch_add(1, Ordering::Relaxed);
        entries.insert(
            bundle.to_path_buf(),
            CachedBundle {
                fingerprint: fp,
                concepts: Arc::clone(&concepts),
            },
        );
        Ok(concepts)
    }

    /// How many real (non-cached) parses this cache has performed —
    /// `#[cfg(test)]` only, and `pub(crate)` rather than private so
    /// `tools.rs`'s own tests can assert an end-to-end call sequence
    /// only parsed once, not just that this module's unit tests do.
    #[cfg(test)]
    pub(crate) fn load_count_for_tests(&self) -> usize {
        self.loads.load(Ordering::Relaxed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::thread::sleep;
    use std::time::Duration;

    fn write(dir: &Path, relative: &str, content: &str) {
        let path = dir.join(relative);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, content).unwrap();
    }

    fn sample_bundle() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        write(
            dir.path(),
            "functions/f.md",
            "---\ntype: Rust Function\ntitle: f\nresource: src/lib.rs#L1\n---\n\nbody\n",
        );
        dir
    }

    #[test]
    fn first_call_loads_and_returns_the_right_concepts() {
        let dir = sample_bundle();
        let cache = BundleCache::new();
        let concepts = cache.get_or_load(dir.path()).unwrap();
        assert_eq!(concepts.len(), 1);
        assert_eq!(concepts[0].id, "functions/f");
        assert_eq!(cache.load_count_for_tests(), 1);
    }

    #[test]
    fn repeated_calls_against_an_unchanged_bundle_reuse_the_cached_parse() {
        let dir = sample_bundle();
        let cache = BundleCache::new();
        cache.get_or_load(dir.path()).unwrap();
        cache.get_or_load(dir.path()).unwrap();
        cache.get_or_load(dir.path()).unwrap();
        // Three calls, but only the first one actually parsed anything.
        assert_eq!(cache.load_count_for_tests(), 1);
    }

    #[test]
    fn a_new_concept_file_invalidates_the_cache() {
        let dir = sample_bundle();
        let cache = BundleCache::new();
        let first = cache.get_or_load(dir.path()).unwrap();
        assert_eq!(first.len(), 1);

        write(
            dir.path(),
            "functions/g.md",
            "---\ntype: Rust Function\ntitle: g\nresource: src/lib.rs#L2\n---\n\nbody\n",
        );
        let second = cache.get_or_load(dir.path()).unwrap();
        assert_eq!(second.len(), 2);
        assert_eq!(cache.load_count_for_tests(), 2);
    }

    #[test]
    fn editing_a_concept_files_content_invalidates_the_cache() {
        let dir = sample_bundle();
        let cache = BundleCache::new();
        let first = cache.get_or_load(dir.path()).unwrap();
        assert!(first[0].description.is_none());

        // Sleep past filesystem mtime granularity so the rewritten
        // file's modified time is guaranteed to differ, the same
        // assumption any mtime-based cache relies on.
        sleep(Duration::from_millis(20));
        write(
            dir.path(),
            "functions/f.md",
            "---\ntype: Rust Function\ntitle: f\ndescription: now documented\nresource: src/lib.rs#L1\n---\n\nbody\n",
        );
        let second = cache.get_or_load(dir.path()).unwrap();
        assert_eq!(second[0].description.as_deref(), Some("now documented"));
        assert_eq!(cache.load_count_for_tests(), 2);
    }

    #[test]
    fn deleting_a_concept_file_invalidates_the_cache() {
        let dir = sample_bundle();
        write(
            dir.path(),
            "functions/g.md",
            "---\ntype: Rust Function\ntitle: g\nresource: src/lib.rs#L2\n---\n\nbody\n",
        );
        let cache = BundleCache::new();
        let first = cache.get_or_load(dir.path()).unwrap();
        assert_eq!(first.len(), 2);

        fs::remove_file(dir.path().join("functions/g.md")).unwrap();
        let second = cache.get_or_load(dir.path()).unwrap();
        assert_eq!(second.len(), 1);
        assert_eq!(cache.load_count_for_tests(), 2);
    }

    #[test]
    fn touching_an_unrelated_non_md_file_does_not_invalidate_the_cache() {
        let dir = sample_bundle();
        let cache = BundleCache::new();
        cache.get_or_load(dir.path()).unwrap();

        write(dir.path(), "notes.txt", "scratch notes, not a concept");
        cache.get_or_load(dir.path()).unwrap();
        assert_eq!(cache.load_count_for_tests(), 1);
    }

    #[test]
    fn missing_bundle_is_a_clear_error_and_never_gets_cached() {
        let cache = BundleCache::new();
        let err = cache.get_or_load(Path::new("/nonexistent")).unwrap_err();
        assert!(err.to_string().contains("okf-rs generate"));
        assert_eq!(cache.load_count_for_tests(), 0);
    }
}
