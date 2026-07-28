mod config;

use anyhow::Result;
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
        Command::Init { path, output } => cmd_init(&path, &output),
        Command::Scan { path } => cmd_scan(&path),
        Command::Generate { path, output } => cmd_generate(&path, output),
        Command::Validate {
            bundle,
            project,
            ci,
        } => cmd_validate(bundle, &project, ci),
        Command::Search {
            query,
            bundle,
            project,
        } => cmd_search(&query, bundle, &project),
    }
}

/// Resolves the bundle path for a command: an explicit path always wins
/// (used as-is, relative to the caller's current directory, standard CLI
/// convention); otherwise falls back to `okf.toml`'s `output`, which is
/// relative to `project_root` — not the current directory — since that's
/// what it was recorded against, so it's joined here rather than left to
/// resolve against whatever directory the command happens to run from.
fn resolve_bundle_arg(project_root: &std::path::Path, explicit: Option<PathBuf>) -> PathBuf {
    explicit.unwrap_or_else(|| project_root.join(config::load(project_root).output))
}

fn cmd_init(path: &std::path::Path, output: &std::path::Path) -> Result<ExitCode> {
    let project = Project::load(path)?;
    let config_path = config::write_default(&project.root, output)?;
    println!(
        "Initialized okf-rs project at {} ({} source files detected)",
        project.root.display(),
        project.files.len()
    );
    println!("Wrote {}", config_path.display());
    Ok(ExitCode::SUCCESS)
}

fn cmd_scan(path: &std::path::Path) -> Result<ExitCode> {
    let project = Project::load(path)?;
    println!("Project root: {}", project.root.display());
    match project.manifest {
        Some(kind) => println!("Manifest: {kind:?}"),
        None => println!("Manifest: none detected"),
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

fn cmd_generate(path: &std::path::Path, output: Option<PathBuf>) -> Result<ExitCode> {
    let project = Project::load(path)?;
    let output = resolve_bundle_arg(&project.root, output);

    let result = okf_analyzer::analyze(&project)?;
    okf_generator::write_bundle(&result.concepts, &output)?;

    let mut by_kind: BTreeMap<ConceptKind, usize> = BTreeMap::new();
    for concept in &result.concepts {
        *by_kind.entry(concept.kind).or_default() += 1;
    }
    println!(
        "Generated {} concepts into {}",
        result.concepts.len(),
        output.display()
    );
    for (kind, count) in by_kind {
        println!("  {:<12} {count}", kind.as_str());
    }
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

fn cmd_search(query: &str, bundle: Option<PathBuf>, project: &std::path::Path) -> Result<ExitCode> {
    let project_root = project
        .canonicalize()
        .unwrap_or_else(|_| project.to_path_buf());
    let bundle = resolve_bundle_arg(&project_root, bundle);
    let index = okf_search::SearchIndex::build(&bundle)?;
    let hits = index.search(query);

    if hits.is_empty() {
        println!("No matches for `{query}` in {}", bundle.display());
        return Ok(ExitCode::SUCCESS);
    }

    for hit in hits {
        println!(
            "{:>3}  {:<24} {:<20} {}",
            hit.score, hit.entry.title, hit.entry.concept_type, hit.entry.id
        );
    }
    Ok(ExitCode::SUCCESS)
}
