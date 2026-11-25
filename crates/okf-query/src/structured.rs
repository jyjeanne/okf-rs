//! JSON renderings of the queries a program, rather than a person or a
//! model, reads: `search_ranked`, `explore`, `callers`/`callees`, and the
//! concepts defined in one file. Each document carries a versioned
//! `schema` name (the same convention as `plan-change`'s
//! [`crate::PLAN_CHANGE_SCHEMA`]), so a caller can refuse a shape it does
//! not know instead of misreading it.
//!
//! The text forms stay the default everywhere; these are opt-in (the MCP
//! server's `"format": "json"` argument).

use crate::{production_first, require_bundle, require_concept};
use anyhow::{anyhow, Result};
use okf_parser::Concept;
use serde::Serialize;
use std::path::Path;

/// Schema name of [`search_ranked_json`]'s document.
pub const SEARCH_RANKED_SCHEMA: &str = "okf-search-ranked/1";
/// Schema name of [`explore_json_from_concepts`]'s document.
pub const EXPLORE_SCHEMA: &str = "okf-explore/1";
/// Schema name of the concept lists: callers, callees, and a file's concepts.
pub const CONCEPTS_SCHEMA: &str = "okf-concepts/1";

/// At most this many direct callers (and, separately, callees) are listed
/// by [`explore_json_from_concepts`]; the totals are always exact.
const EXPLORE_MAX_NEIGHBORS: usize = 20;

/// Where a concept is defined: a path relative to the project root, with
/// `/` separators, and a 1-based line range.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct SourceRange {
    /// Path relative to the project root.
    pub file: String,
    /// First line of the definition.
    pub start_line: usize,
    /// Last line of the definition.
    pub end_line: usize,
}

/// A concept as the JSON documents list it.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct ConceptRef {
    /// Concept id, e.g. `functions/auth/verify_token`.
    pub id: String,
    /// The concept's name, as its `title` frontmatter field.
    pub title: String,
    /// The concept's type, as its `type` frontmatter field.
    #[serde(rename = "type")]
    pub concept_type: String,
    /// Where it is defined; `None` for a concept with no source file.
    pub location: Option<SourceRange>,
}

impl ConceptRef {
    /// The listed form of `concept`.
    pub fn of(concept: &Concept) -> Self {
        let location = (!concept.location.file.is_empty()).then(|| SourceRange {
            file: concept.location.file.clone(),
            start_line: concept.location.start_line,
            end_line: concept.location.end_line,
        });
        Self {
            id: concept.id.clone(),
            title: concept.name.clone(),
            concept_type: concept.frontmatter_type(),
            location,
        }
    }
}

/// One `search_ranked` hit.
#[derive(Debug, Clone, Serialize)]
pub struct RankedHit {
    /// Concept id.
    pub id: String,
    /// The concept's name.
    pub title: String,
    /// The concept's type.
    #[serde(rename = "type")]
    pub concept_type: String,
    /// BM25 relevance; higher is better, comparable only within one query.
    pub score: f32,
}

#[derive(Serialize)]
struct SearchRankedDocument<'a> {
    schema: &'static str,
    query: &'a str,
    hits: Vec<RankedHit>,
}

/// [`crate::search_ranked`] as JSON: `{"schema", "query", "hits": [{"id",
/// "title", "type", "score"}]}`, best first, at most 25 hits. No hits is an
/// empty list, not an error.
pub fn search_ranked_json(bundle: &Path, query: &str) -> Result<String> {
    require_bundle(bundle)?;
    let index = okf_search::FullTextIndex::build(bundle)?;
    search_ranked_json_from_index(&index, query)
}

/// Same as [`search_ranked_json`], against an index already built — see
/// [`crate::search_ranked_from_index`].
pub fn search_ranked_json_from_index(
    index: &okf_search::FullTextIndex,
    query: &str,
) -> Result<String> {
    let hits = index
        .search(query, 25)?
        .into_iter()
        .map(|hit| RankedHit {
            id: hit.id,
            title: hit.title,
            concept_type: hit.concept_type,
            score: hit.score,
        })
        .collect();
    to_json(&SearchRankedDocument {
        schema: SEARCH_RANKED_SCHEMA,
        query,
        hits,
    })
}

#[derive(Serialize)]
struct ExploreDocument {
    schema: &'static str,
    #[serde(flatten)]
    concept: ConceptRef,
    signature: Option<String>,
    description: Option<String>,
    callers_total: usize,
    callers: Vec<ConceptRef>,
    callees_total: usize,
    callees: Vec<ConceptRef>,
    blast_radius_total: usize,
    public_api: bool,
    in_cycle: bool,
}

/// [`crate::explore_from_concepts`] as JSON. `query` resolves the same way:
/// an exact concept id wins, otherwise the top ranked-search hit. Callers
/// and callees are capped at 20 each (production code first), with exact
/// totals beside them.
pub fn explore_json_from_concepts(concepts: &[Concept], query: &str) -> Result<String> {
    let graph = okf_graph::Graph::build(concepts);
    let concept = match graph.get(query) {
        Some(concept) => concept,
        None => crate::resolve_explore_target_by_search(&graph, concepts, query)?,
    };
    let callers = production_first(&graph.callers(&concept.id));
    let callees = production_first(&graph.callees(&concept.id));
    let blast_radius_total = production_first(&graph.transitive_callers(&concept.id, None)).len();
    let in_cycle = graph
        .cycles()
        .into_iter()
        .flatten()
        .any(|id| id == concept.id);
    let capped = |list: &[&Concept]| -> Vec<ConceptRef> {
        list.iter()
            .take(EXPLORE_MAX_NEIGHBORS)
            .map(|c| ConceptRef::of(c))
            .collect()
    };
    to_json(&ExploreDocument {
        schema: EXPLORE_SCHEMA,
        concept: ConceptRef::of(concept),
        signature: concept.signature.clone(),
        description: concept.description.clone(),
        callers_total: callers.len(),
        callers: capped(&callers),
        callees_total: callees.len(),
        callees: capped(&callees),
        blast_radius_total,
        public_api: okf_graph::is_public_api(concept),
        in_cycle,
    })
}

#[derive(Serialize)]
struct ConceptsDocument<'a> {
    schema: &'static str,
    relation: &'a str,
    /// The concept id (callers, callees) or the path (file) asked about.
    subject: &'a str,
    concepts: Vec<ConceptRef>,
}

/// Which direct neighbours [`neighbours_json_from_concepts`] lists.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Neighbours {
    /// Concepts that directly call the subject.
    Callers,
    /// Concepts the subject directly calls.
    Callees,
}

/// [`crate::graph_callers_from_concepts`] or
/// [`crate::graph_callees_from_concepts`] as JSON: every direct neighbour,
/// uncapped, ordered by id. An unknown id is an error, as in the text form.
pub fn neighbours_json_from_concepts(
    concepts: &[Concept],
    id: &str,
    which: Neighbours,
) -> Result<String> {
    let graph = okf_graph::Graph::build(concepts);
    require_concept(&graph, id)?;
    let (relation, mut found) = match which {
        Neighbours::Callers => ("callers", graph.callers(id)),
        Neighbours::Callees => ("callees", graph.callees(id)),
    };
    found.sort_by(|a, b| a.id.cmp(&b.id));
    found.dedup_by(|a, b| a.id == b.id);
    to_json(&ConceptsDocument {
        schema: CONCEPTS_SCHEMA,
        relation,
        subject: id,
        concepts: found.into_iter().map(ConceptRef::of).collect(),
    })
}

/// The concepts defined in `path` (relative to the project root; `\` and
/// a leading `./` are accepted), ordered by first line. Lets a caller map
/// a file it touched to concepts without guessing how ids are formed.
pub fn file_concepts_from_concepts<'a>(concepts: &'a [Concept], path: &str) -> Vec<&'a Concept> {
    let wanted = normalise_path(path);
    let mut found: Vec<&Concept> = concepts
        .iter()
        .filter(|c| !c.location.file.is_empty() && normalise_path(&c.location.file) == wanted)
        .collect();
    found.sort_by(|a, b| (a.location.start_line, &a.id).cmp(&(b.location.start_line, &b.id)));
    found
}

/// [`file_concepts_from_concepts`] as text: one `id (file#Lstart-Lend)`
/// line per concept.
pub fn file_concepts_text(concepts: &[Concept], path: &str) -> Result<String> {
    let found = file_concepts_from_concepts(concepts, path);
    if found.is_empty() {
        return Ok(format!("No concepts found in `{path}`"));
    }
    Ok(found
        .iter()
        .map(|c| format!("{} ({}) — {}", c.id, c.location, c.frontmatter_type()))
        .collect::<Vec<_>>()
        .join("\n"))
}

/// [`file_concepts_from_concepts`] as JSON. A file with no concepts is an
/// empty list, not an error.
pub fn file_concepts_json(concepts: &[Concept], path: &str) -> Result<String> {
    let found = file_concepts_from_concepts(concepts, path);
    to_json(&ConceptsDocument {
        schema: CONCEPTS_SCHEMA,
        relation: "file",
        subject: path,
        concepts: found.into_iter().map(ConceptRef::of).collect(),
    })
}

fn normalise_path(path: &str) -> String {
    let path = path.trim().replace('\\', "/");
    path.trim_start_matches("./").to_string()
}

fn to_json<T: Serialize>(value: &T) -> Result<String> {
    serde_json::to_string_pretty(value).map_err(|e| anyhow!("could not render JSON: {e}"))
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

    fn sample_bundle() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        write(
            dir.path(),
            "functions/auth/verify_token.md",
            "---\ntype: Rust Function\ntitle: verify_token\nresource: src/auth.rs#L1-L4\nrelationships:\n  calls:\n    - functions/auth/decode_jwt\n---\n\nbody\n",
        );
        write(
            dir.path(),
            "functions/auth/decode_jwt.md",
            "---\ntype: Rust Function\ntitle: decode_jwt\nresource: src/auth.rs#L6-L9\nrelationships:\n  called_by:\n    - functions/auth/verify_token\n---\n\nbody\n",
        );
        write(
            dir.path(),
            "functions/db/connect.md",
            "---\ntype: Rust Function\ntitle: connect\nresource: src/db.rs#L1-L3\n---\n\nbody\n",
        );
        dir
    }

    fn json(text: &str) -> serde_json::Value {
        serde_json::from_str(text).expect("valid JSON")
    }

    #[test]
    fn search_ranked_json_lists_hits_with_titles() {
        let dir = sample_bundle();
        let doc = json(&search_ranked_json(dir.path(), "decode_jwt").unwrap());
        assert_eq!(doc["schema"], SEARCH_RANKED_SCHEMA);
        let hit = doc["hits"]
            .as_array()
            .unwrap()
            .iter()
            .find(|h| h["id"] == "functions/auth/decode_jwt")
            .expect("the concept is a hit");
        assert_eq!(hit["title"], "decode_jwt");
        assert_eq!(hit["type"], "Rust Function");
    }

    #[test]
    fn search_ranked_json_with_no_hits_is_an_empty_list() {
        let dir = sample_bundle();
        let doc = json(&search_ranked_json(dir.path(), "zzzznothing").unwrap());
        assert_eq!(doc["hits"], serde_json::json!([]));
    }

    #[test]
    fn explore_json_has_location_and_neighbours() {
        let dir = sample_bundle();
        let concepts = crate::load_concepts(dir.path()).unwrap();
        let doc =
            json(&explore_json_from_concepts(&concepts, "functions/auth/decode_jwt").unwrap());
        assert_eq!(doc["schema"], EXPLORE_SCHEMA);
        assert_eq!(doc["id"], "functions/auth/decode_jwt");
        assert_eq!(doc["location"]["file"], "src/auth.rs");
        assert_eq!(doc["location"]["start_line"], 6);
        assert_eq!(doc["location"]["end_line"], 9);
        assert_eq!(doc["callers_total"], 1);
        assert_eq!(doc["callers"][0]["id"], "functions/auth/verify_token");
        assert_eq!(doc["callers"][0]["location"]["start_line"], 1);
        assert_eq!(doc["callees_total"], 0);
    }

    #[test]
    fn neighbours_json_lists_callers_and_callees() {
        let dir = sample_bundle();
        let concepts = crate::load_concepts(dir.path()).unwrap();
        let callers = json(
            &neighbours_json_from_concepts(
                &concepts,
                "functions/auth/decode_jwt",
                Neighbours::Callers,
            )
            .unwrap(),
        );
        assert_eq!(callers["schema"], CONCEPTS_SCHEMA);
        assert_eq!(callers["relation"], "callers");
        assert_eq!(callers["concepts"][0]["id"], "functions/auth/verify_token");
        let callees = json(
            &neighbours_json_from_concepts(
                &concepts,
                "functions/auth/verify_token",
                Neighbours::Callees,
            )
            .unwrap(),
        );
        assert_eq!(callees["concepts"][0]["id"], "functions/auth/decode_jwt");
    }

    #[test]
    fn neighbours_json_refuses_an_unknown_id() {
        let dir = sample_bundle();
        let concepts = crate::load_concepts(dir.path()).unwrap();
        let err = neighbours_json_from_concepts(&concepts, "functions/nope", Neighbours::Callers)
            .unwrap_err();
        assert!(err.to_string().contains("no concept with id"));
    }

    #[test]
    fn file_concepts_are_the_ones_defined_in_that_file_in_line_order() {
        let dir = sample_bundle();
        let concepts = crate::load_concepts(dir.path()).unwrap();
        let ids: Vec<&str> = file_concepts_from_concepts(&concepts, "./src\\auth.rs")
            .iter()
            .map(|c| c.id.as_str())
            .collect();
        assert_eq!(
            ids,
            ["functions/auth/verify_token", "functions/auth/decode_jwt"]
        );

        let doc = json(&file_concepts_json(&concepts, "src/db.rs").unwrap());
        assert_eq!(doc["relation"], "file");
        assert_eq!(doc["concepts"][0]["id"], "functions/db/connect");

        let none = json(&file_concepts_json(&concepts, "src/missing.rs").unwrap());
        assert_eq!(none["concepts"], serde_json::json!([]));
        assert!(file_concepts_text(&concepts, "src/missing.rs")
            .unwrap()
            .contains("No concepts found"));
        assert!(file_concepts_text(&concepts, "src/db.rs")
            .unwrap()
            .contains("functions/db/connect (src/db.rs#L1-L3)"));
    }
}
