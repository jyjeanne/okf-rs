//! Basic bundle search: by symbol, package, module, type, and tag.
//!
//! Phase 1 scope only — this is deliberately not relationship-aware (no
//! "what calls X" queries); that needs the project-wide graph, which lands
//! with `okf-graph` in Phase 2. This index just matches free text against
//! each concept's title, frontmatter `type`, and tags.

use anyhow::{Context, Result};
use std::cmp::Reverse;
use std::fs;
use std::path::Path;

#[derive(Debug, Clone)]
pub struct SearchEntry {
    /// Concept id (bundle-relative path without `.md`).
    pub id: String,
    pub title: String,
    /// Raw frontmatter `type` value, e.g. "Rust Function".
    pub concept_type: String,
    pub tags: Vec<String>,
}

#[derive(Debug, Default)]
pub struct SearchIndex {
    entries: Vec<SearchEntry>,
}

#[derive(Debug)]
pub struct SearchHit<'a> {
    pub entry: &'a SearchEntry,
    pub score: u32,
}

impl SearchIndex {
    /// Walks `bundle_dir` and indexes every concept file's frontmatter
    /// (skipping `index.md` navigation pages, which carry no frontmatter).
    pub fn build(bundle_dir: &Path) -> Result<Self> {
        let mut entries = Vec::new();
        for entry in walkdir::WalkDir::new(bundle_dir) {
            let entry = entry.context("failed to walk bundle directory")?;
            if !entry.file_type().is_file() {
                continue;
            }
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("md") {
                continue;
            }
            if path.file_name().and_then(|n| n.to_str()) == Some("index.md") {
                continue;
            }
            let relative = path
                .strip_prefix(bundle_dir)
                .unwrap_or(path)
                .to_string_lossy()
                .replace('\\', "/");
            let id = relative
                .strip_suffix(".md")
                .unwrap_or(&relative)
                .to_string();
            // Normalize CRLF to LF, matching okf-validator, so a file
            // edited on Windows still has its frontmatter recognized.
            let content = fs::read_to_string(path)
                .with_context(|| format!("failed to read {relative}"))?
                .replace("\r\n", "\n");
            if let Some(entry) = parse_entry(id, &content) {
                entries.push(entry);
            }
        }
        entries.sort_by(|a, b| a.id.cmp(&b.id));
        Ok(SearchIndex { entries })
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Free-text search across symbol/title, type, and tags. Case
    /// insensitive. Results are ranked (exact title match highest) and
    /// ties are broken by id for deterministic output.
    pub fn search(&self, query: &str) -> Vec<SearchHit<'_>> {
        let query = query.trim().to_ascii_lowercase();
        if query.is_empty() {
            return Vec::new();
        }

        let mut hits: Vec<SearchHit> = self
            .entries
            .iter()
            .filter_map(|entry| score(entry, &query).map(|score| SearchHit { entry, score }))
            .collect();

        hits.sort_by_key(|hit| (Reverse(hit.score), hit.entry.id.clone()));
        hits
    }
}

fn score(entry: &SearchEntry, query: &str) -> Option<u32> {
    let title = entry.title.to_ascii_lowercase();
    let concept_type = entry.concept_type.to_ascii_lowercase();
    let id = entry.id.to_ascii_lowercase();

    if title == *query {
        return Some(100);
    }
    if title.starts_with(query) {
        return Some(80);
    }
    if entry.tags.iter().any(|t| t.to_ascii_lowercase() == *query) {
        return Some(70);
    }
    if title.contains(query) {
        return Some(60);
    }
    if concept_type.contains(query) {
        return Some(40);
    }
    if id.contains(query) {
        return Some(20);
    }
    None
}

fn parse_entry(id: String, content: &str) -> Option<SearchEntry> {
    let rest = content.strip_prefix("---\n")?;
    let end = rest.find("\n---\n")?;
    let yaml = &rest[..end];
    let value: serde_yaml::Value = serde_yaml::from_str(yaml).ok()?;
    let mapping = value.as_mapping()?;

    let concept_type = mapping.get("type")?.as_str()?.to_string();
    let title = mapping
        .get("title")
        .and_then(|v| v.as_str())
        .map(str::to_string)
        .unwrap_or_else(|| id.rsplit('/').next().unwrap_or(&id).to_string());
    let tags = mapping
        .get("tags")
        .and_then(|v| v.as_sequence())
        .map(|seq| {
            seq.iter()
                .filter_map(|t| t.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default();

    Some(SearchEntry {
        id,
        title,
        concept_type,
        tags,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(dir: &Path, relative: &str, content: &str) {
        let path = dir.join(relative);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, content).unwrap();
    }

    #[test]
    fn ranks_exact_title_match_first() {
        let dir = tempfile::tempdir().unwrap();
        write(dir.path(), "index.md", "nav");
        write(
            dir.path(),
            "functions/verify.md",
            "---\ntype: Rust Function\ntitle: verify\ntags: [auth]\n---\n\nbody\n",
        );
        write(
            dir.path(),
            "functions/verify_token.md",
            "---\ntype: Rust Function\ntitle: verify_token\n---\n\nbody\n",
        );

        let index = SearchIndex::build(dir.path()).unwrap();
        assert_eq!(index.len(), 2);

        let hits = index.search("verify");
        assert_eq!(hits.len(), 2);
        assert_eq!(hits[0].entry.title, "verify");
        assert_eq!(hits[0].score, 100);
        assert_eq!(hits[1].entry.title, "verify_token");
    }

    #[test]
    fn matches_by_tag_and_type() {
        let dir = tempfile::tempdir().unwrap();
        write(
            dir.path(),
            "functions/verify.md",
            "---\ntype: Rust Function\ntitle: verify\ntags: [security]\n---\n\nbody\n",
        );

        let index = SearchIndex::build(dir.path()).unwrap();
        assert_eq!(index.search("security").len(), 1);
        assert_eq!(index.search("function").len(), 1);
        assert!(index.search("nonexistent").is_empty());
    }
}
