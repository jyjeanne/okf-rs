use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use okf_core::Project;
use okf_parser::ConceptKind;
use okf_validator::{IssueKind, Severity};
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::process::ExitCode;

#[derive(Parser)]
#[command(
    name = "okf-rs",
    version,
    about = "Generate, validate, and search Open Knowledge Format bundles from source code"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Scan a project and write an `okf.toml` recording defaults for later commands.
    Init {
        #[arg(default_value = ".")]
        path: PathBuf,
        /// Bundle output directory to record as the project default.
        #[arg(short, long, default_value = "knowledge")]
        output: PathBuf,
        /// Skip writing/updating CLAUDE.md, AGENTS.md, and
        /// .github/copilot-instructions.md.
        #[arg(long)]
        no_agent_files: bool,
    },
    /// Recursively scan a repository and report what would be analyzed.
    Scan {
        #[arg(default_value = ".")]
        path: PathBuf,
    },
    /// Analyze a repository and write an OKF bundle.
    Generate {
        #[arg(default_value = ".")]
        path: PathBuf,
        /// Bundle output directory. Defaults to the value in `okf.toml`, or `knowledge`.
        #[arg(short, long)]
        output: Option<PathBuf>,
        /// Ignore and don't update the `.okf-cache.json` incremental-index
        /// cache — every file is re-parsed from scratch. Useful to rule
        /// out a stale/corrupt cache, or to verify output determinism
        /// independent of cache state.
        #[arg(long)]
        no_cache: bool,
        /// Resolve calls whose callee name is ambiguous project-wide by
        /// asking each call site's real language server
        /// (`textDocument/definition`), on top of Tree-sitter's own
        /// unambiguous-name-only resolution. Optional and best-effort: a
        /// language with no available server is simply skipped. Spawns
        /// real language server processes, so this is meaningfully slower
        /// than a plain `generate`.
        #[arg(long)]
        lsp: bool,
        /// Fill in missing function/method/module/package descriptions
        /// via an OpenAI-compatible chat completions endpoint (Ollama, LM
        /// Studio, LocalAI, Crustly, or a cloud provider). A concept that
        /// already has a description — including one from a previous
        /// `--enrich` run, carried forward from the existing bundle at
        /// `--output` — is never re-queried or overwritten. Requires
        /// `--enrich-base-url`/`--enrich-model` (or the
        /// `OKF_ENRICH_BASE_URL`/`OKF_ENRICH_MODEL` environment variables).
        #[arg(long)]
        enrich: bool,
        /// Enrichment endpoint base URL, e.g. `http://localhost:11434/v1`
        /// for Ollama. Falls back to `OKF_ENRICH_BASE_URL` if unset.
        #[arg(long)]
        enrich_base_url: Option<String>,
        /// Enrichment model name, e.g. `llama3.1`. Falls back to
        /// `OKF_ENRICH_MODEL` if unset.
        #[arg(long)]
        enrich_model: Option<String>,
        /// Enrichment endpoint API key, sent as `Authorization: Bearer
        /// <key>`. Omit for a local server that doesn't need one. Falls
        /// back to `OKF_ENRICH_API_KEY` if unset.
        #[arg(long)]
        enrich_api_key: Option<String>,
        /// Also import an existing DITA XML corpus (a directory of
        /// `.dita`/`.xml` topic files, or a single file) as `Document`
        /// concepts, merged into the same bundle alongside the concepts
        /// extracted from source code. A topic that fails to parse is
        /// skipped with a warning rather than failing the whole command.
        #[arg(long)]
        dita: Option<PathBuf>,
        /// Verify determinism instead of writing a bundle: runs analysis
        /// twice, independently (always bypassing the incremental cache
        /// both times, regardless of `--no-cache`), renders each to a
        /// throwaway directory, and diffs them byte-for-byte. Exits
        /// non-zero and lists every differing file if they're not
        /// identical — the tree-sitter-only path is expected to always
        /// pass; combined with `--lsp`, this is the tool for the
        /// local-vs-cold-CI-runner divergence documented in
        /// `ROADMAP.md`'s Phase 2 known limitations. Doesn't touch
        /// `--output` or the real `.okf-cache.json`. Not combinable with
        /// `--enrich`, since a live endpoint's response isn't guaranteed
        /// byte-identical across calls and would report unrelated
        /// non-determinism.
        #[arg(long)]
        check_determinism: bool,
        /// Verify the bundle at `--output` is up to date with source
        /// instead of writing to it: re-analyzes the project fresh (always
        /// bypassing the incremental cache, regardless of `--no-cache`),
        /// renders it to a throwaway directory, and diffs that against the
        /// bundle already on disk. Exits non-zero and lists every stale
        /// file if they differ — the gap named in `ROADMAP.md`'s "CI
        /// validation mode" item: `okf-rs validate` only checks the
        /// bundle's own internal consistency, never whether it still
        /// matches the source it was supposedly generated from. Fails
        /// clearly if `--output` doesn't exist yet, rather than reporting
        /// every file as "stale". Not combinable with `--enrich`, for the
        /// same reason as `--check-determinism`: comparing against a fresh
        /// (unenriched) analysis would flag every AI-written description
        /// as spurious drift.
        #[arg(long)]
        check_fresh: bool,
    },
    /// Watch a project and keep its OKF bundle up to date as files change.
    /// Runs until interrupted (Ctrl+C). Regenerates once immediately, then
    /// again after each burst of filesystem activity settles, reusing the
    /// same `.okf-cache.json` incremental-index cache `generate` does.
    Watch {
        #[arg(default_value = ".")]
        path: PathBuf,
        /// Bundle output directory. Defaults to the value in `okf.toml`, or `knowledge`.
        #[arg(short, long)]
        output: Option<PathBuf>,
        /// How long a quiet period must last, in milliseconds, before a
        /// burst of filesystem events triggers a regenerate.
        #[arg(long, default_value_t = 300)]
        debounce_ms: u64,
    },
    /// Validate that a directory is a conformant OKF bundle.
    Validate {
        /// Defaults to the value in `okf.toml`, or `knowledge`.
        bundle: Option<PathBuf>,
        /// Project directory to look up `okf.toml` in (not the bundle itself).
        #[arg(short, long, default_value = ".")]
        project: PathBuf,
        /// Treat orphaned-concept warnings as failures too (for CI gating).
        #[arg(long)]
        ci: bool,
    },
    /// Search an OKF bundle by symbol, type, or tag.
    Search {
        query: String,
        /// Defaults to the value in `okf.toml`, or `knowledge`.
        bundle: Option<PathBuf>,
        /// Project directory to look up `okf.toml` in (not the bundle itself).
        #[arg(short, long, default_value = ".")]
        project: PathBuf,
        /// Use ranked, relevance-scored full-text search (via Tantivy)
        /// instead of exact/substring matching. Also searches
        /// description/signature text, not just title/type/tags — better
        /// for a natural-language query than an exact symbol name.
        #[arg(long)]
        ranked: bool,
        /// Rank by embedding-cosine similarity ("find by meaning")
        /// instead of exact/substring or lexical-relevance matching, via
        /// an OpenAI-compatible `/embeddings` endpoint. Only concepts
        /// with a description are considered (run `generate --enrich`
        /// first if the bundle has none). Requires
        /// `--enrich-base-url`/`--enrich-model` (or the
        /// `OKF_ENRICH_BASE_URL`/`OKF_ENRICH_MODEL` environment
        /// variables) pointed at an embeddings-capable model. Mutually
        /// exclusive with `--ranked`.
        #[arg(long, conflicts_with = "ranked")]
        semantic: bool,
        /// Embedding endpoint base URL, e.g. `http://localhost:11434/v1`
        /// for Ollama. Falls back to `OKF_ENRICH_BASE_URL` if unset.
        /// Only used with `--semantic`.
        #[arg(long)]
        enrich_base_url: Option<String>,
        /// Embedding model name, e.g. `nomic-embed-text`. Falls back to
        /// `OKF_ENRICH_MODEL` if unset. Only used with `--semantic`.
        #[arg(long)]
        enrich_model: Option<String>,
        /// Embedding endpoint API key. Falls back to `OKF_ENRICH_API_KEY`
        /// if unset. Only used with `--semantic`.
        #[arg(long)]
        enrich_api_key: Option<String>,
    },
    /// One-call context for a concept: signature, description, direct
    /// callers, direct callees, blast radius (every concept transitively
    /// affected if this one changes), public-API membership, and
    /// call-cycle membership — everything `search`/`graph callers`/`graph
    /// callees` would otherwise take several separate calls to assemble,
    /// in one command.
    Explore {
        /// A concept id, or free text to resolve via ranked search (the
        /// top hit is used).
        query: String,
        /// Defaults to the value in `okf.toml`, or `knowledge`.
        bundle: Option<PathBuf>,
        /// Project directory to look up `okf.toml` in (not the bundle itself).
        #[arg(short, long, default_value = ".")]
        project: PathBuf,
    },
    /// Report content-completeness metrics for a bundle: description/tag
    /// coverage, and how much of the bundle participates in the call
    /// graph. Distinct from `validate`, which is pass/fail rather than a
    /// metrics report.
    Coverage {
        /// Defaults to the value in `okf.toml`, or `knowledge`.
        bundle: Option<PathBuf>,
        /// Project directory to look up `okf.toml` in (not the bundle itself).
        #[arg(short, long, default_value = ".")]
        project: PathBuf,
    },
    /// AI-suggested missing relationship links between semantically close
    /// concepts that aren't already connected — advisory only, nothing is
    /// written back into the bundle. Builds on the same OpenAI-compatible
    /// endpoint `generate --enrich` uses: candidates are generated for
    /// free via full-text search (`search --ranked`'s own index), and
    /// only those candidates are sent to the endpoint for a yes/no
    /// judgment, so this is bounded by the bundle's described-concept
    /// count, not every possible pair.
    SuggestLinks {
        /// Defaults to the value in `okf.toml`, or `knowledge`.
        bundle: Option<PathBuf>,
        /// Project directory to look up `okf.toml` in (not the bundle itself).
        #[arg(short, long, default_value = ".")]
        project: PathBuf,
        /// Enrichment endpoint base URL, e.g. `http://localhost:11434/v1`
        /// for Ollama. Falls back to `OKF_ENRICH_BASE_URL` if unset.
        #[arg(long)]
        enrich_base_url: Option<String>,
        /// Enrichment model name, e.g. `llama3.1`. Falls back to
        /// `OKF_ENRICH_MODEL` if unset.
        #[arg(long)]
        enrich_model: Option<String>,
        /// Enrichment endpoint API key, sent as `Authorization: Bearer
        /// <key>`. Falls back to `OKF_ENRICH_API_KEY` if unset.
        #[arg(long)]
        enrich_api_key: Option<String>,
        /// Maximum full-text-search candidates considered per described
        /// concept (and therefore the upper bound on endpoint calls per
        /// concept). Kept small by default since this is O(described
        /// concepts × candidates) network calls, not a one-off.
        #[arg(long, default_value_t = 3)]
        candidates: usize,
    },
    /// Query the concept graph: callers, callees, cycles, public API, and
    /// cross-module dependencies. Reads relationships directly from a
    /// previously generated OKF bundle on disk — run `okf-rs generate`
    /// first (and re-run it after source changes, to keep the bundle's
    /// relationships current).
    Graph {
        #[command(subcommand)]
        query: GraphQuery,
    },
    /// Compare the OKF concepts between two git refs (added/removed/changed).
    Diff {
        from_ref: String,
        to_ref: String,
        /// Project directory to diff, relative to the git repository root.
        #[arg(default_value = ".")]
        path: PathBuf,
        /// Provenance-aware CI mode: classifies changes as source, resolver,
        /// or confidence, and exits non-zero on any source-level change
        /// (always) or resolver/confidence-level change (per `okf.toml`'s
        /// `[diff]` policy — resolver changes warn by default, confidence
        /// changes are ignored by default; either can be set to "fail" or
        /// "ignore"/"warn" respectively).
        #[arg(long)]
        ci: bool,
    },
    /// Compares two already-generated OKF bundle directories directly,
    /// with the same provenance-aware classification `diff --ci` reports
    /// (source/resolver/confidence changes, plus the resolver-only rate).
    /// Unlike `diff`, this never touches git: `diff`'s two-git-ref
    /// comparison has nothing to check out when the two bundles being
    /// compared came from the *same* source snapshot re-analyzed under
    /// different conditions — most concretely, two `generate --lsp` runs
    /// against two different installed resolver versions, the exact
    /// measurement `docs/improvement-plan-provenance-diff.md`'s Phase G
    /// asks a project to run on its own corpus to decide whether
    /// `DiffPolicy::resolver_changes` can safely default to `"ignore"`.
    /// Not limited to that one use case, though: any two independently
    /// generated bundles of conceptually the same project (e.g. `--lsp`
    /// on vs. off) can be compared the same way.
    DiffBundles {
        /// The "before" bundle directory (e.g. generated with the older
        /// resolver version).
        bundle_a: PathBuf,
        /// The "after" bundle directory (e.g. generated with the newer
        /// resolver version).
        bundle_b: PathBuf,
        /// Project directory to load `okf.toml`'s `[diff]` policy from
        /// (the same policy `diff --ci` reads) — not where either bundle
        /// lives, since a bundle directory has no `okf.toml` of its own.
        #[arg(long, default_value = ".")]
        project: PathBuf,
    },
    /// Renders the same change-impact analysis as `impact`, but as a
    /// Markdown report meant to be posted as a pull-request comment (a
    /// leading HTML marker comment lets a bot find and update its own
    /// prior comment instead of piling up a new one on every push).
    Review {
        from_ref: String,
        to_ref: String,
        /// Project directory to analyze, relative to the git repository root.
        #[arg(default_value = ".")]
        path: PathBuf,
        /// Exit non-zero (in addition to printing the report) if any
        /// changed concept's blast radius has at least this many
        /// concepts — for CI merge gating on risky changes. Unset (the
        /// default) never fails regardless of blast radius size; the
        /// report itself is identical either way.
        #[arg(long)]
        fail_on_risk: Option<usize>,
    },
    /// Change-impact ("blast radius") analysis between two git refs: for
    /// every added/removed/changed concept, which other concepts
    /// transitively depend on it, whether it's public API, and whether
    /// it sits in a call-graph cycle — a deterministic, structural risk
    /// signal for what a change actually puts at risk downstream, beyond
    /// what `diff`'s concept-level added/removed/changed list shows on
    /// its own.
    Impact {
        from_ref: String,
        to_ref: String,
        /// Project directory to analyze, relative to the git repository root.
        #[arg(default_value = ".")]
        path: PathBuf,
    },
    /// Generate human-readable documentation from a previously generated OKF
    /// bundle: a browsable static HTML site, a single consolidated Markdown
    /// document, a single paginated PDF, a DITA topic set, a GraphML graph
    /// (for Gephi/yEd/any other GraphML-reading tool), or an Obsidian vault.
    Docs {
        /// Defaults to the value in `okf.toml`, or `knowledge`.
        bundle: Option<PathBuf>,
        /// Project directory to look up `okf.toml` in (not the bundle itself).
        #[arg(short, long, default_value = ".")]
        project: PathBuf,
        /// Output path: a directory for `--format html`/`--format
        /// dita`/`--format obsidian`, a file for `--format
        /// markdown`/`--format pdf`/`--format graphml`. Defaults to
        /// `docs/`, `docs.md`, `docs.pdf`, `docs-dita/`, `docs.graphml`,
        /// or `docs-obsidian/` respectively.
        #[arg(short, long)]
        output: Option<PathBuf>,
        #[arg(short, long, value_enum, default_value_t = DocsFormat::Html)]
        format: DocsFormat,
    },
}

#[derive(Clone, Copy, clap::ValueEnum)]
enum DocsFormat {
    Html,
    Markdown,
    Pdf,
    Dita,
    Graphml,
    Obsidian,
}

#[derive(Subcommand)]
enum GraphQuery {
    /// List concepts that directly call the given concept id.
    Callers {
        id: String,
        /// Defaults to the value in `okf.toml`, or `knowledge`.
        bundle: Option<PathBuf>,
        /// Project directory to look up `okf.toml` in (not the bundle itself).
        #[arg(short, long, default_value = ".")]
        project: PathBuf,
    },
    /// List concepts the given concept id directly calls.
    Callees {
        id: String,
        /// Defaults to the value in `okf.toml`, or `knowledge`.
        bundle: Option<PathBuf>,
        /// Project directory to look up `okf.toml` in (not the bundle itself).
        #[arg(short, long, default_value = ".")]
        project: PathBuf,
    },
    /// List groups of concepts that call each other in a cycle.
    Cycles {
        /// Defaults to the value in `okf.toml`, or `knowledge`.
        bundle: Option<PathBuf>,
        /// Project directory to look up `okf.toml` in (not the bundle itself).
        #[arg(short, long, default_value = ".")]
        project: PathBuf,
    },
    /// List concepts with no `Calls`/`CalledBy` edge in either direction
    /// (never observed calling anything, and never observed being called).
    Isolated {
        /// Defaults to the value in `okf.toml`, or `knowledge`.
        bundle: Option<PathBuf>,
        /// Project directory to look up `okf.toml` in (not the bundle itself).
        #[arg(short, long, default_value = ".")]
        project: PathBuf,
    },
    /// List the public API surface (public functions/methods/types).
    Api {
        /// Defaults to the value in `okf.toml`, or `knowledge`.
        bundle: Option<PathBuf>,
        /// Project directory to look up `okf.toml` in (not the bundle itself).
        #[arg(short, long, default_value = ".")]
        project: PathBuf,
    },
    /// List cross-module dependency edges (which modules call into which).
    Modules {
        /// Defaults to the value in `okf.toml`, or `knowledge`.
        bundle: Option<PathBuf>,
        /// Project directory to look up `okf.toml` in (not the bundle itself).
        #[arg(short, long, default_value = ".")]
        project: PathBuf,
    },
    /// Report graph topology metrics: concept-kind breakdown, relationship
    /// edge counts by kind, and connected components of the
    /// `Calls`/`CalledBy` graph.
    Stats {
        /// Defaults to the value in `okf.toml`, or `knowledge`.
        bundle: Option<PathBuf>,
        /// Project directory to look up `okf.toml` in (not the bundle itself).
        #[arg(short, long, default_value = ".")]
        project: PathBuf,
    },
    /// Find the shortest call path between two concept ids.
    Path {
        from: String,
        to: String,
        /// Defaults to the value in `okf.toml`, or `knowledge`.
        bundle: Option<PathBuf>,
        /// Project directory to look up `okf.toml` in (not the bundle itself).
        #[arg(short, long, default_value = ".")]
        project: PathBuf,
    },
    /// Explain *why* a relationship exists between two concepts: which
    /// kind it is, and a human-readable reason derived from its
    /// provenance/confidence (e.g. "resolved via Tree-sitter's
    /// unambiguous name match" or "resolved by asking rust-analyzer
    /// which definition this call site resolves to"). Falls back to
    /// explaining the shortest call path, hop by hop, when there's no
    /// single direct relationship.
    Explain {
        from: String,
        to: String,
        /// Defaults to the value in `okf.toml`, or `knowledge`.
        bundle: Option<PathBuf>,
        /// Project directory to look up `okf.toml` in (not the bundle itself).
        #[arg(short, long, default_value = ".")]
        project: PathBuf,
    },
    /// Report each package's layer in the layered architecture derived
    /// from the package dependency graph (layer 0 = depends on nothing
    /// else in the bundle).
    Layers {
        /// Defaults to the value in `okf.toml`, or `knowledge`.
        bundle: Option<PathBuf>,
        /// Project directory to look up `okf.toml` in (not the bundle itself).
        #[arg(short, long, default_value = ".")]
        project: PathBuf,
    },
    /// List domain boundaries: clusters of packages that depend on each
    /// other, directly or transitively.
    Domains {
        /// Defaults to the value in `okf.toml`, or `knowledge`.
        bundle: Option<PathBuf>,
        /// Project directory to look up `okf.toml` in (not the bundle itself).
        #[arg(short, long, default_value = ".")]
        project: PathBuf,
    },
    /// List package communities found by modularity-optimization
    /// community detection — a finer-grained signal than `domains`'
    /// plain connected components: two packages technically reachable
    /// from each other can still land in different communities if their
    /// connection is weak relative to how strongly each already
    /// collaborates within its own cluster.
    Communities {
        /// Defaults to the value in `okf.toml`, or `knowledge`.
        bundle: Option<PathBuf>,
        /// Project directory to look up `okf.toml` in (not the bundle itself).
        #[arg(short, long, default_value = ".")]
        project: PathBuf,
    },
    /// List design patterns detected via structural/naming heuristics
    /// (Builder, Singleton, Factory, Visitor).
    Patterns {
        /// Defaults to the value in `okf.toml`, or `knowledge`.
        bundle: Option<PathBuf>,
        /// Project directory to look up `okf.toml` in (not the bundle itself).
        #[arg(short, long, default_value = ".")]
        project: PathBuf,
    },
    /// List REST endpoints, database models, and event-flow participants
    /// detected via structural/naming heuristics (e.g. a public method on
    /// a `*Controller`-named type, a `*Model`/`*Entity`-named type, an
    /// `emit_*`/`on_*`-named function).
    Features {
        /// Defaults to the value in `okf.toml`, or `knowledge`.
        bundle: Option<PathBuf>,
        /// Project directory to look up `okf.toml` in (not the bundle itself).
        #[arg(short, long, default_value = ".")]
        project: PathBuf,
    },
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    match run(cli.command) {
        Ok(code) => code,
        Err(err) => {
            eprintln!("error: {err:#}");
            ExitCode::FAILURE
        }
    }
}

fn run(command: Command) -> Result<ExitCode> {
    match command {
        Command::Init {
            path,
            output,
            no_agent_files,
        } => cmd_init(&path, &output, no_agent_files),
        Command::Scan { path } => cmd_scan(&path),
        Command::Generate {
            path,
            output,
            no_cache,
            lsp,
            enrich,
            enrich_base_url,
            enrich_model,
            enrich_api_key,
            dita,
            check_determinism,
            check_fresh,
        } => {
            if check_determinism && check_fresh {
                anyhow::bail!(
                    "--check-determinism and --check-fresh check different things (reproducibility vs. staleness) and can't be combined in one run — run them separately"
                );
            }
            if check_determinism {
                if enrich {
                    anyhow::bail!(
                        "--check-determinism can't be combined with --enrich: enrichment depends on a live endpoint's response, which isn't guaranteed byte-identical across calls, and would report non-determinism unrelated to what this flag actually checks"
                    );
                }
                cmd_check_determinism(&path, lsp, dita.as_deref())
            } else if check_fresh {
                if enrich {
                    anyhow::bail!(
                        "--check-fresh can't be combined with --enrich: comparing against a freshly analyzed (unenriched) bundle would flag every AI-written description as spurious drift"
                    );
                }
                cmd_check_fresh(&path, output, lsp, dita.as_deref())
            } else {
                cmd_generate(
                    &path,
                    output,
                    no_cache,
                    lsp,
                    EnrichArgs {
                        enrich,
                        base_url: enrich_base_url,
                        model: enrich_model,
                        api_key: enrich_api_key,
                    },
                    dita,
                )
            }
        }
        Command::Watch {
            path,
            output,
            debounce_ms,
        } => cmd_watch(&path, output, debounce_ms),
        Command::Validate {
            bundle,
            project,
            ci,
        } => cmd_validate(bundle, &project, ci),
        Command::Search {
            query,
            bundle,
            project,
            ranked,
            semantic,
            enrich_base_url,
            enrich_model,
            enrich_api_key,
        } => cmd_search(
            &query,
            bundle,
            &project,
            ranked,
            SemanticSearchArgs {
                enabled: semantic,
                base_url: enrich_base_url,
                model: enrich_model,
                api_key: enrich_api_key,
            },
        ),
        Command::Explore {
            query,
            bundle,
            project,
        } => {
            let bundle = resolve_query_bundle(bundle, &project);
            print_query_result(okf_query::explore(&bundle, &query))
        }
        Command::Coverage { bundle, project } => {
            let bundle = resolve_query_bundle(bundle, &project);
            print_query_result(okf_query::coverage(&bundle))
        }
        Command::SuggestLinks {
            bundle,
            project,
            enrich_base_url,
            enrich_model,
            enrich_api_key,
            candidates,
        } => cmd_suggest_links(
            bundle,
            &project,
            enrich_base_url,
            enrich_model,
            enrich_api_key,
            candidates,
        ),
        Command::Graph { query } => cmd_graph(query),
        Command::Diff {
            from_ref,
            to_ref,
            path,
            ci,
        } => cmd_diff(&from_ref, &to_ref, &path, ci),
        Command::DiffBundles {
            bundle_a,
            bundle_b,
            project,
        } => cmd_diff_bundles(&bundle_a, &bundle_b, &project),
        Command::Impact {
            from_ref,
            to_ref,
            path,
        } => cmd_impact(&from_ref, &to_ref, &path),
        Command::Review {
            from_ref,
            to_ref,
            path,
            fail_on_risk,
        } => cmd_review(&from_ref, &to_ref, &path, fail_on_risk),
        Command::Docs {
            bundle,
            project,
            output,
            format,
        } => cmd_docs(bundle, &project, output, format),
    }
}

/// Resolves the bundle path for a command: an explicit path always wins
/// (used as-is, relative to the caller's current directory, standard CLI
/// convention); otherwise falls back to `okf.toml`'s `output`, which is
/// relative to `project_root` — not the current directory — since that's
/// what it was recorded against, so it's joined here rather than left to
/// resolve against whatever directory the command happens to run from.
fn resolve_bundle_arg(project_root: &std::path::Path, explicit: Option<PathBuf>) -> PathBuf {
    okf_core::config::resolve_bundle(project_root, explicit)
}

fn cmd_init(
    path: &std::path::Path,
    output: &std::path::Path,
    no_agent_files: bool,
) -> Result<ExitCode> {
    let project = Project::load(path)?;
    let config_path = okf_core::config::write_default(&project.root, output)?;
    println!(
        "Initialized okf-rs project at {} ({} source files detected)",
        project.root.display(),
        project.files.len()
    );
    println!("Wrote {}", config_path.display());

    if !no_agent_files {
        let output_display = output.display().to_string();
        let agent_files = okf_generator::write_agent_entrypoints(&project.root, &output_display)?;
        for file in agent_files {
            println!("Wrote {}", file.display());
        }
    }

    Ok(ExitCode::SUCCESS)
}

fn cmd_scan(path: &std::path::Path) -> Result<ExitCode> {
    let project = Project::load(path)?;
    println!("Project root: {}", project.root.display());
    match project.packages.as_slice() {
        [] => println!("Manifest: none detected"),
        // A single manifest at the project root: the common single-package
        // case, reported the same way it always was.
        [pkg] if pkg.relative_dir.is_empty() => println!("Manifest: {:?}", pkg.manifest),
        packages => {
            println!("{} packages detected:", packages.len());
            for pkg in packages {
                let dir = if pkg.relative_dir.is_empty() {
                    "."
                } else {
                    &pkg.relative_dir
                };
                println!("  {:<30} {:?}", dir, pkg.manifest);
            }
        }
    }

    let mut by_language: BTreeMap<String, usize> = BTreeMap::new();
    for file in &project.files {
        *by_language.entry(file.language.to_string()).or_default() += 1;
    }
    println!("{} source files:", project.files.len());
    for (language, count) in by_language {
        println!("  {language:<12} {count}");
    }
    Ok(ExitCode::SUCCESS)
}

/// Where `generate` persists its incremental-indexing cache: a hidden
/// file at the project root, sibling to `okf.toml`, so it survives
/// between invocations regardless of `--output`. Not part of the OKF
/// bundle itself (it's not `.md`, and lives outside `--output` entirely)
/// — a purely local, disposable performance cache, safe to delete or
/// `.gitignore` like `target/`.
const CACHE_FILE: &str = ".okf-cache.json";

/// The `--enrich*` flags bundled together, so `cmd_generate` takes one
/// parameter for them instead of four.
struct EnrichArgs {
    enrich: bool,
    base_url: Option<String>,
    model: Option<String>,
    api_key: Option<String>,
}

fn cmd_generate(
    path: &std::path::Path,
    output: Option<PathBuf>,
    no_cache: bool,
    lsp: bool,
    enrich: EnrichArgs,
    dita: Option<PathBuf>,
) -> Result<ExitCode> {
    let project = Project::load(path)?;
    let output = resolve_bundle_arg(&project.root, output);
    let cache_path = project.root.join(CACHE_FILE);

    let mut cache = if no_cache {
        okf_analyzer::AnalysisCache::default()
    } else {
        okf_analyzer::AnalysisCache::load(&cache_path)
    };
    let (mut result, stats) = okf_analyzer::analyze_with_cache_lsp(&project, &mut cache, lsp)?;

    let dita_imported = if let Some(dita_path) = &dita {
        let (concepts, skipped) = okf_dita::import_dita(dita_path)
            .with_context(|| format!("failed to import DITA corpus at {}", dita_path.display()))?;
        for skip in &skipped {
            eprintln!("warning: skipped DITA topic {skip}");
        }
        let imported = concepts.len();
        result.concepts.extend(concepts);
        Some(imported)
    } else {
        None
    };

    // A config or endpoint failure here is captured rather than
    // propagated immediately: the analysis (and any DITA import) above
    // already did real work, and `?`-ing out of this block would discard
    // all of it — the bundle/cache below get written from whatever
    // `result.concepts` ended up with (unenriched, if enrichment never
    // got to run; partially enriched, if it failed partway through)
    // either way, and the enrichment failure itself is reported after,
    // as a non-zero exit rather than silently swallowed.
    let mut enrich_error: Option<anyhow::Error> = None;
    let enrich_stats = if enrich.enrich {
        match resolve_enrich_config(enrich.base_url, enrich.model, enrich.api_key) {
            Ok(config) => {
                let client = okf_enrich::EnrichClient::new(config);
                // The bundle previously written to `output` (if any), read
                // before `write_bundle` below overwrites it — a concept
                // whose id already carries a description there (human-
                // written, or from an earlier `--enrich` run) is reused
                // instead of re-queried, since a fresh `analyze` never
                // carries descriptions forward on its own.
                let previous = if output.is_dir() {
                    okf_parser::read_bundle(&output)?
                } else {
                    Vec::new()
                };
                match okf_enrich::enrich_missing_descriptions(
                    &client,
                    &mut result.concepts,
                    &previous,
                ) {
                    Ok(stats) => Some(stats),
                    Err(e) => {
                        enrich_error = Some(e);
                        None
                    }
                }
            }
            Err(e) => {
                enrich_error = Some(e);
                None
            }
        }
    } else {
        None
    };

    let source_revision = okf_core::git::head_revision(&project.root);
    okf_generator::write_bundle(&result.concepts, &output, source_revision.as_deref())?;
    if !no_cache {
        cache.save(&cache_path)?;
    }

    let mut by_kind: BTreeMap<ConceptKind, usize> = BTreeMap::new();
    for concept in &result.concepts {
        *by_kind.entry(concept.kind).or_default() += 1;
    }
    println!(
        "Generated {} concepts into {} ({} files parsed, {} reused from cache)",
        result.concepts.len(),
        output.display(),
        stats.reparsed,
        stats.reused
    );
    if let Some(enrich_stats) = enrich_stats {
        println!(
            "Enriched {} concepts ({} generated, {} reused from the previous bundle)",
            enrich_stats.generated + enrich_stats.reused,
            enrich_stats.generated,
            enrich_stats.reused
        );
    }
    if let Some(imported) = dita_imported {
        println!("Imported {imported} DITA topics as Document concepts");
    }
    for (kind, count) in by_kind {
        println!("  {:<12} {count}", kind.as_str());
    }

    if let Some(e) = enrich_error {
        eprintln!("error: {e:#}");
        return Ok(ExitCode::FAILURE);
    }
    Ok(ExitCode::SUCCESS)
}

/// `okf-rs generate --check-determinism`: runs the analysis pipeline
/// twice, independently, and diffs the two renders byte-for-byte instead
/// of writing a bundle. Neither run touches `--output` or the real
/// `.okf-cache.json` — each gets its own fresh, throwaway cache and its
/// own throwaway render directory, both cleaned up on return.
///
/// This exists specifically because determinism means different things
/// on the two paths `generate` can take: the default tree-sitter-only
/// path has no input but the source text itself, so two runs are
/// expected to always agree. `--lsp` resolution asks a real language
/// server, whose answer can depend on that server's own index state in
/// whatever environment it's running in — see `ROADMAP.md`'s Phase 2
/// known limitations for the concrete local-vs-cold-CI-runner scenario
/// this flag exists to let someone actually check for, rather than
/// discover the hard way in CI.
fn cmd_check_determinism(
    path: &std::path::Path,
    lsp: bool,
    dita: Option<&std::path::Path>,
) -> Result<ExitCode> {
    let project = Project::load(path)?;

    let mut cache1 = okf_analyzer::AnalysisCache::default();
    let (mut result1, _) = okf_analyzer::analyze_with_cache_lsp(&project, &mut cache1, lsp)?;
    let mut cache2 = okf_analyzer::AnalysisCache::default();
    let (mut result2, _) = okf_analyzer::analyze_with_cache_lsp(&project, &mut cache2, lsp)?;

    if let Some(dita_path) = dita {
        let (concepts1, _) = okf_dita::import_dita(dita_path)
            .with_context(|| format!("failed to import DITA corpus at {}", dita_path.display()))?;
        let (concepts2, _) = okf_dita::import_dita(dita_path)
            .with_context(|| format!("failed to import DITA corpus at {}", dita_path.display()))?;
        result1.concepts.extend(concepts1);
        result2.concepts.extend(concepts2);
    }

    let run1 = ScratchDir::new("determinism-run1");
    let run2 = ScratchDir::new("determinism-run2");
    // No source_revision: this is a throwaway comparison between two
    // in-process analyses, not a real generate -- see write_bundle's docs.
    okf_generator::write_bundle(&result1.concepts, run1.path(), None)?;
    okf_generator::write_bundle(&result2.concepts, run2.path(), None)?;

    let diffs = diff_dirs(run1.path(), run2.path(), "run 1", "run 2")?;
    if diffs.is_empty() {
        println!(
            "Deterministic: two independent `generate{}` runs on {} produced byte-identical output ({} concepts).",
            if lsp { " --lsp" } else { "" },
            path.display(),
            result1.concepts.len(),
        );
        Ok(ExitCode::SUCCESS)
    } else {
        println!(
            "Non-deterministic: {} file(s) differed between two independent `generate{}` runs on {}{}:",
            diffs.len(),
            if lsp { " --lsp" } else { "" },
            path.display(),
            if lsp {
                " (expected culprit: language-server index state, not source text — see ROADMAP.md's Phase 2 known limitations)"
            } else {
                " (unexpected on the tree-sitter-only path — please report this as a bug)"
            },
        );
        for d in &diffs {
            println!("  {d}");
        }
        Ok(ExitCode::FAILURE)
    }
}

/// `okf-rs generate --check-fresh`: verifies the bundle already on disk
/// at `--output` still matches what analyzing the project fresh would
/// produce, instead of overwriting it — the "bundle is up to date with
/// source" check named in `ROADMAP.md`'s "CI validation mode" item.
/// `okf-rs validate` only ever checks a bundle's own internal
/// consistency (schema-valid, links resolve, no orphans); nothing short
/// of a real re-analysis can tell whether a *committed* bundle still
/// reflects the *current* source tree, since the two aren't linked by
/// anything the bundle itself records. Read-only: never touches
/// `--output` or the real `.okf-cache.json`, the same posture
/// `--check-determinism` already has.
fn cmd_check_fresh(
    path: &std::path::Path,
    output: Option<PathBuf>,
    lsp: bool,
    dita: Option<&std::path::Path>,
) -> Result<ExitCode> {
    let project = Project::load(path)?;
    let existing = resolve_bundle_arg(&project.root, output);
    if !existing.is_dir() {
        anyhow::bail!(
            "no bundle at {} to check for staleness — run `okf-rs generate` first",
            existing.display()
        );
    }

    let mut cache = okf_analyzer::AnalysisCache::default();
    let (mut result, _) = okf_analyzer::analyze_with_cache_lsp(&project, &mut cache, lsp)?;

    if let Some(dita_path) = dita {
        let (concepts, _) = okf_dita::import_dita(dita_path)
            .with_context(|| format!("failed to import DITA corpus at {}", dita_path.display()))?;
        result.concepts.extend(concepts);
    }

    let fresh = ScratchDir::new("check-fresh");
    // No source_revision here either: `diff_dirs` already normalizes it
    // out of the root index.md comparison below (see its own docs for
    // why), so what's passed on this side is moot -- `None` avoids an
    // unnecessary git shell-out for a throwaway write.
    okf_generator::write_bundle(&result.concepts, fresh.path(), None)?;

    let diffs = diff_dirs(
        &existing,
        fresh.path(),
        "the committed bundle",
        "a fresh regenerate",
    )?;
    if diffs.is_empty() {
        println!(
            "Up to date: the bundle at {} matches a fresh `generate{}` on {} ({} concepts).",
            existing.display(),
            if lsp { " --lsp" } else { "" },
            path.display(),
            result.concepts.len(),
        );
        Ok(ExitCode::SUCCESS)
    } else {
        println!(
            "Stale: {} file(s) in {} don't match a fresh `generate{}` on {} — run `okf-rs generate` and commit the result:",
            diffs.len(),
            existing.display(),
            if lsp { " --lsp" } else { "" },
            path.display(),
        );
        for d in &diffs {
            println!("  {d}");
        }
        Ok(ExitCode::FAILURE)
    }
}

/// A throwaway directory under the OS temp dir, removed on drop whether
/// or not the caller succeeded — the same manual-temp-dir-plus-`Drop`
/// shape [`WorktreeCheckout`] already uses for `diff`/`impact`/`review`'s
/// git worktrees, reused here for two disposable bundle renders instead.
struct ScratchDir(std::path::PathBuf);

impl ScratchDir {
    fn new(label: &str) -> Self {
        ScratchDir(std::env::temp_dir().join(format!(
            "okf-rs-{label}-{}-{}",
            std::process::id(),
            fastrand_suffix()
        )))
    }

    fn path(&self) -> &std::path::Path {
        &self.0
    }
}

impl Drop for ScratchDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// Recursively compares two directory trees for byte-for-byte identity,
/// returning one human-readable line per relative path that differs
/// (present on only one side, or with different content) — empty means
/// the trees are identical. `label_a`/`label_b` name the two sides in
/// that output (e.g. "run 1"/"run 2", or "the committed bundle"/"a fresh
/// regenerate") — callers comparing genuinely different things, not just
/// two interchangeable runs, get an accurate message instead of a
/// generic "run 1"/"run 2" that wouldn't describe what's actually being
/// compared.
fn diff_dirs(
    a: &std::path::Path,
    b: &std::path::Path,
    label_a: &str,
    label_b: &str,
) -> Result<Vec<String>> {
    let mut a_files = BTreeMap::new();
    collect_files(a, a, &mut a_files)?;
    let mut b_files = BTreeMap::new();
    collect_files(b, b, &mut b_files)?;

    let all_paths: std::collections::BTreeSet<&String> =
        a_files.keys().chain(b_files.keys()).collect();
    let mut diffs = Vec::new();
    for rel in &all_paths {
        match (a_files.get(*rel), b_files.get(*rel)) {
            (Some(x), Some(y)) if x == y => {}
            (Some(_), Some(_)) => diffs.push(format!(
                "{rel} (content differs between {label_a} and {label_b})"
            )),
            (Some(_), None) => diffs.push(format!("{rel} (present in {label_a} only)")),
            (None, Some(_)) => diffs.push(format!("{rel} (present in {label_b} only)")),
            (None, None) => unreachable!("path came from one of the two maps"),
        }
    }
    Ok(diffs)
}

fn collect_files(
    root: &std::path::Path,
    dir: &std::path::Path,
    out: &mut BTreeMap<String, Vec<u8>>,
) -> Result<()> {
    for entry in std::fs::read_dir(dir).with_context(|| format!("reading {}", dir.display()))? {
        let entry = entry?;
        let path = entry.path();
        if path.is_dir() {
            collect_files(root, &path, out)?;
        } else {
            let relative = path
                .strip_prefix(root)
                .unwrap_or(&path)
                .to_string_lossy()
                .replace('\\', "/");
            let content = std::fs::read(&path)?;
            let content = if relative == "index.md" {
                strip_source_revision_line(&content)
            } else {
                content
            };
            out.insert(relative, content);
        }
    }
    Ok(())
}

/// Strips a `source_revision: ...` line from the bundle-root `index.md`'s
/// frontmatter before `diff_dirs` compares it, both for
/// `--check-determinism` (comparing two in-process analyses of the same
/// `HEAD` — harmless either way, since both sides would agree regardless)
/// and, the case this actually matters for, `--check-fresh` (comparing a
/// fresh regenerate against the real, already-committed bundle on disk):
/// without this, a commit that moves `HEAD` without touching the analyzed
/// source tree at all (a README edit, say) would make `--check-fresh`
/// report false staleness purely from this one line differing, never
/// from real content drift — see `okf-generator::write_root_index`'s own
/// docs for the fuller reasoning. Line-based rather than a full
/// YAML-aware strip, since this only ever needs to recognize the exact
/// shape `write_root_index` itself writes.
fn strip_source_revision_line(content: &[u8]) -> Vec<u8> {
    let Ok(text) = std::str::from_utf8(content) else {
        return content.to_vec();
    };
    text.lines()
        .filter(|line| !line.starts_with("source_revision:"))
        .collect::<Vec<_>>()
        .join("\n")
        .into_bytes()
}

/// Resolves `--enrich-*` flags, falling back to the matching
/// `OKF_ENRICH_*` environment variable, then erroring clearly (naming
/// both the flag and the env var) if neither supplies a required value.
/// `api_key` alone has no required fallback — a local server (Ollama, LM
/// Studio, LocalAI) typically doesn't need one. Shared by `generate
/// --enrich` and `suggest-links`, the two commands that talk to an
/// enrichment endpoint.
fn resolve_enrich_config(
    base_url: Option<String>,
    model: Option<String>,
    api_key: Option<String>,
) -> Result<okf_enrich::EnrichConfig> {
    let base_url = base_url
        .or_else(|| std::env::var("OKF_ENRICH_BASE_URL").ok())
        .ok_or_else(|| {
            anyhow::anyhow!(
                "this command requires --enrich-base-url (or the OKF_ENRICH_BASE_URL environment variable)"
            )
        })?;
    let model = model
        .or_else(|| std::env::var("OKF_ENRICH_MODEL").ok())
        .ok_or_else(|| {
            anyhow::anyhow!(
                "this command requires --enrich-model (or the OKF_ENRICH_MODEL environment variable)"
            )
        })?;
    let api_key = api_key.or_else(|| std::env::var("OKF_ENRICH_API_KEY").ok());
    Ok(okf_enrich::EnrichConfig {
        base_url,
        model,
        api_key,
    })
}

fn cmd_suggest_links(
    bundle: Option<PathBuf>,
    project: &std::path::Path,
    enrich_base_url: Option<String>,
    enrich_model: Option<String>,
    enrich_api_key: Option<String>,
    candidates: usize,
) -> Result<ExitCode> {
    let bundle = resolve_query_bundle(bundle, project);
    let config = resolve_enrich_config(enrich_base_url, enrich_model, enrich_api_key)?;
    let client = okf_enrich::EnrichClient::new(config);
    print_query_result(okf_query::suggest_links(&bundle, &client, candidates))
}

fn cmd_watch(
    path: &std::path::Path,
    output: Option<PathBuf>,
    debounce_ms: u64,
) -> Result<ExitCode> {
    // Just the canonicalized root is needed here -- `okf_watch::watch`
    // does its own full `Project::load` scan for the baseline regenerate,
    // so doing another one here first would walk the whole directory
    // tree twice before the first bundle is even produced.
    let project_root = path
        .canonicalize()
        .with_context(|| format!("failed to resolve project root {}", path.display()))?;
    let output = resolve_bundle_arg(&project_root, output);
    let cache_path = project_root.join(CACHE_FILE);

    println!(
        "Watching {} for changes (bundle: {}, Ctrl+C to stop)...",
        project_root.display(),
        output.display()
    );
    okf_watch::watch(
        &project_root,
        &output,
        &cache_path,
        std::time::Duration::from_millis(debounce_ms),
        |event| {
            println!(
                "Regenerated {} concepts ({} files parsed, {} reused from cache)",
                event.concepts, event.stats.reparsed, event.stats.reused
            );
        },
    )?;
    Ok(ExitCode::SUCCESS)
}

fn cmd_validate(bundle: Option<PathBuf>, project: &std::path::Path, ci: bool) -> Result<ExitCode> {
    // Falls back to the raw (uncanonicalized) path rather than erroring:
    // validating a bundle with no accompanying project checkout (e.g. one
    // fetched from elsewhere) is a legitimate use, and `--project` simply
    // won't resolve an `okf.toml` in that case.
    let project_root = project
        .canonicalize()
        .unwrap_or_else(|_| project.to_path_buf());
    let bundle = resolve_bundle_arg(&project_root, bundle);
    let report = okf_validator::validate_bundle(&bundle)?;

    if report.issues.is_empty() {
        println!("{} — no issues found", bundle.display());
        return Ok(ExitCode::SUCCESS);
    }

    for issue in &report.issues {
        let label = match issue.severity {
            Severity::Error => "error",
            Severity::Warning => "warning",
        };
        println!("{label}: {}: {}", issue.file, issue.message);
    }

    // `--ci` only promotes orphan warnings to failures, per its help text
    // and the README's documented behavior — every other warning class
    // (redundant duplicate links, the actor-format convention, relationship
    // provenance/symmetry mismatches) is advisory by design (spec §7, §11)
    // and stays non-fatal even under `--ci`. See issue #23.
    let fail = report.has_errors() || (ci && report.has_warning_of_kind(IssueKind::Orphan));
    if fail {
        Ok(ExitCode::FAILURE)
    } else {
        Ok(ExitCode::SUCCESS)
    }
}

/// Resolves `bundle`/`project` the same way `validate` does — an explicit
/// bundle path wins, otherwise `okf.toml`'s recorded output relative to
/// the (canonicalized) project root.
fn resolve_query_bundle(bundle: Option<PathBuf>, project: &std::path::Path) -> PathBuf {
    let project_root = project
        .canonicalize()
        .unwrap_or_else(|_| project.to_path_buf());
    resolve_bundle_arg(&project_root, bundle)
}

/// Prints an `okf-query` result the same way every `search`/`graph`
/// subcommand does: the text on success, or the error on stderr with a
/// non-zero exit — one place for that pairing instead of one per
/// subcommand.
fn print_query_result(result: Result<String>) -> Result<ExitCode> {
    match result {
        Ok(text) => {
            println!("{text}");
            Ok(ExitCode::SUCCESS)
        }
        Err(err) => {
            eprintln!("error: {err:#}");
            Ok(ExitCode::FAILURE)
        }
    }
}

/// The `--semantic`/`--enrich-*` flags bundled together, the same way
/// [`EnrichArgs`] bundles `generate --enrich`'s equivalents — these four
/// are only ever used together (all four or none), so they travel as one
/// parameter instead of growing `cmd_search`'s own parameter list every
/// time a new semantic-search option is added.
struct SemanticSearchArgs {
    enabled: bool,
    base_url: Option<String>,
    model: Option<String>,
    api_key: Option<String>,
}

fn cmd_search(
    query: &str,
    bundle: Option<PathBuf>,
    project: &std::path::Path,
    ranked: bool,
    semantic: SemanticSearchArgs,
) -> Result<ExitCode> {
    let bundle = resolve_query_bundle(bundle, project);
    if semantic.enabled {
        let config = resolve_enrich_config(semantic.base_url, semantic.model, semantic.api_key)?;
        let client = okf_enrich::EnrichClient::new(config);
        print_query_result(okf_query::search_semantic(&bundle, &client, query, 25))
    } else if ranked {
        print_query_result(okf_query::search_ranked(&bundle, query))
    } else {
        print_query_result(okf_query::search(&bundle, query))
    }
}

fn analyze_path(path: &std::path::Path) -> Result<okf_analyzer::AnalysisResult> {
    let project = Project::load(path)?;
    okf_analyzer::analyze(&project)
}

fn cmd_graph(query: GraphQuery) -> Result<ExitCode> {
    match query {
        GraphQuery::Callers {
            id,
            bundle,
            project,
        } => {
            let bundle = resolve_query_bundle(bundle, &project);
            print_query_result(okf_query::graph_callers(&bundle, &id))
        }
        GraphQuery::Callees {
            id,
            bundle,
            project,
        } => {
            let bundle = resolve_query_bundle(bundle, &project);
            print_query_result(okf_query::graph_callees(&bundle, &id))
        }
        GraphQuery::Cycles { bundle, project } => {
            let bundle = resolve_query_bundle(bundle, &project);
            print_query_result(okf_query::graph_cycles(&bundle))
        }
        GraphQuery::Isolated { bundle, project } => {
            let bundle = resolve_query_bundle(bundle, &project);
            print_query_result(okf_query::graph_isolated(&bundle))
        }
        GraphQuery::Api { bundle, project } => {
            let bundle = resolve_query_bundle(bundle, &project);
            print_query_result(okf_query::graph_api(&bundle))
        }
        GraphQuery::Modules { bundle, project } => {
            let bundle = resolve_query_bundle(bundle, &project);
            print_query_result(okf_query::graph_modules(&bundle))
        }
        GraphQuery::Stats { bundle, project } => {
            let bundle = resolve_query_bundle(bundle, &project);
            print_query_result(okf_query::graph_stats(&bundle))
        }
        GraphQuery::Path {
            from,
            to,
            bundle,
            project,
        } => {
            let bundle = resolve_query_bundle(bundle, &project);
            print_query_result(okf_query::graph_path(&bundle, &from, &to))
        }
        GraphQuery::Explain {
            from,
            to,
            bundle,
            project,
        } => {
            let bundle = resolve_query_bundle(bundle, &project);
            print_query_result(okf_query::explain(&bundle, &from, &to))
        }
        GraphQuery::Layers { bundle, project } => {
            let bundle = resolve_query_bundle(bundle, &project);
            print_query_result(okf_query::graph_layers(&bundle))
        }
        GraphQuery::Domains { bundle, project } => {
            let bundle = resolve_query_bundle(bundle, &project);
            print_query_result(okf_query::graph_domains(&bundle))
        }
        GraphQuery::Communities { bundle, project } => {
            let bundle = resolve_query_bundle(bundle, &project);
            print_query_result(okf_query::graph_communities(&bundle))
        }
        GraphQuery::Patterns { bundle, project } => {
            let bundle = resolve_query_bundle(bundle, &project);
            print_query_result(okf_query::graph_patterns(&bundle))
        }
        GraphQuery::Features { bundle, project } => {
            let bundle = resolve_query_bundle(bundle, &project);
            print_query_result(okf_query::graph_features(&bundle))
        }
    }
}

fn cmd_docs(
    bundle: Option<PathBuf>,
    project: &std::path::Path,
    output: Option<PathBuf>,
    format: DocsFormat,
) -> Result<ExitCode> {
    let bundle = resolve_query_bundle(bundle, project);
    let concepts = okf_query::load_concepts(&bundle)?;
    match format {
        DocsFormat::Html => {
            let output = output.unwrap_or_else(|| PathBuf::from("docs"));
            okf_docs::generate_html(&concepts, &output)?;
            println!(
                "Generated HTML documentation for {} concepts into {}",
                concepts.len(),
                output.display()
            );
        }
        DocsFormat::Markdown => {
            let output = output.unwrap_or_else(|| PathBuf::from("docs.md"));
            let markdown = okf_docs::generate_markdown(&concepts);
            std::fs::write(&output, markdown)
                .with_context(|| format!("failed to write {}", output.display()))?;
            println!(
                "Generated Markdown documentation for {} concepts into {}",
                concepts.len(),
                output.display()
            );
        }
        DocsFormat::Pdf => {
            let output = output.unwrap_or_else(|| PathBuf::from("docs.pdf"));
            let pdf = okf_docs::generate_pdf(&concepts)?;
            std::fs::write(&output, pdf)
                .with_context(|| format!("failed to write {}", output.display()))?;
            println!(
                "Generated PDF documentation for {} concepts into {}",
                concepts.len(),
                output.display()
            );
        }
        DocsFormat::Dita => {
            let output = output.unwrap_or_else(|| PathBuf::from("docs-dita"));
            okf_dita::export_dita(&concepts, &output)?;
            println!(
                "Generated DITA documentation for {} concepts into {}",
                concepts.len(),
                output.display()
            );
        }
        DocsFormat::Graphml => {
            let output = output.unwrap_or_else(|| PathBuf::from("docs.graphml"));
            let graphml = okf_docs::generate_graphml(&concepts);
            std::fs::write(&output, graphml)
                .with_context(|| format!("failed to write {}", output.display()))?;
            println!(
                "Generated GraphML graph for {} concepts into {}",
                concepts.len(),
                output.display()
            );
        }
        DocsFormat::Obsidian => {
            let output = output.unwrap_or_else(|| PathBuf::from("docs-obsidian"));
            okf_docs::generate_obsidian(&concepts, &output)?;
            println!(
                "Generated Obsidian vault for {} concepts into {}",
                concepts.len(),
                output.display()
            );
        }
    }
    Ok(ExitCode::SUCCESS)
}

/// A `git worktree` checkout of a specific ref, non-destructively created
/// alongside the repository (never touching the caller's working tree)
/// and always removed on drop, whether or not analysis succeeded.
struct WorktreeCheckout {
    repo_root: std::path::PathBuf,
    worktree_path: std::path::PathBuf,
}

impl WorktreeCheckout {
    fn new(repo_root: &std::path::Path, git_ref: &str) -> Result<Self> {
        let base = std::env::temp_dir().join(format!(
            "okf-rs-diff-{}-{}",
            std::process::id(),
            fastrand_suffix()
        ));
        let status = std::process::Command::new("git")
            .args(["worktree", "add", "--detach"])
            .arg(&base)
            .arg(git_ref)
            .current_dir(repo_root)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .context("failed to run `git worktree add`")?;
        if !status.success() {
            anyhow::bail!("`git worktree add` failed for ref `{git_ref}` — is it a valid ref?");
        }
        Ok(WorktreeCheckout {
            repo_root: repo_root.to_path_buf(),
            worktree_path: base,
        })
    }

    fn path(&self) -> &std::path::Path {
        &self.worktree_path
    }
}

impl Drop for WorktreeCheckout {
    fn drop(&mut self) {
        let _ = std::process::Command::new("git")
            .args(["worktree", "remove", "--force"])
            .arg(&self.worktree_path)
            .current_dir(&self.repo_root)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status();
    }
}

/// A short, unique-enough suffix for scratch directory names, without
/// pulling in a dedicated random/uuid dependency for one call site.
fn fastrand_suffix() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0)
}

fn git_repo_root(path: &std::path::Path) -> Result<std::path::PathBuf> {
    let output = std::process::Command::new("git")
        .args(["rev-parse", "--show-toplevel"])
        .current_dir(path)
        .output()
        .context("failed to run `git rev-parse --show-toplevel`")?;
    if !output.status.success() {
        anyhow::bail!("{} is not inside a git repository", path.display());
    }
    let root = String::from_utf8(output.stdout)
        .context("git output was not valid UTF-8")?
        .trim()
        .to_string();
    Ok(std::path::PathBuf::from(root))
}

/// Analyzes `path` as of two git refs, via disposable, non-destructive
/// `git worktree` checkouts (never touching the caller's working tree —
/// each [`WorktreeCheckout`] is removed on drop, whether or not analysis
/// succeeded). Shared by `diff`/`impact`/`review`, which only differ in
/// what they do with the two resulting [`okf_analyzer::AnalysisResult`]s.
fn analyze_two_refs(
    path: &std::path::Path,
    from_ref: &str,
    to_ref: &str,
) -> Result<(okf_analyzer::AnalysisResult, okf_analyzer::AnalysisResult)> {
    let repo_root = git_repo_root(path)?;
    let canonical_path = path
        .canonicalize()
        .with_context(|| format!("failed to resolve {}", path.display()))?;
    let relative_project = canonical_path
        .strip_prefix(&repo_root)
        .unwrap_or(std::path::Path::new("."));

    let from_checkout = WorktreeCheckout::new(&repo_root, from_ref)?;
    let from_result = analyze_path(&from_checkout.path().join(relative_project))?;

    let to_checkout = WorktreeCheckout::new(&repo_root, to_ref)?;
    let to_result = analyze_path(&to_checkout.path().join(relative_project))?;

    Ok((from_result, to_result))
}

fn cmd_diff(from_ref: &str, to_ref: &str, path: &std::path::Path, ci: bool) -> Result<ExitCode> {
    let (from_result, to_result) = analyze_two_refs(path, from_ref, to_ref)?;

    let report = okf_analyzer::diff(&from_result.concepts, &to_result.concepts);

    if ci {
        return cmd_diff_ci(&report, path, from_ref, to_ref);
    }

    if report.is_empty() {
        println!("No concept-level changes between {from_ref} and {to_ref}");
        return Ok(ExitCode::SUCCESS);
    }

    if !report.added.is_empty() {
        println!("Added ({}):", report.added.len());
        for concept in &report.added {
            println!("  + {} — {}", concept.id, concept.frontmatter_type());
        }
    }
    if !report.removed.is_empty() {
        println!("Removed ({}):", report.removed.len());
        for concept in &report.removed {
            println!("  - {} — {}", concept.id, concept.frontmatter_type());
        }
    }
    if !report.changed.is_empty() {
        println!("Changed ({}):", report.changed.len());
        for change in &report.changed {
            println!("  ~ {} — {}", change.id, change.kind.as_str());
            if change.before_signature != change.after_signature {
                if let Some(before) = &change.before_signature {
                    println!("      - {before}");
                }
                if let Some(after) = &change.after_signature {
                    println!("      + {after}");
                }
            }
        }
    }

    Ok(ExitCode::SUCCESS)
}

/// `okf-rs diff-bundles <bundle-a> <bundle-b>`: the same provenance-aware
/// classification `diff --ci` reports, but comparing two bundle
/// directories already on disk instead of re-analyzing two git refs — see
/// the `DiffBundles` variant's own doc comment for why this needs to
/// exist as a separate command rather than another `diff` flag (there is
/// no git ref to check out when the two bundles came from the same source
/// snapshot under two different resolver versions).
///
/// Always renders the `--ci`-style classified report (reusing
/// `render_ci_report` unchanged) rather than `diff`'s own plain
/// added/removed/changed listing: a bare list of "changed" concepts with
/// no provenance context would defeat the one thing this command exists
/// to surface, and — for its primary use case, comparing two resolver
/// versions against identical source — `added`/`removed` are expected to
/// always be empty anyway.
fn cmd_diff_bundles(
    bundle_a: &std::path::Path,
    bundle_b: &std::path::Path,
    project: &std::path::Path,
) -> Result<ExitCode> {
    let concepts_a = okf_parser::read_bundle(bundle_a)
        .with_context(|| format!("reading bundle at {}", bundle_a.display()))?;
    let concepts_b = okf_parser::read_bundle(bundle_b)
        .with_context(|| format!("reading bundle at {}", bundle_b.display()))?;

    let report = okf_analyzer::diff(&concepts_a, &concepts_b);
    let policy = okf_core::config::load(project).diff;
    let summary = okf_analyzer::ci_summary(&report);
    let resolver_only_rate = okf_analyzer::resolver_only_rate(&report);
    let (text, failed) = render_ci_report(
        &summary,
        resolver_only_rate,
        policy,
        &bundle_a.display().to_string(),
        &bundle_b.display().to_string(),
    );
    println!("{text}");
    Ok(if failed {
        ExitCode::FAILURE
    } else {
        ExitCode::SUCCESS
    })
}

/// `okf-rs diff --ci`: classifies `report` into source/resolver/confidence
/// counts (`okf_analyzer::ci_summary`), evaluates them against `okf.toml`'s
/// `[diff]` policy (`okf_core::config::DiffPolicy`), and prints/returns
/// what `render_ci_report` computed. All the actual logic lives in that
/// pure function; this is just its CLI glue (load config, print, turn a
/// bool into a process `ExitCode`) — see `render_ci_report`'s own docs.
fn cmd_diff_ci(
    report: &okf_analyzer::DiffReport,
    path: &std::path::Path,
    from_ref: &str,
    to_ref: &str,
) -> Result<ExitCode> {
    let policy = okf_core::config::load(path).diff;
    let summary = okf_analyzer::ci_summary(report);
    // Computed from the full `DiffReport` (not `summary`) -- see
    // `okf_analyzer::resolver_only_rate`'s own docs for why: `CiSummary`'s
    // `source_changes` also folds in whole-concept adds/removes and
    // signature-only changes, which would dilute this rate.
    let resolver_only_rate = okf_analyzer::resolver_only_rate(report);
    let (text, failed) = render_ci_report(&summary, resolver_only_rate, policy, from_ref, to_ref);
    println!("{text}");
    Ok(if failed {
        ExitCode::FAILURE
    } else {
        ExitCode::SUCCESS
    })
}

/// Renders an `okf_analyzer::CiSummary` against a `DiffPolicy` as
/// `okf-rs diff --ci`'s report text, and says whether it should fail the
/// exit code — pure data-in, data-out, so the full exit-code table
/// (`docs/improvement-plan-provenance-diff.md`'s Phase C) is directly
/// unit-testable without a real git repository or language server,
/// the same reason `render_review_markdown` is split out from `cmd_review`.
/// `resolver_only_rate` is a separate parameter rather than a field this
/// function derives from `summary` itself: `okf_analyzer::resolver_only_rate`
/// needs the full `DiffReport` (not `CiSummary`'s reduced counts) to avoid
/// concept-level churn diluting it — see that function's own docs — and
/// threading it through as plain data keeps this function exactly as
/// unit-testable against hand-built values as it was before.
///
/// Source changes are never configurable — always reported (❌) and
/// always failure-worthy. Resolver/confidence changes are reported with
/// whichever icon their policy implies (❌ `"fail"`, ⚠️ `"warn"`, ℹ️
/// `"ignore"`) and fail the exit code only under `"fail"`.
fn render_ci_report(
    summary: &okf_analyzer::CiSummary,
    resolver_only_rate: Option<f64>,
    policy: okf_core::config::DiffPolicy,
    from_ref: &str,
    to_ref: &str,
) -> (String, bool) {
    if summary.is_empty() {
        return (
            format!("No changes between {from_ref} and {to_ref}\n\nexit code: 0"),
            false,
        );
    }

    let mut lines = Vec::new();
    // Source changes are never configurable -- always both reported and
    // failure-worthy.
    let mut failed = summary.source_changes > 0;
    if summary.source_changes > 0 {
        lines.push(format!("❌ SOURCE CHANGES: {}", summary.source_changes));
    }
    if summary.resolver_changes > 0 {
        let (icon, fails) = policy_icon(policy.resolver_changes);
        failed |= fails;
        lines.push(format!(
            "{icon} RESOLVER CHANGES: {}",
            summary.resolver_changes
        ));
        // See `okf_analyzer::resolver_only_rate`'s own docs: the empirical
        // number that decides whether `resolver_changes` is worth
        // defaulting to "ignore" in this project's own `okf.toml`, rather
        // than guessing from one worked example. `{:.1}`, not `{:.0}`:
        // found by actually running `diff-bundles` against a real pair of
        // resolver-version bundles (see `docs/improvement-plan-provenance-diff.md`'s
        // Phase G) -- a real 99.85% (2 genuine source changes out of 1294
        // relationship-level changes) rounded to a bare "100%" at zero
        // decimal places, indistinguishable from an actually-clean diff
        // with zero source changes, which is exactly the distinction this
        // number exists to preserve.
        if let Some(rate) = resolver_only_rate {
            lines.push(format!(
                "   ({:.1}% of relationship-level changes in this diff were resolver-only)",
                rate * 100.0
            ));
        }
    }
    if summary.confidence_changes > 0 {
        let (icon, fails) = policy_icon(policy.confidence_changes);
        failed |= fails;
        lines.push(format!(
            "{icon} CONFIDENCE CHANGES: {}",
            summary.confidence_changes
        ));
    }
    lines.push(String::new());
    lines.push(format!("exit code: {}", if failed { 1 } else { 0 }));

    (lines.join("\n"), failed)
}

/// The icon a `--ci` category line renders with (❌ fail, ⚠️ warn, ℹ️
/// ignore), and whether that action should fail the exit code.
fn policy_icon(action: okf_core::config::PolicyAction) -> (&'static str, bool) {
    match action {
        okf_core::config::PolicyAction::Fail => ("❌", true),
        okf_core::config::PolicyAction::Warn => ("⚠️ ", false),
        okf_core::config::PolicyAction::Ignore => ("ℹ️ ", false),
    }
}

fn cmd_impact(from_ref: &str, to_ref: &str, path: &std::path::Path) -> Result<ExitCode> {
    let (from_result, to_result) = analyze_two_refs(path, from_ref, to_ref)?;

    let report = okf_analyzer::impact(&from_result.concepts, &to_result.concepts);

    if report.impacted.is_empty() {
        println!("No concept-level changes between {from_ref} and {to_ref}");
        return Ok(ExitCode::SUCCESS);
    }

    println!(
        "{} concept(s) changed between {from_ref} and {to_ref}, ordered by blast radius (most affected first):\n",
        report.impacted.len()
    );
    for impacted in &report.impacted {
        let change_label = match impacted.change {
            okf_analyzer::ChangeKind::Added => "+",
            okf_analyzer::ChangeKind::Removed => "-",
            okf_analyzer::ChangeKind::Changed => "~",
        };
        let mut flags = Vec::new();
        if impacted.is_public_api {
            flags.push("public API");
        }
        if impacted.in_cycle {
            flags.push("in a call cycle");
        }
        let flags_suffix = if flags.is_empty() {
            String::new()
        } else {
            format!(" [{}]", flags.join(", "))
        };
        println!(
            "  {change_label} {} — {}{flags_suffix}",
            impacted.id,
            impacted.kind.as_str()
        );
        if impacted.blast_radius.is_empty() {
            println!(
                "      blast radius: none (nothing else calls this, directly or transitively)"
            );
        } else {
            println!(
                "      blast radius: {} concept(s): {}",
                impacted.blast_radius.len(),
                impacted.blast_radius.join(", ")
            );
        }
    }

    Ok(ExitCode::SUCCESS)
}

/// A stable marker at the top of every `okf-rs review` report, so a bot
/// posting it as a pull-request comment can find its own prior comment
/// (e.g. by searching for this exact string) and update it in place
/// instead of piling up a new comment on every push.
const REVIEW_MARKER: &str = "<!-- okf-rs-review -->";

fn cmd_review(
    from_ref: &str,
    to_ref: &str,
    path: &std::path::Path,
    fail_on_risk: Option<usize>,
) -> Result<ExitCode> {
    let (from_result, to_result) = analyze_two_refs(path, from_ref, to_ref)?;

    let report = okf_analyzer::impact(&from_result.concepts, &to_result.concepts);
    println!("{}", render_review_markdown(&report, from_ref, to_ref));

    let risky = fail_on_risk.is_some_and(|threshold| {
        report
            .impacted
            .iter()
            .any(|c| c.blast_radius.len() >= threshold)
    });
    if risky {
        Ok(ExitCode::FAILURE)
    } else {
        Ok(ExitCode::SUCCESS)
    }
}

/// Renders an [`okf_analyzer::ImpactReport`] as a pull-request-comment-ready
/// Markdown report — pure text formatting over data `cmd_review` already
/// computed, so it's directly unit-testable without a real git repository.
fn render_review_markdown(
    report: &okf_analyzer::ImpactReport,
    from_ref: &str,
    to_ref: &str,
) -> String {
    let mut out =
        format!("{REVIEW_MARKER}\n## okf-rs impact report: `{from_ref}` → `{to_ref}`\n\n");

    if report.impacted.is_empty() {
        out.push_str("No concept-level changes between these two refs.\n");
        return out;
    }

    let total_blast_radius: std::collections::HashSet<&str> = report
        .impacted
        .iter()
        .flat_map(|c| c.blast_radius.iter().map(String::as_str))
        .collect();
    out.push_str(&format!(
        "{} concept(s) changed, affecting {} other concept(s) transitively (ordered by blast radius, most affected first).\n\n",
        report.impacted.len(),
        total_blast_radius.len()
    ));

    out.push_str("| Change | Concept | Kind | Blast radius | Public API | In cycle |\n");
    out.push_str("|---|---|---|---|---|---|\n");
    for impacted in &report.impacted {
        let change = match impacted.change {
            okf_analyzer::ChangeKind::Added => "➕ added",
            okf_analyzer::ChangeKind::Removed => "➖ removed",
            okf_analyzer::ChangeKind::Changed => "✏️ changed",
        };
        out.push_str(&format!(
            "| {change} | `{}` | {} | {} | {} | {} |\n",
            impacted.id,
            impacted.kind.as_str(),
            impacted.blast_radius.len(),
            if impacted.is_public_api { "✅" } else { "" },
            if impacted.in_cycle { "⚠️" } else { "" },
        ));
    }

    out.push_str(
        "\n_Deterministic, structural blast-radius analysis (`okf-rs impact`) — not a substitute for human review._\n",
    );
    out
}

#[cfg(test)]
mod review_tests {
    use super::*;
    use okf_analyzer::{ChangeKind, ImpactReport, ImpactedConcept};
    use okf_parser::ConceptKind;

    fn impacted(id: &str, change: ChangeKind, blast_radius: &[&str]) -> ImpactedConcept {
        ImpactedConcept {
            id: id.to_string(),
            kind: ConceptKind::Function,
            change,
            blast_radius: blast_radius.iter().map(|s| s.to_string()).collect(),
            is_public_api: true,
            in_cycle: false,
        }
    }

    #[test]
    fn renders_no_changes_as_a_clear_one_liner() {
        let report = ImpactReport::default();
        let markdown = render_review_markdown(&report, "main", "HEAD");
        assert!(markdown.starts_with(REVIEW_MARKER));
        assert!(markdown.contains("No concept-level changes"));
    }

    #[test]
    fn renders_a_table_row_per_changed_concept_with_blast_radius_and_flags() {
        let report = ImpactReport {
            diff: Default::default(),
            impacted: vec![impacted(
                "functions/callee",
                ChangeKind::Changed,
                &["functions/caller"],
            )],
        };
        let markdown = render_review_markdown(&report, "main", "feature");
        assert!(markdown.starts_with(REVIEW_MARKER));
        assert!(markdown.contains("okf-rs impact report: `main` → `feature`"));
        assert!(markdown.contains("1 concept(s) changed, affecting 1 other concept(s)"));
        assert!(markdown.contains("✏️ changed"));
        assert!(markdown.contains("`functions/callee`"));
        assert!(markdown.contains("| 1 |"));
        assert!(markdown.contains("✅"));
    }

    #[test]
    fn counts_the_union_of_blast_radii_not_a_sum_with_double_counting() {
        // Two changed concepts that share one common downstream caller:
        // the report's overall "N other concepts affected" figure must
        // count that shared concept once, not twice.
        let report = ImpactReport {
            diff: Default::default(),
            impacted: vec![
                impacted("functions/a", ChangeKind::Changed, &["functions/shared"]),
                impacted("functions/b", ChangeKind::Changed, &["functions/shared"]),
            ],
        };
        let markdown = render_review_markdown(&report, "main", "feature");
        assert!(markdown.contains("2 concept(s) changed, affecting 1 other concept(s)"));
    }
}

/// Covers `okf-rs diff --ci`'s full exit-code table
/// (`docs/improvement-plan-provenance-diff.md`'s Phase C) directly against
/// `render_ci_report`, entirely without a real git repository or language
/// server — a synthetic `CiSummary` plays the role two real `--lsp`
/// analyses would otherwise need to produce (in particular, a genuine
/// resolver-*version* difference needs two different installed
/// `rust-analyzer` binaries, which this environment — and CI — has no way
/// to guarantee; the classification logic that turns real relationship
/// pairs into these counts is `okf-analyzer`'s own job, and is tested
/// there against real provenance data).
#[cfg(test)]
mod diff_ci_tests {
    use super::*;
    use okf_analyzer::CiSummary;
    use okf_core::config::{DiffPolicy, PolicyAction};

    #[test]
    fn no_changes_exits_zero() {
        let (text, failed) =
            render_ci_report(&CiSummary::default(), None, DiffPolicy::default(), "a", "b");
        assert!(!failed);
        assert!(text.contains("No changes between a and b"));
        assert!(text.contains("exit code: 0"));
    }

    #[test]
    fn metadata_only_change_with_default_policy_exits_zero() {
        let summary = CiSummary {
            confidence_changes: 2,
            ..Default::default()
        };
        let (text, failed) = render_ci_report(&summary, None, DiffPolicy::default(), "a", "b");
        assert!(!failed);
        assert!(text.contains("ℹ️"));
        assert!(text.contains("CONFIDENCE CHANGES: 2"));
        assert!(text.contains("exit code: 0"));
    }

    #[test]
    fn resolver_only_change_with_default_policy_warns_but_exits_zero() {
        let summary = CiSummary {
            resolver_changes: 4,
            ..Default::default()
        };
        let (text, failed) = render_ci_report(&summary, Some(1.0), DiffPolicy::default(), "a", "b");
        assert!(!failed);
        assert!(text.contains("⚠️"));
        assert!(text.contains("RESOLVER CHANGES: 4"));
        assert!(text.contains("exit code: 0"));
    }

    /// The empirical number `docs/improvement-plan-provenance-diff.md`'s
    /// Phase G adds: when every relationship-level change in the diff is
    /// resolver-only, `--ci` says so as a rate, not just a bare count --
    /// the number a project would actually watch across real resolver
    /// version bumps to decide whether `resolver_changes` can default to
    /// `"ignore"` in its own `okf.toml`. `resolver_only_rate` is passed in
    /// directly here (not derived from `summary`) -- see `render_ci_report`'s
    /// own docs for why -- so this test locks in only the rendering, not
    /// the rate computation itself (covered by `okf_analyzer::resolver_only_rate`'s
    /// own tests against a real `DiffReport`).
    #[test]
    fn resolver_only_change_reports_its_share_of_relationship_level_changes() {
        let summary = CiSummary {
            resolver_changes: 4,
            ..Default::default()
        };
        let (text, _) = render_ci_report(&summary, Some(1.0), DiffPolicy::default(), "a", "b");
        assert!(
            text.contains("100.0% of relationship-level changes in this diff were resolver-only")
        );
    }

    /// A rate that isn't a round number renders at one decimal place, not
    /// rounded to a bare "100%" that would be indistinguishable from an
    /// actually-perfect rate -- the exact bug found by running
    /// `diff-bundles` against a real pair of resolver-version bundles
    /// (99.85% rounding to "100%" at zero decimal places, hiding that 2
    /// real source changes were present). `0.998454...` is the literal
    /// rate from that run (1292 resolver-only out of 1294 total
    /// relationship-level changes).
    #[test]
    fn resolver_only_rate_renders_at_one_decimal_place_not_rounded_to_a_bare_100_percent() {
        let summary = CiSummary {
            source_changes: 2,
            resolver_changes: 1292,
            ..Default::default()
        };
        let (text, _) = render_ci_report(
            &summary,
            Some(1292.0 / 1294.0),
            DiffPolicy::default(),
            "a",
            "b",
        );
        assert!(
            text.contains("99.8% of relationship-level changes in this diff were resolver-only")
        );
        assert!(!text.contains("100% of relationship-level changes"));
    }

    /// A resolver-only change mixed with a genuine source rewire reports a
    /// rate below 100% -- confirming this isn't just "resolver_changes >
    /// 0 -> print 100%" but an actual share of the total.
    #[test]
    fn resolver_only_rate_is_below_100_percent_when_source_changes_also_present() {
        let summary = CiSummary {
            source_changes: 1,
            resolver_changes: 1,
            ..Default::default()
        };
        let (text, _) = render_ci_report(&summary, Some(0.5), DiffPolicy::default(), "a", "b");
        assert!(
            text.contains("50.0% of relationship-level changes in this diff were resolver-only")
        );
    }

    /// `resolver_only_rate: None` (the "no relationship-level change pair
    /// to take a rate over" case) renders no rate line at all, rather than
    /// e.g. printing "0%" or panicking on the `None` -- `render_ci_report`
    /// only ever prints the rate line when it actually has one.
    #[test]
    fn no_rate_line_is_rendered_when_resolver_only_rate_is_none() {
        let summary = CiSummary {
            resolver_changes: 4,
            ..Default::default()
        };
        let (text, _) = render_ci_report(&summary, None, DiffPolicy::default(), "a", "b");
        assert!(text.contains("RESOLVER CHANGES: 4"));
        assert!(!text.contains("resolver-only"));
    }

    #[test]
    fn added_source_edge_exits_one() {
        let summary = CiSummary {
            source_changes: 1,
            ..Default::default()
        };
        let (text, failed) = render_ci_report(&summary, None, DiffPolicy::default(), "a", "b");
        assert!(failed);
        assert!(text.contains("❌ SOURCE CHANGES: 1"));
        assert!(text.contains("exit code: 1"));
    }

    #[test]
    fn removed_source_edge_exits_one() {
        // Same shape as an addition -- `CiSummary` doesn't distinguish
        // add vs. remove in the count itself (only which relationship
        // pair changed matters, not which direction), so this is the
        // same case as `added_source_edge_exits_one` from this
        // function's point of view; `okf-analyzer`'s own tests cover
        // that both directions really do produce a `SourceChange`.
        let summary = CiSummary {
            source_changes: 1,
            ..Default::default()
        };
        let (_, failed) = render_ci_report(&summary, None, DiffPolicy::default(), "a", "b");
        assert!(failed);
    }

    #[test]
    fn semantic_target_change_exits_one() {
        // A rewired target is two `SourceChange` entries (the removed
        // pair, the added pair) -- see
        // `okf_analyzer::diff_relationships`'s own scenario-3/5 tests.
        let summary = CiSummary {
            source_changes: 2,
            ..Default::default()
        };
        let (text, failed) = render_ci_report(&summary, None, DiffPolicy::default(), "a", "b");
        assert!(failed);
        assert!(text.contains("SOURCE CHANGES: 2"));
    }

    #[test]
    fn mixed_source_and_resolver_changes_exits_one() {
        let summary = CiSummary {
            source_changes: 1,
            resolver_changes: 1,
            ..Default::default()
        };
        let (text, failed) = render_ci_report(&summary, Some(0.5), DiffPolicy::default(), "a", "b");
        assert!(
            failed,
            "a source change fails regardless of the resolver change's own policy"
        );
        assert!(text.contains("❌ SOURCE CHANGES: 1"));
        assert!(text.contains("RESOLVER CHANGES: 1"));
    }

    #[test]
    fn resolver_only_change_fails_when_policy_is_fail() {
        let summary = CiSummary {
            resolver_changes: 4,
            ..Default::default()
        };
        let policy = DiffPolicy {
            resolver_changes: PolicyAction::Fail,
            confidence_changes: PolicyAction::Ignore,
        };
        let (text, failed) = render_ci_report(&summary, Some(1.0), policy, "a", "b");
        assert!(failed);
        assert!(text.contains("❌"));
        assert!(text.contains("RESOLVER CHANGES: 4"));
        assert!(text.contains("exit code: 1"));
    }

    #[test]
    fn resolver_ignore_policy_still_reports_but_never_fails() {
        let summary = CiSummary {
            resolver_changes: 3,
            ..Default::default()
        };
        let policy = DiffPolicy {
            resolver_changes: PolicyAction::Ignore,
            confidence_changes: PolicyAction::Ignore,
        };
        let (text, failed) = render_ci_report(&summary, Some(1.0), policy, "a", "b");
        assert!(!failed);
        assert!(text.contains("ℹ️"));
        assert!(text.contains("RESOLVER CHANGES: 3"));
    }

    #[test]
    fn confidence_fail_policy_fails_the_exit_code() {
        let summary = CiSummary {
            confidence_changes: 2,
            ..Default::default()
        };
        let policy = DiffPolicy {
            resolver_changes: PolicyAction::Warn,
            confidence_changes: PolicyAction::Fail,
        };
        let (text, failed) = render_ci_report(&summary, None, policy, "a", "b");
        assert!(failed);
        assert!(text.contains("❌"));
        assert!(text.contains("exit code: 1"));
    }

    #[test]
    fn provenance_change_is_reported_and_gated_as_a_resolver_change_not_confidence() {
        // A ProvenanceChange (resolved_by/resolver_version *and*
        // confidence both moved) is folded into `resolver_changes` by
        // `okf_analyzer::ci_summary`, so it follows `resolver_changes`'
        // policy, not `confidence_changes` -- this test locks in that
        // routing decision at the CLI-policy level, distinct from
        // `okf-analyzer`'s own test verifying the classification itself.
        let summary = CiSummary {
            resolver_changes: 1,
            ..Default::default()
        };
        let policy = DiffPolicy {
            resolver_changes: PolicyAction::Fail,
            confidence_changes: PolicyAction::Warn,
        };
        let (_, failed) = render_ci_report(&summary, Some(1.0), policy, "a", "b");
        assert!(
            failed,
            "resolver_changes = \"fail\" should gate a ProvenanceChange-derived count too"
        );
    }
}

#[cfg(test)]
mod determinism_tests {
    use super::*;
    use std::fs;

    #[test]
    fn diff_dirs_is_empty_for_byte_identical_trees() {
        let a = tempfile::tempdir().unwrap();
        let b = tempfile::tempdir().unwrap();
        fs::create_dir_all(a.path().join("functions")).unwrap();
        fs::create_dir_all(b.path().join("functions")).unwrap();
        fs::write(a.path().join("functions/f.md"), "same content\n").unwrap();
        fs::write(b.path().join("functions/f.md"), "same content\n").unwrap();

        assert!(diff_dirs(a.path(), b.path(), "run 1", "run 2")
            .unwrap()
            .is_empty());
    }

    #[test]
    fn diff_dirs_reports_content_that_differs_between_the_two_runs() {
        let a = tempfile::tempdir().unwrap();
        let b = tempfile::tempdir().unwrap();
        fs::write(a.path().join("f.md"), "run one\n").unwrap();
        fs::write(b.path().join("f.md"), "run two\n").unwrap();

        let diffs = diff_dirs(a.path(), b.path(), "run 1", "run 2").unwrap();
        assert_eq!(diffs.len(), 1);
        assert!(diffs[0].contains("f.md"));
        assert!(diffs[0].contains("content differs"));
    }

    #[test]
    fn diff_dirs_reports_a_file_present_on_only_one_side() {
        let a = tempfile::tempdir().unwrap();
        let b = tempfile::tempdir().unwrap();
        fs::write(a.path().join("only_in_a.md"), "x\n").unwrap();
        fs::write(b.path().join("only_in_b.md"), "x\n").unwrap();

        let diffs = diff_dirs(a.path(), b.path(), "run 1", "run 2").unwrap();
        assert_eq!(diffs.len(), 2);
        assert!(diffs
            .iter()
            .any(|d| d.contains("only_in_a.md") && d.contains("run 1 only")));
        assert!(diffs
            .iter()
            .any(|d| d.contains("only_in_b.md") && d.contains("run 2 only")));
    }

    #[test]
    fn diff_dirs_uses_the_given_labels_not_a_hardcoded_run_1_run_2() {
        let a = tempfile::tempdir().unwrap();
        let b = tempfile::tempdir().unwrap();
        fs::write(a.path().join("only_in_committed.md"), "x\n").unwrap();

        let diffs = diff_dirs(
            a.path(),
            b.path(),
            "the committed bundle",
            "a fresh regenerate",
        )
        .unwrap();
        assert_eq!(diffs.len(), 1);
        assert!(diffs[0].contains("present in the committed bundle only"));
    }

    #[test]
    fn diff_dirs_walks_nested_subdirectories() {
        let a = tempfile::tempdir().unwrap();
        let b = tempfile::tempdir().unwrap();
        fs::create_dir_all(a.path().join("functions/nested")).unwrap();
        fs::create_dir_all(b.path().join("functions/nested")).unwrap();
        fs::write(a.path().join("functions/nested/f.md"), "a\n").unwrap();
        fs::write(b.path().join("functions/nested/f.md"), "b\n").unwrap();

        let diffs = diff_dirs(a.path(), b.path(), "run 1", "run 2").unwrap();
        assert_eq!(diffs.len(), 1);
        assert!(diffs[0].contains("functions/nested/f.md"));
    }

    #[test]
    fn strip_source_revision_line_removes_only_that_line() {
        let input = b"---\nokf_version: \"0.2\"\ngenerator_name: \"okf-rs\"\ngenerator_version: \"0.4.0\"\nsource_revision: \"abc123\"\n---\n\n# Knowledge Base\n";
        let stripped = strip_source_revision_line(input);
        let stripped = String::from_utf8(stripped).unwrap();
        assert!(!stripped.contains("source_revision"));
        assert!(stripped.contains("generator_version: \"0.4.0\""));
        assert!(stripped.contains("# Knowledge Base"));
    }

    #[test]
    fn strip_source_revision_line_is_a_no_op_without_one() {
        let input = b"---\nokf_version: \"0.2\"\n---\n\n# Knowledge Base\n";
        let stripped = strip_source_revision_line(input);
        assert_eq!(
            String::from_utf8(stripped).unwrap(),
            "---\nokf_version: \"0.2\"\n---\n\n# Knowledge Base"
        );
    }

    /// The regression test for the false-staleness risk this phase's own
    /// design doc calls out: two root `index.md` files differing *only*
    /// in `source_revision` (the shape `--check-fresh` sees whenever
    /// `HEAD` has moved since the committed bundle was generated, even if
    /// the analyzed source itself didn't change) must report no diff.
    #[test]
    fn diff_dirs_ignores_a_source_revision_only_difference_in_the_root_index() {
        let a = tempfile::tempdir().unwrap();
        let b = tempfile::tempdir().unwrap();
        fs::write(
            a.path().join("index.md"),
            "---\nokf_version: \"0.2\"\ngenerator_name: \"okf-rs\"\ngenerator_version: \"0.4.0\"\nsource_revision: \"aaaaaaa\"\n---\n\n# Knowledge Base\n",
        )
        .unwrap();
        fs::write(
            b.path().join("index.md"),
            "---\nokf_version: \"0.2\"\ngenerator_name: \"okf-rs\"\ngenerator_version: \"0.4.0\"\nsource_revision: \"bbbbbbb\"\n---\n\n# Knowledge Base\n",
        )
        .unwrap();

        assert!(
            diff_dirs(
                a.path(),
                b.path(),
                "the committed bundle",
                "a fresh regenerate"
            )
            .unwrap()
            .is_empty(),
            "a source_revision-only difference must not be reported as staleness"
        );
    }

    /// The complementary case: a *real* content difference in the root
    /// index (not just `source_revision`) must still be caught —
    /// normalization strips one specific line, not the whole file's
    /// ability to be diffed.
    #[test]
    fn diff_dirs_still_catches_a_real_root_index_content_difference() {
        let a = tempfile::tempdir().unwrap();
        let b = tempfile::tempdir().unwrap();
        fs::write(
            a.path().join("index.md"),
            "---\nokf_version: \"0.2\"\n---\n\n# Knowledge Base\n\n- [Functions](functions/index.md) (1)\n",
        )
        .unwrap();
        fs::write(
            b.path().join("index.md"),
            "---\nokf_version: \"0.2\"\n---\n\n# Knowledge Base\n\n- [Functions](functions/index.md) (2)\n",
        )
        .unwrap();

        let diffs = diff_dirs(a.path(), b.path(), "a", "b").unwrap();
        assert_eq!(diffs.len(), 1);
        assert!(diffs[0].contains("index.md"));
    }

    #[test]
    fn scratch_dir_is_removed_on_drop() {
        let path = {
            let scratch = ScratchDir::new("determinism-test");
            fs::create_dir_all(scratch.path()).unwrap();
            fs::write(scratch.path().join("f.txt"), "x").unwrap();
            assert!(scratch.path().exists());
            scratch.path().to_path_buf()
        };
        assert!(!path.exists());
    }
}
