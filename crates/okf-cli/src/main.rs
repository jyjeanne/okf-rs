use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use okf_core::Project;
use okf_parser::ConceptKind;
use okf_validator::Severity;
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
    },
    /// Generate human-readable documentation from a previously generated OKF
    /// bundle: either a browsable static HTML site, or a single consolidated
    /// Markdown document.
    Docs {
        /// Defaults to the value in `okf.toml`, or `knowledge`.
        bundle: Option<PathBuf>,
        /// Project directory to look up `okf.toml` in (not the bundle itself).
        #[arg(short, long, default_value = ".")]
        project: PathBuf,
        /// Output path: a directory for `--format html`, a file for
        /// `--format markdown`. Defaults to `docs/` or `docs.md` respectively.
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
        } => cmd_generate(&path, output, no_cache, lsp),
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
        } => cmd_search(&query, bundle, &project, ranked),
        Command::Coverage { bundle, project } => {
            let bundle = resolve_query_bundle(bundle, &project);
            print_query_result(okf_query::coverage(&bundle))
        }
        Command::Graph { query } => cmd_graph(query),
        Command::Diff {
            from_ref,
            to_ref,
            path,
        } => cmd_diff(&from_ref, &to_ref, &path),
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

fn cmd_generate(
    path: &std::path::Path,
    output: Option<PathBuf>,
    no_cache: bool,
    lsp: bool,
) -> Result<ExitCode> {
    let project = Project::load(path)?;
    let output = resolve_bundle_arg(&project.root, output);
    let cache_path = project.root.join(CACHE_FILE);

    let mut cache = if no_cache {
        okf_analyzer::AnalysisCache::default()
    } else {
        okf_analyzer::AnalysisCache::load(&cache_path)
    };
    let (result, stats) = okf_analyzer::analyze_with_cache_lsp(&project, &mut cache, lsp)?;
    okf_generator::write_bundle(&result.concepts, &output)?;
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
    for (kind, count) in by_kind {
        println!("  {:<12} {count}", kind.as_str());
    }
    Ok(ExitCode::SUCCESS)
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

    let fail = report.has_errors()
        || (ci
            && report
                .issues
                .iter()
                .any(|i| i.severity == Severity::Warning));
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

fn cmd_search(
    query: &str,
    bundle: Option<PathBuf>,
    project: &std::path::Path,
    ranked: bool,
) -> Result<ExitCode> {
    let bundle = resolve_query_bundle(bundle, project);
    if ranked {
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

fn cmd_diff(from_ref: &str, to_ref: &str, path: &std::path::Path) -> Result<ExitCode> {
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

    let report = okf_analyzer::diff(&from_result.concepts, &to_result.concepts);

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
