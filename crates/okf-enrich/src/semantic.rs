//! Semantic (embedding-based) search: ranks concepts by meaning rather
//! than exact wording, via any OpenAI-compatible `/embeddings` endpoint
//! (see [`EnrichClient::embed`]) — layered on top of, not replacing,
//! `okf-search`'s exact/substring and ranked full-text search. Local-first
//! by default (point `--enrich-base-url` at a local embedding model, e.g.
//! Ollama's `nomic-embed-text`), with a cloud provider equally supported
//! through the same OpenAI-compatible shape every other `okf-enrich`
//! feature already targets — never a hard dependency on one vendor.
//!
//! Only concepts with a non-empty description are embedded and
//! considered: an embedding of a bare identifier carries little more
//! semantic signal than the identifier itself, so `okf-search`'s existing
//! exact/ranked search already covers that case at zero network cost.

use crate::EnrichClient;
use anyhow::Result;
use okf_parser::Concept;
use serde::Serialize;

/// One semantically-ranked search result — see [`semantic_search`].
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct SemanticHit {
    pub id: String,
    pub title: String,
    pub concept_type: String,
    /// Cosine similarity to the query embedding, in `[-1.0, 1.0]` (in
    /// practice close to `[0.0, 1.0]` for real embedding models, whose
    /// vectors are rarely near-opposite) — higher is more similar.
    pub score: f32,
}

fn embeddable_text(concept: &Concept, description: &str) -> String {
    format!(
        "{} ({}): {description}",
        concept.qualified_name,
        concept.frontmatter_type()
    )
}

fn vector_norm(v: &[f32]) -> f32 {
    v.iter().map(|x| x * x).sum::<f32>().sqrt()
}

/// Cosine similarity between `a` and `b`, given `a`'s already-computed
/// norm — lets a caller comparing one fixed vector (the query embedding)
/// against many candidates compute that norm once, rather than
/// recomputing the same value on every comparison (see
/// [`semantic_search`], which does exactly that).
fn cosine_similarity_with_norm(a: &[f32], norm_a: f32, b: &[f32]) -> f32 {
    if a.len() != b.len() || a.is_empty() {
        return 0.0;
    }
    let dot: f32 = a.iter().zip(b).map(|(x, y)| x * y).sum();
    let norm_b = vector_norm(b);
    if norm_a == 0.0 || norm_b == 0.0 {
        return 0.0;
    }
    dot / (norm_a * norm_b)
}

#[cfg(test)]
fn cosine_similarity(a: &[f32], b: &[f32]) -> f32 {
    cosine_similarity_with_norm(a, vector_norm(a), b)
}

/// Ranks every described concept in `concepts` by embedding-cosine
/// similarity to `query`, via `client`'s configured `/embeddings`
/// endpoint. One embedding call per described concept plus one for
/// `query` — bounded by the bundle's described-concept count, not free,
/// so this is meant to be run on demand (`okf-rs search --semantic`), not
/// on every keystroke, the same cost posture `suggest_missing_links`
/// already has for its own per-candidate endpoint calls.
///
/// Returns at most `limit` hits, sorted by score descending (ties broken
/// by id, so results are deterministic across repeated runs against the
/// same endpoint and bundle).
pub fn semantic_search(
    client: &EnrichClient,
    concepts: &[Concept],
    query: &str,
    limit: usize,
) -> Result<Vec<SemanticHit>> {
    let candidates: Vec<(&Concept, String)> = concepts
        .iter()
        .filter_map(|c| {
            c.description
                .as_deref()
                .filter(|d| !d.trim().is_empty())
                .map(|d| (c, embeddable_text(c, d)))
        })
        .collect();

    if candidates.is_empty() {
        return Ok(Vec::new());
    }

    let query_embedding = client.embed(query)?;
    let query_norm = vector_norm(&query_embedding);
    let embeddings = crate::run_bounded(&candidates, |(_, text)| client.embed(text));

    let mut hits = Vec::with_capacity(candidates.len());
    for ((concept, _), embedding) in candidates.iter().zip(embeddings) {
        let embedding = embedding?;
        hits.push(SemanticHit {
            id: concept.id.clone(),
            title: concept.name.clone(),
            concept_type: concept.frontmatter_type(),
            score: cosine_similarity_with_norm(&query_embedding, query_norm, &embedding),
        });
    }

    hits.sort_by(|a, b| b.score.total_cmp(&a.score).then_with(|| a.id.cmp(&b.id)));
    hits.truncate(limit);
    Ok(hits)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{embedding_client_for, start_embedding_mock_server};
    use okf_parser::{ConceptKind, Language, Location};
    use std::collections::HashMap;
    use std::sync::atomic::Ordering;

    fn concept(id: &str, description: Option<&str>) -> Concept {
        Concept {
            id: id.to_string(),
            kind: ConceptKind::Function,
            language: Language::Rust,
            name: id.rsplit('/').next().unwrap().to_string(),
            qualified_name: id.replace('/', "."),
            description: description.map(str::to_string),
            location: Location {
                file: "src/lib.rs".to_string(),
                start_line: 1,
                end_line: 1,
            },
            signature: None,
            tags: Vec::new(),
            is_public: true,
            generated_at: None,
            relationships: Vec::new(),
        }
    }

    #[test]
    fn ranks_the_closer_embedding_first() {
        let token_fn = concept(
            "functions/verify_token",
            Some("Verifies a signed authentication token."),
        );
        let currency_fn = concept(
            "functions/format_currency",
            Some("Formats a currency amount for display."),
        );

        let mut responses = HashMap::new();
        responses.insert(
            embeddable_text(&token_fn, token_fn.description.as_deref().unwrap()),
            vec![1.0, 0.0],
        );
        responses.insert(
            embeddable_text(&currency_fn, currency_fn.description.as_deref().unwrap()),
            vec![0.0, 1.0],
        );
        responses.insert("authentication token".to_string(), vec![1.0, 0.0]);

        let server = start_embedding_mock_server(responses);
        let client = embedding_client_for(&server);

        let hits = semantic_search(
            &client,
            &[token_fn.clone(), currency_fn.clone()],
            "authentication token",
            10,
        )
        .unwrap();

        assert_eq!(hits.len(), 2);
        assert_eq!(hits[0].id, "functions/verify_token");
        assert!(hits[0].score > hits[1].score);
        assert_eq!(hits[1].id, "functions/format_currency");
    }

    #[test]
    fn concepts_without_a_description_are_never_embedded_or_returned() {
        let described = concept("functions/a", Some("does something"));
        let undescribed = concept("functions/b", None);

        let mut responses = HashMap::new();
        responses.insert(
            embeddable_text(&described, "does something"),
            vec![1.0, 0.0],
        );
        responses.insert("query".to_string(), vec![1.0, 0.0]);

        let server = start_embedding_mock_server(responses);
        let client = embedding_client_for(&server);

        let hits = semantic_search(&client, &[described, undescribed], "query", 10).unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].id, "functions/a");
        // One call for the query embedding, one for the single described
        // candidate -- the undescribed concept must never be embedded.
        assert_eq!(server.request_count.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn empty_bundle_of_described_concepts_returns_no_hits_without_a_network_call() {
        let undescribed = concept("functions/a", None);
        let server = start_embedding_mock_server(HashMap::new());
        let client = embedding_client_for(&server);

        let hits = semantic_search(&client, &[undescribed], "query", 10).unwrap();
        assert!(hits.is_empty());
        assert_eq!(server.request_count.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn results_are_capped_at_the_requested_limit() {
        let concepts: Vec<Concept> = (0..5)
            .map(|i| concept(&format!("functions/f{i}"), Some("token handler")))
            .collect();

        let mut responses = HashMap::new();
        for c in &concepts {
            responses.insert(embeddable_text(c, "token handler"), vec![1.0, 0.0]);
        }
        responses.insert("token".to_string(), vec![1.0, 0.0]);

        let server = start_embedding_mock_server(responses);
        let client = embedding_client_for(&server);

        let hits = semantic_search(&client, &concepts, "token", 2).unwrap();
        assert_eq!(hits.len(), 2);
    }

    #[test]
    fn cosine_similarity_of_identical_vectors_is_one() {
        assert!((cosine_similarity(&[1.0, 2.0, 3.0], &[1.0, 2.0, 3.0]) - 1.0).abs() < 1e-6);
    }

    #[test]
    fn cosine_similarity_of_orthogonal_vectors_is_zero() {
        assert!(cosine_similarity(&[1.0, 0.0], &[0.0, 1.0]).abs() < 1e-6);
    }

    #[test]
    fn cosine_similarity_of_a_zero_vector_is_zero_not_nan() {
        assert_eq!(cosine_similarity(&[0.0, 0.0], &[1.0, 2.0]), 0.0);
    }
}
