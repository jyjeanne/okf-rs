//! Content-hash-keyed cache of per-file tree-sitter extractions, so a
//! later `analyze_with_cache` run can skip re-parsing files whose content
//! hasn't changed since the cache was last saved.
//!
//! Purely a performance optimization: a cache miss just costs a re-parse,
//! never a correctness issue. The content hash itself doesn't need to be
//! portable or stable across okf-rs versions -- but the *extraction logic*
//! a hit reuses does need to match the running build, or an upgrade that
//! fixes an extractor bug would silently keep serving the old, pre-fix
//! output for every unchanged file. `AnalysisCache::load` guards against
//! that by stamping (and checking) the okf-rs version that wrote the
//! cache, invalidating it wholesale on a version mismatch.

use okf_tree_sitter::FileExtraction;
use serde::{Deserialize, Serialize};
use std::collections::hash_map::DefaultHasher;
use std::collections::BTreeMap;
use std::hash::{Hash, Hasher};
use std::path::Path;

/// The okf-rs version stamped into a saved cache -- see the module docs.
const CACHE_VERSION: &str = env!("CARGO_PKG_VERSION");

/// A saved (or in-progress) cache, keyed by each file's project-relative
/// path. Serializes with keys in sorted order (`BTreeMap`), so the cache
/// file itself is stable to diff across runs rather than shuffling on
/// every save the way a `HashMap`'s iteration order would.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AnalysisCache {
    #[serde(default)]
    version: String,
    entries: BTreeMap<String, CacheEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct CacheEntry {
    content_hash: u64,
    extraction: FileExtraction,
}

impl AnalysisCache {
    /// Loads a previously saved cache from `path`. A missing file, one
    /// that fails to read or parse, or one saved by a different okf-rs
    /// version than this build, is the normal non-fatal case and falls
    /// back to an empty cache rather than erroring — every file is simply
    /// treated as a cache miss and re-parsed (with this build's own
    /// extraction logic, never a stale one from whatever version wrote
    /// the file being discarded).
    pub fn load(path: &Path) -> AnalysisCache {
        let cache: AnalysisCache = std::fs::read_to_string(path)
            .ok()
            .and_then(|content| serde_json::from_str(&content).ok())
            .unwrap_or_default();
        if cache.version == CACHE_VERSION {
            cache
        } else {
            AnalysisCache::default()
        }
    }

    /// Persists the cache to `path` as JSON, stamped with this build's
    /// okf-rs version (see the module docs).
    pub fn save(&mut self, path: &Path) -> anyhow::Result<()> {
        self.version = CACHE_VERSION.to_string();
        let content = serde_json::to_string_pretty(self)?;
        std::fs::write(path, content)?;
        Ok(())
    }

    /// The cached extraction for `relative_path`, if present and its
    /// stored content hash still matches `content_hash` (i.e. the file
    /// hasn't changed since the cache was saved).
    pub(crate) fn get(&self, relative_path: &str, content_hash: u64) -> Option<FileExtraction> {
        let entry = self.entries.get(relative_path)?;
        if entry.content_hash != content_hash {
            return None;
        }
        Some(entry.extraction.clone())
    }

    /// Records `extraction` for `relative_path` under `content_hash`.
    /// `analyze_with_cache` builds a fresh `AnalysisCache` this way on
    /// every run (rather than mutating the loaded one in place), so files
    /// removed from the project since the last run are naturally dropped
    /// instead of accumulating as stale entries forever.
    pub(crate) fn insert(
        &mut self,
        relative_path: &str,
        content_hash: u64,
        extraction: FileExtraction,
    ) {
        self.entries.insert(
            relative_path.to_string(),
            CacheEntry {
                content_hash,
                extraction,
            },
        );
    }

    #[cfg(test)]
    pub(crate) fn len(&self) -> usize {
        self.entries.len()
    }
}

/// A fast, non-cryptographic hash of `content`, used only for cache
/// invalidation (see the module docs on why stability across
/// processes/versions doesn't matter here).
pub(crate) fn hash_content(content: &str) -> u64 {
    let mut hasher = DefaultHasher::new();
    content.hash(&mut hasher);
    hasher.finish()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_through_save_and_load() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(".okf-cache.json");

        let mut cache = AnalysisCache::default();
        cache.insert(
            "src/lib.rs",
            42,
            FileExtraction {
                concepts: Vec::new(),
                calls: Vec::new(),
            },
        );
        cache.save(&path).unwrap();

        let loaded = AnalysisCache::load(&path);
        assert_eq!(loaded.len(), 1);
        assert!(loaded.get("src/lib.rs", 42).is_some());
        assert!(
            loaded.get("src/lib.rs", 43).is_none(),
            "hash mismatch should miss"
        );
        assert!(loaded.get("src/other.rs", 42).is_none());
    }

    #[test]
    fn a_cache_saved_by_a_different_okf_rs_version_is_discarded() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(".okf-cache.json");

        std::fs::write(
            &path,
            r#"{"version":"0.0.0-not-this-build","entries":{"src/lib.rs":{"content_hash":42,"extraction":{"concepts":[],"calls":[]}}}}"#,
        )
        .unwrap();

        let loaded = AnalysisCache::load(&path);
        assert_eq!(
            loaded.len(),
            0,
            "a cache stamped with a different okf-rs version must be treated as empty, \
             not silently reused"
        );
    }

    #[test]
    fn missing_or_corrupt_file_is_an_empty_cache_not_an_error() {
        let dir = tempfile::tempdir().unwrap();
        let missing = AnalysisCache::load(&dir.path().join("nope.json"));
        assert_eq!(missing.len(), 0);

        let corrupt_path = dir.path().join("corrupt.json");
        std::fs::write(&corrupt_path, "not json").unwrap();
        let corrupt = AnalysisCache::load(&corrupt_path);
        assert_eq!(corrupt.len(), 0);
    }
}
