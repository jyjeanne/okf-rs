//! Phase E of `docs/improvement-plan-provenance-diff.md`: the
//! specialized-vs-consolidated MCP tool-*selection* benchmark harness.
//!
//! The design decision itself already shipped — `okf-mcp` 0.3.0 collapsed
//! 13 `graph_*` tools into one `graph(relation=...)` tool, based on the
//! schema-size argument alone (see `ROADMAP.md`). What was never measured
//! is whether a model picks the right `relation` value inside that one
//! consolidated tool as reliably as it previously picked the right tool
//! name — the tradeoff a reviewer flagged as a real cost of
//! consolidation, not just a benefit. This module is a **retrospective
//! validation** of that shipped decision, not a live A/B between two
//! options still on the table.
//!
//! This module built the *fully offline, model-free* half of that
//! benchmark first: a fixed question set, each with a known-correct
//! `relation`/answer verified directly against a real fixture bundle (no
//! model involved — every other benchmark in this codebase,
//! [`crate::benchmark`] included, is offline for the same reason), plus
//! the reconstructed pre-0.3.0 specialized-tool-name mapping to compare
//! against. [`crate::tool_selection_live`] is the live-endpoint runner
//! layered on top: it drives [`questions`], [`specialized_tool_name`],
//! [`scores_correctly`], and [`fixture_bundle`] from here against a real
//! OpenAI-compatible model.
//!
//! `#[allow(dead_code)]` below still applies to [`QuestionArgs`]'s
//! variant fields, [`QuestionArgs::to_json`], and [`Question::args`]
//! specifically — not an oversight, but a direct consequence of how the
//! live runner works: it asks a real model to choose its own tool
//! arguments (that's the whole point of a tool-*selection* benchmark),
//! so it never reads a question's own `args`/`to_json` to drive the
//! call. Those stay exercised only by this module's own tests (see `mod
//! tests`), which check `args`/`to_json` against the fixture directly —
//! the source of truth a live run's result gets scored against, even
//! though the live run itself never touches them.
#![allow(dead_code)]

/// How to call the consolidated `graph` tool for one [`Question`] — the
/// argument shape varies by relation (`callers`/`callees`/`isolated`
/// need an `id`; `path`/`explain` need `from`/`to`; everything else
/// needs nothing beyond `relation` itself).
#[derive(Debug, Clone, Copy)]
pub enum QuestionArgs {
    None,
    Id(&'static str),
    FromTo(&'static str, &'static str),
}

impl QuestionArgs {
    /// Builds the `graph` tool's `arguments` object for `relation`.
    pub fn to_json(self, relation: &str) -> serde_json::Value {
        match self {
            QuestionArgs::None => serde_json::json!({ "relation": relation }),
            QuestionArgs::Id(id) => serde_json::json!({ "relation": relation, "id": id }),
            QuestionArgs::FromTo(from, to) => {
                serde_json::json!({ "relation": relation, "from": from, "to": to })
            }
        }
    }
}

/// One representative natural-language question, its known-correct
/// `graph` relation, and the substring a correct answer must contain —
/// checkable directly against [`fixture_bundle`] with no model involved.
#[derive(Debug, Clone, Copy)]
pub struct Question {
    pub prompt: &'static str,
    pub relation: &'static str,
    pub args: QuestionArgs,
    pub expected_substring: &'static str,
}

/// The pre-0.3.0 specialized tool name for `relation`, or `None` for
/// `explain` — which shipped *after* the `graph_*` consolidation this
/// benchmark reconstructs, so it has no historical specialized-tool
/// counterpart to compare against at all (not a gap in this benchmark;
/// see `docs/improvement-plan-provenance-diff.md`'s Phase E). Every
/// other relation's old name was simply `graph_<relation>` — recoverable
/// from `ROADMAP.md`'s own enumeration of the pre-consolidation surface.
pub fn specialized_tool_name(relation: &str) -> Option<String> {
    if relation == "explain" {
        None
    } else {
        Some(format!("graph_{relation}"))
    }
}

/// The fixed question set: one per relation the consolidated `graph`
/// tool exposes today (14 — see `crates/okf-mcp/src/tools.rs`'s
/// `relation` enum), mirroring the kind of question a reviewer's own
/// worked examples used ("Who calls Foo?", "Does the call graph have any
/// cycles?", ...).
pub fn questions() -> Vec<Question> {
    vec![
        Question {
            prompt: "Who calls bar?",
            relation: "callers",
            args: QuestionArgs::Id("functions/pkg-b/bar"),
            expected_substring: "functions/pkg-a/foo",
        },
        Question {
            prompt: "What does foo call?",
            relation: "callees",
            args: QuestionArgs::Id("functions/pkg-a/foo"),
            expected_substring: "functions/pkg-b/bar",
        },
        Question {
            prompt: "What's the shortest call path from foo to bar?",
            relation: "path",
            args: QuestionArgs::FromTo("functions/pkg-a/foo", "functions/pkg-b/bar"),
            expected_substring: "functions/pkg-b/bar",
        },
        Question {
            prompt: "Why does foo call bar?",
            relation: "explain",
            args: QuestionArgs::FromTo("functions/pkg-a/foo", "functions/pkg-b/bar"),
            expected_substring: "Tree-sitter",
        },
        Question {
            prompt: "Is foo part of the public API?",
            relation: "api",
            args: QuestionArgs::None,
            expected_substring: "functions/pkg-a/foo",
        },
        Question {
            prompt: "Does the call graph have any cycles?",
            relation: "cycles",
            args: QuestionArgs::None,
            expected_substring: "functions/pkg-a/cyc_a",
        },
        Question {
            prompt: "What cross-module call dependencies exist?",
            relation: "modules",
            args: QuestionArgs::None,
            expected_substring: "modules/pkg-a",
        },
        Question {
            prompt: "Which concepts have no calls in or out?",
            relation: "isolated",
            args: QuestionArgs::None,
            expected_substring: "functions/pkg-a/lonely",
        },
        Question {
            prompt: "What are this bundle's concept-kind and relationship stats?",
            relation: "stats",
            args: QuestionArgs::None,
            expected_substring: "Function",
        },
        Question {
            prompt: "What layer is each package in?",
            relation: "layers",
            args: QuestionArgs::None,
            expected_substring: "packages/pkg-a",
        },
        Question {
            prompt: "Which packages form a connected domain?",
            relation: "domains",
            args: QuestionArgs::None,
            expected_substring: "packages/pkg-a",
        },
        Question {
            prompt: "What package communities exist?",
            relation: "communities",
            args: QuestionArgs::None,
            expected_substring: "packages/pkg-a",
        },
        Question {
            prompt: "What design patterns are detected in this bundle?",
            relation: "patterns",
            args: QuestionArgs::None,
            expected_substring: "WidgetBuilder",
        },
        Question {
            prompt: "What REST endpoints, database models, or event-flow participants exist?",
            relation: "features",
            args: QuestionArgs::None,
            expected_substring: "UserController",
        },
    ]
}

/// Whether `chosen` (the relation/tool name a model reported picking)
/// matches `expected` — the scoring policy this benchmark's
/// tool-selection accuracy is computed from. Deliberately a real
/// function, not inlined at every call site: a future live-endpoint
/// runner (or a fuzzier matching policy, if a model's raw response needs
/// normalizing first) has exactly one place to call into, and this is
/// exactly the unit this phase's own test plan asks to be checkable
/// against canned/mocked responses, independent of any real model.
pub fn scores_correctly(chosen: &str, expected: &str) -> bool {
    chosen == expected
}

/// Whether `response` — the real, well-formed output of a *wrong*
/// tool/relation call — is one of `okf-query`'s own "nothing found"
/// sentinels (`"No callers found for..."`, `` "`id` doesn't call
/// anything..." ``, `"No cycles found..."`, and the rest of that family;
/// see `crates/okf-query/src/lib.rs`, every `graph_*` function's own
/// empty-result branch). This is [`crate::tool_selection_live::FailureMode::DetectableWrong`]'s
/// scoring rule: a **conservative, narrow** operationalization of
/// "detectable wrong" (external review's third failure category,
/// `docs/improvement-plan-provenance-diff.md`'s Phase G), chosen
/// deliberately over trying to model whether a live model itself would
/// notice its own mistake — this benchmark only ever scores one tool call
/// per question, never asks a model to reflect on its own answer, so
/// there's no "did it notice" signal to read at all. What *is*
/// measurable without a second model call: whether the wrong tool's
/// response is structurally empty/negative for these arguments — a
/// signal any downstream consumer could act on without knowing the right
/// answer, unlike a populated-but-wrong response (still scored
/// `SilentWrong`), which looks exactly as plausible as a correct one.
/// This deliberately does **not** try to catch the broader "populated,
/// but obviously about a different subject" case (e.g. a `stats`
/// breakdown returned for a "who calls X" question) — that's real
/// information a downstream consumer might also use, but it needs a
/// notion of expected response *shape* per question this benchmark
/// doesn't build, so it stays `SilentWrong` rather than being folded into
/// a heuristic broad enough to risk false positives.
pub fn is_negative_response(response: &str) -> bool {
    response.starts_with("No ") || response.contains("doesn't call anything")
}

/// The shared ground-truth fixture every [`Question`] above is checked
/// against — two packages/modules with a real cross-package call (for
/// `callers`/`callees`/`path`/`explain`/`api`/`modules`/`layers`/
/// `domains`/`communities`), a genuine two-function call cycle (for
/// `cycles`), a concept with no call edges at all (for `isolated`), and
/// one `*Builder`/`*Controller`-shaped pair each (for `patterns`/
/// `features`) — the same hand-authored-bundle-file convention
/// `crate::benchmark`'s and `okf-query`'s own fixture tests already use,
/// reused here rather than reinvented.
///
/// Not `#[cfg(test)]`: [`crate::tool_selection_live`]'s
/// `--benchmark-tool-selection` runner builds this same fixture in the
/// real binary, not just under `cargo test` — the live benchmark measures
/// a model's tool-selection behavior against one fixed, known-correct
/// bundle, not whatever project happens to be passed on the command
/// line (which has no known-correct answers to score against at all).
///
/// Panics on I/O failure — fine for the many test call sites, which treat
/// "can't even build the fixture" as a hard test-setup error the same way
/// `tempfile::tempdir().unwrap()` idioms do throughout this codebase's
/// tests. [`crate::tool_selection_live::run`], the one *production* call
/// site, uses [`fixture_bundle_try`] instead, so a real I/O failure there
/// (an unwritable or full temp directory) surfaces as a clean `anyhow`
/// error through `main()` rather than panicking the whole process.
pub fn fixture_bundle() -> tempfile::TempDir {
    fixture_bundle_try()
        .expect("failed to build the tool-selection benchmark's in-memory fixture bundle")
}

/// Fallible form of [`fixture_bundle`] — see its doc comment for why this
/// exists separately.
pub fn fixture_bundle_try() -> anyhow::Result<tempfile::TempDir> {
    let dir = tempfile::tempdir()?;
    let write = |relative: &str, content: &str| -> anyhow::Result<()> {
        let path = dir.path().join(relative);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(path, content)?;
        Ok(())
    };

    write(
        "packages/pkg-a.md",
        "---\ntype: Rust Package\ntitle: pkg-a\nresource: pkg-a/Cargo.toml\n---\n\nbody\n",
    )?;
    write(
        "packages/pkg-b.md",
        "---\ntype: Rust Package\ntitle: pkg-b\nresource: pkg-b/Cargo.toml\n---\n\nbody\n",
    )?;
    write(
        "modules/pkg-a.md",
        "---\ntype: Rust Module\ntitle: pkg-a\nresource: pkg-a/src/lib.rs#L1\nrelationships:\n  member_of:\n    - packages/pkg-a\n---\n\nbody\n",
    )?;
    write(
        "modules/pkg-b.md",
        "---\ntype: Rust Module\ntitle: pkg-b\nresource: pkg-b/src/lib.rs#L1\nrelationships:\n  member_of:\n    - packages/pkg-b\n---\n\nbody\n",
    )?;

    write(
        "functions/pkg-a/foo.md",
        "---\ntype: Rust Function\ntitle: foo\nresource: pkg-a/src/lib.rs#L3\nrelationships:\n  calls:\n    - functions/pkg-b/bar\n---\n\nbody\n",
    )?;
    write(
        "functions/pkg-b/bar.md",
        "---\ntype: Rust Function\ntitle: bar\nresource: pkg-b/src/lib.rs#L3\nrelationships:\n  called_by:\n    - functions/pkg-a/foo\n---\n\nbody\n",
    )?;

    write(
        "functions/pkg-a/cyc_a.md",
        "---\ntype: Rust Function\ntitle: cyc_a\nresource: pkg-a/src/cyc.rs#L1\nrelationships:\n  calls:\n    - functions/pkg-a/cyc_b\n---\n\nbody\n",
    )?;
    write(
        "functions/pkg-a/cyc_b.md",
        "---\ntype: Rust Function\ntitle: cyc_b\nresource: pkg-a/src/cyc.rs#L5\nrelationships:\n  calls:\n    - functions/pkg-a/cyc_a\n---\n\nbody\n",
    )?;

    write(
        "functions/pkg-a/lonely.md",
        "---\ntype: Rust Function\ntitle: lonely\nresource: pkg-a/src/lonely.rs#L1\n---\n\nbody\n",
    )?;

    write(
        "classes/pkg-a/WidgetBuilder.md",
        "---\ntype: Rust Class\ntitle: WidgetBuilder\nresource: pkg-a/src/widget.rs#L1\n---\n\nbody\n",
    )?;
    write(
        "functions/pkg-a/WidgetBuilder/build.md",
        "---\ntype: Rust Method\ntitle: build\nresource: pkg-a/src/widget.rs#L5\n---\n\nbody\n",
    )?;

    write(
        "classes/pkg-a/UserController.md",
        "---\ntype: Rust Class\ntitle: UserController\nresource: pkg-a/src/controller.rs#L1\n---\n\nbody\n",
    )?;
    write(
        "functions/pkg-a/UserController/get_user.md",
        "---\ntype: Rust Method\ntitle: get_user\nresource: pkg-a/src/controller.rs#L5\n---\n\nbody\n",
    )?;

    Ok(dir)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cache::BundleCache;
    use crate::tools;

    /// This repository's root, derived from `okf-mcp`'s own manifest
    /// directory — the same technique every other crate's golden-fixture
    /// test in this workspace uses to find `tests/fixtures/` regardless
    /// of `cargo test`'s working directory.
    fn repo_root() -> std::path::PathBuf {
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .and_then(std::path::Path::parent)
            .expect("crates/okf-mcp should be two levels below the repo root")
            .to_path_buf()
    }

    /// Phase F's `tests/fixtures/mcp/` (see
    /// `docs/improvement-plan-provenance-diff.md`) exports this module's
    /// question set and specialized-tool-name mapping as portable JSON,
    /// for a non-Rust live-endpoint runner to load directly rather than
    /// re-deriving it. This test is what keeps that export honest: the
    /// exported files are checked against the actual Rust data on every
    /// run, so the two can't silently drift apart.
    #[test]
    fn exported_json_fixtures_match_the_rust_question_set() {
        let questions_json: serde_json::Value = serde_json::from_str(
            &std::fs::read_to_string(
                repo_root().join("tests/fixtures/mcp/consolidated/questions.json"),
            )
            .unwrap(),
        )
        .unwrap();
        let exported_questions = questions_json["questions"].as_array().unwrap();
        let rust_questions = questions();
        assert_eq!(exported_questions.len(), rust_questions.len());

        for (exported, rust) in exported_questions.iter().zip(rust_questions.iter()) {
            assert_eq!(exported["prompt"], rust.prompt);
            assert_eq!(exported["relation"], rust.relation);
            assert_eq!(exported["expected_substring"], rust.expected_substring);
            assert_eq!(exported["arguments"], rust.args.to_json(rust.relation));
        }

        let tool_names_json: serde_json::Value = serde_json::from_str(
            &std::fs::read_to_string(
                repo_root().join("tests/fixtures/mcp/specialized/tool-names.json"),
            )
            .unwrap(),
        )
        .unwrap();
        let exported_tool_names: Vec<String> = tool_names_json["tool_names"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap().to_string())
            .collect();
        let rust_tool_names: Vec<String> = rust_questions
            .iter()
            .filter_map(|q| specialized_tool_name(q.relation))
            .collect();
        assert_eq!(exported_tool_names, rust_tool_names);
    }

    #[test]
    fn every_question_s_relation_is_one_the_graph_tool_actually_exposes() {
        let tools_list = tools::list();
        let graph_tool = tools_list
            .iter()
            .find(|t| t["name"] == "graph")
            .expect("the graph tool should be registered");
        let known_relations: Vec<&str> = graph_tool["inputSchema"]["properties"]["relation"]
            ["enum"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap())
            .collect();

        for q in questions() {
            assert!(
                known_relations.contains(&q.relation),
                "question {:?} references relation `{}`, which the graph tool's own schema doesn't list -- the question set has drifted out of sync",
                q.prompt,
                q.relation,
            );
        }
    }

    #[test]
    fn there_are_fourteen_questions_one_per_relation_no_duplicates() {
        let qs = questions();
        assert_eq!(qs.len(), 14);
        let mut relations: Vec<&str> = qs.iter().map(|q| q.relation).collect();
        relations.sort();
        relations.dedup();
        assert_eq!(
            relations.len(),
            14,
            "every relation should appear exactly once"
        );
    }

    #[test]
    fn exactly_one_question_has_no_specialized_tool_counterpart() {
        let without_counterpart: Vec<&str> = questions()
            .iter()
            .filter(|q| specialized_tool_name(q.relation).is_none())
            .map(|q| q.relation)
            .collect();
        assert_eq!(
            without_counterpart,
            vec!["explain"],
            "explain is the only relation with no pre-0.3.0 graph_* tool to compare against"
        );
    }

    #[test]
    fn specialized_tool_name_follows_the_graph_prefix_convention() {
        assert_eq!(
            specialized_tool_name("callers"),
            Some("graph_callers".to_string())
        );
        assert_eq!(
            specialized_tool_name("patterns"),
            Some("graph_patterns".to_string())
        );
        assert_eq!(specialized_tool_name("explain"), None);
    }

    #[test]
    fn scores_correctly_matches_and_rejects_as_expected() {
        assert!(scores_correctly("callers", "callers"));
        assert!(!scores_correctly("callees", "callers"));
        assert!(!scores_correctly("graph_callers", "callers"));
    }

    #[test]
    fn is_negative_response_recognizes_the_okf_query_empty_result_sentinels() {
        assert!(is_negative_response(
            "No callers found for `functions/pkg-a/foo`"
        ));
        assert!(is_negative_response("No cycles found in the call graph"));
        assert!(is_negative_response("No isolated concepts found"));
        assert!(is_negative_response(
            "`functions/pkg-b/bar` doesn't call anything (or only calls unresolved/ambiguous targets)"
        ));
        assert!(!is_negative_response("functions/pkg-a/foo — Rust Function"));
        assert!(!is_negative_response(
            "3 public concepts:\n  Function     functions/pkg-a/foo"
        ));
    }

    /// The real behavioral distinction [`is_negative_response`] exists to
    /// detect, verified directly against the fixture rather than assumed:
    /// on this bundle, swapping `callers`/`callees` for either of the two
    /// questions built around the `foo -> bar` edge produces a genuine
    /// empty-result sentinel (`bar` has no outgoing calls of its own;
    /// `foo` has no callers of its own) -- confirming there's a real
    /// signal here to classify `DetectableWrong` against, not a heuristic
    /// that would never fire on this benchmark's own fixture.
    #[test]
    fn callers_callees_swap_on_this_fixture_produces_a_real_negative_response() {
        let dir = fixture_bundle();
        let cache = BundleCache::new();

        // "Who calls bar?" (correct: callers) answered with `callees`
        // instead: bar has no outgoing calls at all.
        let wrong_callees_for_bar = tools::call(
            "graph",
            &serde_json::json!({ "relation": "callees", "id": "functions/pkg-b/bar" }),
            dir.path(),
            &cache,
        )
        .unwrap();
        assert!(is_negative_response(&wrong_callees_for_bar));

        // "What does foo call?" (correct: callees) answered with
        // `callers` instead: nothing in the fixture calls foo.
        let wrong_callers_for_foo = tools::call(
            "graph",
            &serde_json::json!({ "relation": "callers", "id": "functions/pkg-a/foo" }),
            dir.path(),
            &cache,
        )
        .unwrap();
        assert!(is_negative_response(&wrong_callers_for_foo));
    }

    /// The claim this whole benchmark's fixed question set rests on:
    /// every question's declared `expected_substring` really is what the
    /// consolidated `graph` tool returns for its declared `relation`/
    /// `args`, checked directly against the fixture with no model
    /// involved -- exactly what the plan's own acceptance criteria asks
    /// for ("a question intended to map to `callers` really does have a
    /// known-correct `graph_callers`-shaped answer").
    #[test]
    fn every_question_s_expected_answer_is_correct_against_the_fixture() {
        let dir = fixture_bundle();
        let cache = BundleCache::new();
        for q in questions() {
            let args = q.args.to_json(q.relation);
            let response = tools::call("graph", &args, dir.path(), &cache).unwrap_or_else(|e| {
                panic!(
                    "question {:?} (relation {}) failed: {e}",
                    q.prompt, q.relation
                )
            });
            assert!(
                response.contains(q.expected_substring),
                "question {:?} (relation {}): expected {:?} in response, got: {response}",
                q.prompt,
                q.relation,
                q.expected_substring,
            );
        }
    }
}
