//! Ranked, relevance-scored full-text search, layered onto the
//! exact/substring [`crate::SearchIndex`] above via [Tantivy](https://github.com/quickwit-oss/tantivy)'s
//! BM25 scoring. Where `SearchIndex` only ever matches a concept's title,
//! type, and tags, this also searches its description and signature, and
//! ranks results by relevance instead of the fixed field-priority scoring
//! `SearchIndex` uses — useful for a query that's closer to natural
//! language than an exact symbol name.
//!
//! Built fresh from a bundle on disk each time (via
//! [`okf_parser::read_bundle`]), the same way `SearchIndex::build` and
//! `okf-graph`/`okf-query` already work — no persistent index file to keep
//! in sync with the bundle.

use anyhow::{Context, Result};
use okf_parser::Concept;
use std::path::Path;
use tantivy::collector::TopDocs;
use tantivy::query::QueryParser;
use tantivy::schema::{Field, Schema, TantivyDocument, Value, STORED, STRING, TEXT};
use tantivy::{doc, Index, IndexReader, ReloadPolicy};

pub struct FullTextHit {
    pub id: String,
    pub title: String,
    pub concept_type: String,
    pub score: f32,
}

pub struct FullTextIndex {
    index: Index,
    reader: IndexReader,
    field_id: Field,
    field_title: Field,
    field_type: Field,
    field_body: Field,
}

impl FullTextIndex {
    /// Reads every concept out of `bundle_dir` (via
    /// [`okf_parser::read_bundle`]) and builds an in-memory Tantivy index
    /// over it. Errors if the bundle can't be read at all; an empty
    /// bundle just produces an empty (but valid) index.
    pub fn build(bundle_dir: &Path) -> Result<Self> {
        let concepts = okf_parser::read_bundle(bundle_dir)
            .with_context(|| format!("failed to read bundle at {}", bundle_dir.display()))?;
        Self::build_from_concepts(&concepts)
    }

    /// Builds the same in-memory Tantivy index as [`FullTextIndex::build`],
    /// but from concepts already in memory rather than reading a bundle
    /// off disk — for a caller (like `okf-enrich`'s missing-link
    /// suggestions) that already has the concept set loaded and would
    /// otherwise have to read the same bundle a second time.
    pub fn build_from_concepts(concepts: &[Concept]) -> Result<Self> {
        let mut schema_builder = Schema::builder();
        let field_id = schema_builder.add_text_field("id", STRING | STORED);
        let field_title = schema_builder.add_text_field("title", TEXT | STORED);
        let field_type = schema_builder.add_text_field("concept_type", TEXT | STORED);
        let field_body = schema_builder.add_text_field("body", TEXT);
        let schema = schema_builder.build();

        let index = Index::create_in_ram(schema);
        {
            // Small, in-memory, rebuilt-from-scratch-per-run index: 15MB is
            // comfortably more than any bundle this tool is realistically
            // pointed at needs before the first (and only) commit.
            let mut writer = index.writer(15_000_000)?;
            for concept in concepts {
                writer.add_document(doc!(
                    field_id => concept.id.as_str(),
                    field_title => split_identifier_words(&concept.name),
                    field_type => concept.frontmatter_type(),
                    field_body => document_body(concept),
                ))?;
            }
            writer.commit()?;
        }

        let reader = index
            .reader_builder()
            .reload_policy(ReloadPolicy::Manual)
            .try_into()?;

        Ok(FullTextIndex {
            index,
            reader,
            field_id,
            field_title,
            field_type,
            field_body,
        })
    }

    pub fn len(&self) -> usize {
        self.reader.searcher().num_docs() as usize
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Ranked full-text search across title, type, description, and
    /// signature. Title matches are boosted above a body/description hit
    /// on the same term. Never errors on malformed query syntax (stray
    /// `:`/`"`/`(` from a natural-language query) — unparseable clauses
    /// are dropped rather than failing the whole search, via Tantivy's
    /// lenient parser. Ties (equal score) are broken by id, so results are
    /// deterministic across runs of an unchanged bundle.
    pub fn search(&self, query: &str, limit: usize) -> Result<Vec<FullTextHit>> {
        let query = query.trim();
        if query.is_empty() {
            return Ok(Vec::new());
        }

        let searcher = self.reader.searcher();
        let mut parser = QueryParser::for_index(
            &self.index,
            vec![self.field_title, self.field_type, self.field_body],
        );
        parser.set_field_boost(self.field_title, 3.0);
        parser.set_field_boost(self.field_type, 1.5);

        let (parsed, _errors) = parser.parse_query_lenient(&split_identifier_words(query));

        // Re-rank a bounded oversample ourselves (rather than trusting
        // `TopDocs::with_limit(limit)` alone) so a tie in Tantivy's BM25
        // score doesn't leave the order dependent on internal doc-id
        // assignment. An 8x oversample (with a floor so a small `limit`
        // still re-ranks a meaningful pool) makes that tie-breaking
        // deterministic for realistic levels of score ambiguity without
        // paying to sort the entire corpus on every call regardless of
        // how small `limit` is.
        let corpus_size = searcher.num_docs().max(1) as usize;
        let capacity = limit.saturating_mul(8).max(50).min(corpus_size);
        let top_docs = searcher.search(&parsed, &TopDocs::with_limit(capacity))?;

        let mut hits = Vec::with_capacity(top_docs.len());
        for (score, address) in top_docs {
            let retrieved: TantivyDocument = searcher.doc(address)?;
            hits.push(FullTextHit {
                id: stored_str(&retrieved, self.field_id),
                title: stored_str(&retrieved, self.field_title),
                concept_type: stored_str(&retrieved, self.field_type),
                score,
            });
        }
        hits.sort_by(|a, b| b.score.total_cmp(&a.score).then_with(|| a.id.cmp(&b.id)));
        hits.truncate(limit);
        Ok(hits)
    }
}

fn stored_str(doc: &TantivyDocument, field: Field) -> String {
    doc.get_first(field)
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_string()
}

/// Everything besides title/type that's worth full-text matching against:
/// the qualified name, description, signature, and tags. Joined with
/// newlines into one field rather than kept as separate fields, since none
/// of them need independent boosting or retrieval — only relevance
/// ranking, which Tantivy already gives per-term regardless of which part
/// of the joined text a term came from.
fn document_body(concept: &Concept) -> String {
    let mut parts = vec![split_identifier_words(&concept.qualified_name)];
    if let Some(description) = &concept.description {
        parts.push(description.clone());
    }
    if let Some(signature) = &concept.signature {
        parts.push(split_identifier_words(signature));
    }
    if !concept.tags.is_empty() {
        parts.push(concept.tags.join(" "));
    }
    parts.join("\n")
}

/// Inserts a space at `snake_case`/`kebab-case`/`camelCase`/`PascalCase`
/// word boundaries so Tantivy's default tokenizer (which already splits on
/// non-alphanumeric characters, so `_`/`-` are handled for free) also
/// splits `verifyToken` into `verify`/`token` — otherwise a search for
/// `verify` would never match a camelCase-named concept, only a
/// snake_case one. Applied symmetrically to both indexed text and query
/// text, so the two stay comparable.
fn split_identifier_words(s: &str) -> String {
    let chars: Vec<char> = s.chars().collect();
    let mut out = String::with_capacity(s.len() + 8);
    for (i, &c) in chars.iter().enumerate() {
        if i > 0 && c.is_uppercase() {
            let prev = chars[i - 1];
            let next = chars.get(i + 1).copied();
            let at_boundary = prev.is_lowercase()
                || prev.is_ascii_digit()
                || (prev.is_uppercase() && next.is_some_and(|n| n.is_lowercase()));
            if at_boundary {
                out.push(' ');
            }
        }
        out.push(c);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn write(dir: &Path, relative: &str, content: &str) {
        let path = dir.join(relative);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, content).unwrap();
    }

    #[test]
    fn splits_snake_kebab_and_camel_case() {
        assert_eq!(split_identifier_words("verify_token"), "verify_token");
        assert_eq!(split_identifier_words("verifyToken"), "verify Token");
        assert_eq!(split_identifier_words("HTTPServer"), "HTTP Server");
        assert_eq!(split_identifier_words("plain"), "plain");
    }

    #[test]
    fn finds_a_concept_by_camel_case_prefix_against_snake_case_name() {
        let dir = tempfile::tempdir().unwrap();
        write(
            dir.path(),
            "functions/auth/verify_token.md",
            "---\ntype: Rust Function\ntitle: verify_token\nresource: src/auth.rs#L1\n---\n\nbody\n",
        );

        let index = FullTextIndex::build(dir.path()).unwrap();
        assert_eq!(index.len(), 1);
        let hits = index.search("verifyToken", 10).unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].id, "functions/auth/verify_token");
    }

    #[test]
    fn build_from_concepts_matches_build_from_a_bundle_on_disk() {
        let dir = tempfile::tempdir().unwrap();
        write(
            dir.path(),
            "functions/auth/verify_token.md",
            "---\ntype: Rust Function\ntitle: verify_token\nresource: src/auth.rs#L1\n---\n\nbody\n",
        );
        let concepts = okf_parser::read_bundle(dir.path()).unwrap();

        let index = FullTextIndex::build_from_concepts(&concepts).unwrap();
        assert_eq!(index.len(), 1);
        let hits = index.search("verify_token", 10).unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].id, "functions/auth/verify_token");
    }

    #[test]
    fn ranks_a_description_hit_below_a_title_hit_for_the_same_term() {
        let dir = tempfile::tempdir().unwrap();
        write(
            dir.path(),
            "functions/auth/token.md",
            "---\ntype: Rust Function\ntitle: token\nresource: src/auth.rs#L1\n---\n\nbody\n",
        );
        write(
            dir.path(),
            "functions/auth/other.md",
            "---\ntype: Rust Function\ntitle: other\ndescription: Reads a token from the request header.\nresource: src/auth.rs#L9\n---\n\nbody\n",
        );

        let index = FullTextIndex::build(dir.path()).unwrap();
        let hits = index.search("token", 10).unwrap();
        assert_eq!(hits.len(), 2);
        assert_eq!(hits[0].id, "functions/auth/token");
        assert_eq!(hits[1].id, "functions/auth/other");
    }

    #[test]
    fn matches_by_description_text_not_reachable_by_exact_substring_search() {
        let dir = tempfile::tempdir().unwrap();
        write(
            dir.path(),
            "functions/auth/f.md",
            "---\ntype: Rust Function\ntitle: f\ndescription: Parses and validates a JSON Web Token.\nresource: src/auth.rs#L1\n---\n\nbody\n",
        );

        let index = FullTextIndex::build(dir.path()).unwrap();
        let hits = index.search("validates", 10).unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].id, "functions/auth/f");
    }

    #[test]
    fn empty_query_returns_no_hits_without_erroring() {
        let dir = tempfile::tempdir().unwrap();
        write(
            dir.path(),
            "functions/f.md",
            "---\ntype: Rust Function\ntitle: f\nresource: src/lib.rs#L1\n---\n\nbody\n",
        );
        let index = FullTextIndex::build(dir.path()).unwrap();
        assert!(index.search("   ", 10).unwrap().is_empty());
    }

    #[test]
    fn malformed_query_syntax_is_handled_leniently_not_an_error() {
        let dir = tempfile::tempdir().unwrap();
        write(
            dir.path(),
            "functions/f.md",
            "---\ntype: Rust Function\ntitle: f\nresource: src/lib.rs#L1\n---\n\nbody\n",
        );
        let index = FullTextIndex::build(dir.path()).unwrap();
        // Unbalanced quote and a bare colon are invalid Tantivy query
        // syntax on their own; the lenient parser must not bubble that up
        // as an `Err` from a free-text search box.
        assert!(index.search("\"unterminated", 10).is_ok());
        assert!(index.search("weird:::query", 10).is_ok());
    }

    #[test]
    fn results_are_capped_at_the_requested_limit() {
        let dir = tempfile::tempdir().unwrap();
        for i in 0..5 {
            write(
                dir.path(),
                &format!("functions/f{i}.md"),
                &format!(
                    "---\ntype: Rust Function\ntitle: f{i}\ndescription: token handler\nresource: src/lib.rs#L{i}\n---\n\nbody\n"
                ),
            );
        }
        let index = FullTextIndex::build(dir.path()).unwrap();
        assert_eq!(index.search("token", 2).unwrap().len(), 2);
    }
}
