//! End-to-end tests for the `okf-rs` standalone executable.
//!
//! Every test here shells out to the actual compiled binary (via
//! `CARGO_BIN_EXE_okf-rs`, the same mechanism `cargo test` uses to build
//! and locate a crate's own `[[bin]]`), the same way a user invokes it
//! from a terminal — not by calling library functions directly. That's
//! the point: it exercises argument parsing, process exit codes, and
//! stdout/stderr, none of which the unit tests in the library crates
//! touch.
//!
//! The analysis target is this repository's own source tree, copied into
//! a scratch directory per test so `init`/`generate`/`docs` etc. can
//! freely write files without touching the real checkout or racing each
//! other across parallel tests.

use std::fs;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{mpsc, Arc};
use std::time::Duration;

fn bin_path() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_okf-rs"))
}

/// This repository's root, derived from the `okf-cli` crate's own
/// manifest directory (`<repo>/crates/okf-cli`) rather than the test
/// process's current directory, which `cargo test` doesn't guarantee.
fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("crates/okf-cli should be two levels below the repo root")
        .to_path_buf()
}

/// Copies this repository's source into `dst`, skipping build artifacts
/// (`target/`) and VCS metadata (`.git/`) — the same things the analyzer
/// itself ignores, and copying them would only make the fixture slower
/// and (for `.git/`) confuse `okf-rs diff`'s own git-root detection.
fn copy_repo_into(dst: &Path) {
    copy_dir(&repo_root(), dst);
}

fn copy_dir(src: &Path, dst: &Path) {
    fs::create_dir_all(dst).unwrap();
    for entry in fs::read_dir(src).unwrap() {
        let entry = entry.unwrap();
        let name = entry.file_name();
        if name == "target" || name == ".git" {
            continue;
        }
        let src_path = entry.path();
        let dst_path = dst.join(&name);
        let file_type = entry.file_type().unwrap();
        if file_type.is_dir() {
            copy_dir(&src_path, &dst_path);
        } else if file_type.is_file() {
            fs::copy(&src_path, &dst_path).unwrap();
        }
    }
}

fn run(dir: &Path, args: &[&str]) -> Output {
    Command::new(bin_path())
        .args(args)
        .current_dir(dir)
        .output()
        .unwrap_or_else(|e| panic!("failed to run `okf-rs {}`: {e}", args.join(" ")))
}

fn stdout_of(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

fn stderr_of(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

/// Whether a real, runnable `rust-analyzer` is on `PATH` — checked the
/// same way `okf-lsp::probe_available` does (a rustup proxy stub for an
/// uninstalled component is present as a file but exits immediately with
/// an "unknown binary" error, so merely finding the name isn't enough).
/// Duplicated here rather than depending on `okf-lsp` from this crate's
/// tests, since a plain subprocess probe is all a CLI-level test needs.
fn rust_analyzer_available() -> bool {
    Command::new("rust-analyzer")
        .arg("--version")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|status| status.success())
        .unwrap_or(false)
}

fn assert_success(output: &Output, args: &[&str]) {
    assert!(
        output.status.success(),
        "`okf-rs {}` failed\nstdout: {}\nstderr: {}",
        args.join(" "),
        stdout_of(output),
        stderr_of(output)
    );
}

/// Pulls the first concept id out of `okf-rs graph api` output (each
/// entry line looks like `  Rust Struct  <id>`, after a `N public
/// concepts:` header) so the graph queries below exercise a real id from
/// the generated bundle instead of hardcoding one that could drift as
/// this project's own source changes.
fn first_public_id(api_output: &str) -> String {
    api_output
        .lines()
        .find_map(|line| {
            let trimmed = line.trim();
            if trimmed.is_empty() || trimmed.ends_with(':') {
                None
            } else {
                trimmed.split_whitespace().last().map(str::to_string)
            }
        })
        .expect("expected at least one public concept in this project's own bundle")
}

/// Runs the full command surface of the standalone `okf-rs` executable
/// against a copy of this repository, in the order a real user would:
/// scan, init, generate (twice, to exercise the incremental cache),
/// validate, search, every `graph` subcommand, and both `docs` formats.
#[test]
fn standalone_binary_runs_the_full_command_surface_against_this_project() {
    let workspace = tempfile::tempdir().unwrap();
    let project = workspace.path().join("project");
    copy_repo_into(&project);

    // `--version`/`--help` need no project at all.
    let version = run(&project, &["--version"]);
    assert_success(&version, &["--version"]);
    assert!(stdout_of(&version).contains("okf-rs"));

    let help = run(&project, &["--help"]);
    assert_success(&help, &["--help"]);
    assert!(stdout_of(&help).contains("Generate, validate, and search"));

    // scan: reports the Rust workspace without writing anything.
    let scan = run(&project, &["scan", "."]);
    assert_success(&scan, &["scan"]);
    let scan_out = stdout_of(&scan);
    assert!(scan_out.contains("source files:"));
    assert!(scan_out.contains("Rust"));

    // init: writes/refreshes okf.toml plus the agent entry-point files.
    // This repository already checks in an `okf.toml` of its own (this
    // copy included it too) — `init` overwriting it in place is exactly
    // the idempotent behavior being tested here.
    let init = run(&project, &["init", ".", "--output", "knowledge"]);
    assert_success(&init, &["init"]);
    assert!(project.join("okf.toml").exists());
    assert!(project.join("CLAUDE.md").exists());
    assert!(project.join("AGENTS.md").exists());
    assert!(project.join(".github/copilot-instructions.md").exists());

    // generate: analyzes the workspace and writes the OKF bundle.
    let generate = run(&project, &["generate", "."]);
    assert_success(&generate, &["generate"]);
    let generate_out = stdout_of(&generate);
    assert!(generate_out.contains("Generated"));
    assert!(generate_out.contains("concepts into"));
    assert!(generate_out.contains("Function"));
    let bundle = project.join("knowledge");
    assert!(bundle.join("index.md").exists());
    assert!(project.join(".okf-cache.json").exists());

    // Re-running generate unchanged must reuse every file from the cache.
    let regenerate = run(&project, &["generate", "."]);
    assert_success(&regenerate, &["generate (cached)"]);
    assert!(stdout_of(&regenerate).contains("(0 files parsed,"));

    // validate: the freshly generated bundle must be conformant, with and
    // without the stricter `--ci` warnings-as-errors gate.
    let validate = run(&project, &["validate", "--project", "."]);
    assert_success(&validate, &["validate"]);
    assert!(stdout_of(&validate).contains("no issues found"));

    let validate_ci = run(&project, &["validate", "--project", ".", "--ci"]);
    assert_success(&validate_ci, &["validate --ci"]);

    // search: a symbol this repository is guaranteed to define
    // (`okf_core::Project`).
    let search = run(&project, &["search", "Project", "--project", "."]);
    assert_success(&search, &["search"]);
    assert!(stdout_of(&search).contains("Project"));

    // graph api: also the source of a real concept id for the queries below.
    let api = run(&project, &["graph", "api", "--project", "."]);
    assert_success(&api, &["graph api"]);
    let api_out = stdout_of(&api);
    assert!(api_out.contains("public concepts:"));
    let id = first_public_id(&api_out);

    let callers = run(&project, &["graph", "callers", &id, "--project", "."]);
    assert_success(&callers, &["graph callers"]);

    let callees = run(&project, &["graph", "callees", &id, "--project", "."]);
    assert_success(&callees, &["graph callees"]);

    let cycles = run(&project, &["graph", "cycles", "--project", "."]);
    assert_success(&cycles, &["graph cycles"]);

    let modules = run(&project, &["graph", "modules", "--project", "."]);
    assert_success(&modules, &["graph modules"]);

    // graph path: a concept's path to itself is always the trivial
    // single-node path — a deterministic assertion without needing to
    // know two ids that are actually connected.
    let path = run(&project, &["graph", "path", &id, &id, "--project", "."]);
    assert_success(&path, &["graph path"]);
    assert_eq!(stdout_of(&path).trim(), id);

    // explore: one-call context (signature/callers/callees/blast radius)
    // for a real concept id from this project's own bundle.
    let explore = run(&project, &["explore", &id, "--project", "."]);
    assert_success(&explore, &["explore"]);
    let explore_out = stdout_of(&explore);
    assert!(explore_out.starts_with(&id));
    assert!(explore_out.contains("Callers ("));
    assert!(explore_out.contains("Blast radius ("));

    // graph queries reject unknown ids with a clear, non-zero-exit error.
    let unknown = run(
        &project,
        &[
            "graph",
            "callers",
            "functions/does/not/exist",
            "--project",
            ".",
        ],
    );
    assert!(!unknown.status.success());
    assert!(stderr_of(&unknown).contains("no concept with id"));

    // docs --format markdown: a single consolidated file.
    let docs_md = run(
        &project,
        &[
            "docs",
            "--project",
            ".",
            "--format",
            "markdown",
            "--output",
            "docs.md",
        ],
    );
    assert_success(&docs_md, &["docs markdown"]);
    let docs_md_path = project.join("docs.md");
    assert!(docs_md_path.exists());
    assert!(fs::metadata(&docs_md_path).unwrap().len() > 0);

    // docs --format html: a browsable static site.
    let docs_html = run(
        &project,
        &[
            "docs",
            "--project",
            ".",
            "--format",
            "html",
            "--output",
            "docs-site",
        ],
    );
    assert_success(&docs_html, &["docs html"]);
    assert!(project.join("docs-site/index.html").exists());

    // docs --format pdf: a single paginated PDF.
    let docs_pdf = run(
        &project,
        &[
            "docs",
            "--project",
            ".",
            "--format",
            "pdf",
            "--output",
            "docs.pdf",
        ],
    );
    assert_success(&docs_pdf, &["docs pdf"]);
    let docs_pdf_path = project.join("docs.pdf");
    let pdf_bytes = fs::read(&docs_pdf_path).unwrap();
    assert!(
        pdf_bytes.starts_with(b"%PDF-"),
        "expected a well-formed PDF header"
    );
    assert!(pdf_bytes.len() > 1000, "suspiciously small PDF output");

    // docs --format dita: one DITA topic per concept plus a root map.
    let docs_dita = run(
        &project,
        &[
            "docs",
            "--project",
            ".",
            "--format",
            "dita",
            "--output",
            "docs-dita",
        ],
    );
    assert_success(&docs_dita, &["docs dita"]);
    assert!(project.join("docs-dita/root.ditamap").exists());

    // docs --format graphml: a single GraphML graph for Gephi/yEd/etc.
    let docs_graphml = run(
        &project,
        &[
            "docs",
            "--project",
            ".",
            "--format",
            "graphml",
            "--output",
            "docs.graphml",
        ],
    );
    assert_success(&docs_graphml, &["docs graphml"]);
    let docs_graphml_path = project.join("docs.graphml");
    let graphml_content = fs::read_to_string(&docs_graphml_path).unwrap();
    assert!(graphml_content.starts_with("<?xml version=\"1.0\""));
    assert!(graphml_content.contains("<node id="));

    // docs --format obsidian: one note per concept plus a root index.
    let docs_obsidian = run(
        &project,
        &[
            "docs",
            "--project",
            ".",
            "--format",
            "obsidian",
            "--output",
            "docs-obsidian",
        ],
    );
    assert_success(&docs_obsidian, &["docs obsidian"]);
    assert!(project.join("docs-obsidian/index.md").exists());
}

/// `okf-rs generate --dita` imports a DITA corpus as `Document` concepts,
/// merged into the same bundle as concepts extracted from source — and,
/// since export and import are the two halves of one converter, this
/// exercises them together: export a tiny bundle to DITA, then import
/// that exact output back into a second project's `generate --dita` run.
#[test]
fn standalone_binary_generate_dita_imports_topics_as_document_concepts() {
    let workspace = tempfile::tempdir().unwrap();

    let source_project = workspace.path().join("source");
    fs::create_dir_all(source_project.join("src")).unwrap();
    fs::write(
        source_project.join("src/lib.rs"),
        "pub fn hello() -> &'static str {\n    \"hello\"\n}\n",
    )
    .unwrap();
    let generate = run(&source_project, &["generate", "."]);
    assert_success(&generate, &["generate (source project)"]);

    let dita_dir = workspace.path().join("dita-export");
    let export = run(
        &source_project,
        &[
            "docs",
            "--project",
            ".",
            "--format",
            "dita",
            "--output",
            dita_dir.to_str().unwrap(),
        ],
    );
    assert_success(&export, &["docs --format dita"]);

    let target_project = workspace.path().join("target");
    fs::create_dir_all(target_project.join("src")).unwrap();
    fs::write(
        target_project.join("src/lib.rs"),
        "pub fn world() -> &'static str {\n    \"world\"\n}\n",
    )
    .unwrap();
    let import = run(
        &target_project,
        &["generate", ".", "--dita", dita_dir.to_str().unwrap()],
    );
    assert_success(&import, &["generate --dita"]);
    let import_out = stdout_of(&import);
    assert!(import_out.contains("Imported 2 DITA topics as Document concepts"));
    assert!(import_out.contains("Document     2"));

    let validate = run(&target_project, &["validate", "--project", "."]);
    assert_success(&validate, &["validate"]);
    assert!(stdout_of(&validate).contains("no issues found"));

    // The imported Document concept is searchable alongside the target
    // project's own extracted code, not living in a separate system.
    let search = run(&target_project, &["search", "hello", "--project", "."]);
    assert_success(&search, &["search"]);
    assert!(stdout_of(&search).contains("DITA Document"));
}

/// `okf-rs diff` compares two git refs' OKF concepts via disposable
/// worktrees. Exercised against a small synthetic repo (rather than this
/// project's own history) so the expected added/removed set is exact and
/// doesn't depend on this repository's commit history.
#[test]
fn standalone_binary_diff_reports_added_and_removed_concepts() {
    let workspace = tempfile::tempdir().unwrap();
    let repo = workspace.path().join("diff-repo");
    fs::create_dir_all(repo.join("src")).unwrap();

    let git = |args: &[&str]| {
        let status = Command::new("git")
            .args(args)
            .current_dir(&repo)
            .status()
            .expect("failed to run git");
        assert!(status.success(), "git {args:?} failed");
    };

    git(&["init", "-q"]);
    // Local, throwaway identity for this fixture repo's commits only —
    // it never leaves the temp directory, so it's independent of
    // whatever git identity/signing the host environment has configured.
    git(&["config", "user.name", "okf-rs e2e tests"]);
    git(&["config", "user.email", "e2e@example.invalid"]);
    git(&["config", "commit.gpgsign", "false"]);

    fs::write(repo.join("src/lib.rs"), "pub fn foo() -> i32 {\n    1\n}\n").unwrap();
    git(&["add", "-A"]);
    git(&["commit", "-q", "-m", "c1"]);
    let c1 = String::from_utf8(
        Command::new("git")
            .args(["rev-parse", "HEAD"])
            .current_dir(&repo)
            .output()
            .unwrap()
            .stdout,
    )
    .unwrap()
    .trim()
    .to_string();

    fs::write(
        repo.join("src/lib.rs"),
        "pub fn bar() -> i32 {\n    2\n}\n\npub fn baz() -> i32 {\n    3\n}\n",
    )
    .unwrap();
    git(&["add", "-A"]);
    git(&["commit", "-q", "-m", "c2"]);
    let c2 = String::from_utf8(
        Command::new("git")
            .args(["rev-parse", "HEAD"])
            .current_dir(&repo)
            .output()
            .unwrap()
            .stdout,
    )
    .unwrap()
    .trim()
    .to_string();

    let diff = run(&repo, &["diff", &c1, &c2, "."]);
    assert_success(&diff, &["diff"]);
    let diff_out = stdout_of(&diff);
    assert!(diff_out.contains("Added (2):"));
    assert!(diff_out.contains("functions/src/bar"));
    assert!(diff_out.contains("functions/src/baz"));
    assert!(diff_out.contains("Removed (1):"));
    assert!(diff_out.contains("functions/src/foo"));

    // No changes between a ref and itself.
    let no_diff = run(&repo, &["diff", &c2, &c2, "."]);
    assert_success(&no_diff, &["diff (no-op)"]);
    assert!(stdout_of(&no_diff).contains("No concept-level changes"));
}

/// `okf-rs diff --ci` against the same two commits `diff` (no `--ci`)
/// already covers above: added/removed concepts are always source-level
/// changes, so `--ci` must exit non-zero and report them under
/// `❌ SOURCE CHANGES`, while a ref diffed against itself must exit zero.
/// The resolver-change/confidence-change branches of the exit-code table
/// (`docs/improvement-plan-provenance-diff.md`'s Phase C) aren't
/// reachable this way — they need a real `--lsp`-resolved edge, and
/// `okf-rs diff`'s underlying two-ref analysis doesn't run `--lsp` at
/// all (a pre-existing limitation, not new to this test) — those are
/// covered directly against `render_ci_report` in `okf-cli`'s own
/// `diff_ci_tests` instead, with synthetic `CiSummary` data.
#[test]
fn standalone_binary_diff_ci_reports_source_changes_and_fails_the_exit_code() {
    let workspace = tempfile::tempdir().unwrap();
    let repo = workspace.path().join("diff-ci-repo");
    fs::create_dir_all(repo.join("src")).unwrap();

    let git = |args: &[&str]| {
        let status = Command::new("git")
            .args(args)
            .current_dir(&repo)
            .status()
            .expect("failed to run git");
        assert!(status.success(), "git {args:?} failed");
    };
    let rev_parse_head = || {
        String::from_utf8(
            Command::new("git")
                .args(["rev-parse", "HEAD"])
                .current_dir(&repo)
                .output()
                .unwrap()
                .stdout,
        )
        .unwrap()
        .trim()
        .to_string()
    };

    git(&["init", "-q"]);
    git(&["config", "user.name", "okf-rs e2e tests"]);
    git(&["config", "user.email", "e2e@example.invalid"]);
    git(&["config", "commit.gpgsign", "false"]);

    fs::write(repo.join("src/lib.rs"), "pub fn foo() -> i32 {\n    1\n}\n").unwrap();
    git(&["add", "-A"]);
    git(&["commit", "-q", "-m", "c1"]);
    let c1 = rev_parse_head();

    fs::write(repo.join("src/lib.rs"), "pub fn bar() -> i32 {\n    2\n}\n").unwrap();
    git(&["add", "-A"]);
    git(&["commit", "-q", "-m", "c2"]);
    let c2 = rev_parse_head();

    let ci = run(&repo, &["diff", &c1, &c2, ".", "--ci"]);
    assert!(
        !ci.status.success(),
        "a source-level change must fail --ci\nstdout: {}\nstderr: {}",
        stdout_of(&ci),
        stderr_of(&ci)
    );
    let ci_out = stdout_of(&ci);
    assert!(ci_out.contains("❌ SOURCE CHANGES: 2"), "{ci_out}");
    assert!(ci_out.contains("exit code: 1"), "{ci_out}");

    // No changes between a ref and itself: --ci exits 0.
    let ci_no_op = run(&repo, &["diff", &c2, &c2, ".", "--ci"]);
    assert_success(&ci_no_op, &["diff --ci (no-op)"]);
    let ci_no_op_out = stdout_of(&ci_no_op);
    assert!(
        ci_no_op_out.contains("No changes between"),
        "{ci_no_op_out}"
    );
    assert!(ci_no_op_out.contains("exit code: 0"), "{ci_no_op_out}");
}

/// `--ci` still works end-to-end when the project has an `okf.toml` with
/// a `[diff]` policy — confirms the config-loading path
/// (`okf_core::config::load`) doesn't error or get skipped just because
/// the file exists, even though this fixture can't exercise the
/// resolver/confidence *gating* itself (see the test above for why).
#[test]
fn standalone_binary_diff_ci_loads_a_configured_diff_policy_without_erroring() {
    let workspace = tempfile::tempdir().unwrap();
    let repo = workspace.path().join("diff-ci-config-repo");
    fs::create_dir_all(repo.join("src")).unwrap();

    let git = |args: &[&str]| {
        let status = Command::new("git")
            .args(args)
            .current_dir(&repo)
            .status()
            .expect("failed to run git");
        assert!(status.success(), "git {args:?} failed");
    };
    let rev_parse_head = || {
        String::from_utf8(
            Command::new("git")
                .args(["rev-parse", "HEAD"])
                .current_dir(&repo)
                .output()
                .unwrap()
                .stdout,
        )
        .unwrap()
        .trim()
        .to_string()
    };

    fs::write(
        repo.join("okf.toml"),
        "output = \"knowledge\"\n\n[diff]\nresolver_changes = \"fail\"\nconfidence_changes = \"warn\"\n",
    )
    .unwrap();

    git(&["init", "-q"]);
    git(&["config", "user.name", "okf-rs e2e tests"]);
    git(&["config", "user.email", "e2e@example.invalid"]);
    git(&["config", "commit.gpgsign", "false"]);

    fs::write(repo.join("src/lib.rs"), "pub fn foo() -> i32 {\n    1\n}\n").unwrap();
    git(&["add", "-A"]);
    git(&["commit", "-q", "-m", "c1"]);
    let c1 = rev_parse_head();

    fs::write(
        repo.join("src/lib.rs"),
        "pub fn foo() -> i32 {\n    1\n}\n\npub fn bar() -> i32 {\n    2\n}\n",
    )
    .unwrap();
    git(&["add", "-A"]);
    git(&["commit", "-q", "-m", "c2"]);
    let c2 = rev_parse_head();

    let ci = run(&repo, &["diff", &c1, &c2, ".", "--ci"]);
    assert!(
        !ci.status.success(),
        "a source change fails regardless of the configured resolver/confidence policy"
    );
    assert!(stdout_of(&ci).contains("❌ SOURCE CHANGES: 1"));
}

/// `okf-rs impact` extends `diff`'s concept-level added/removed/changed
/// list with blast radius (transitive callers) and structural
/// criticality. Two commits where only `callee`'s signature changes
/// (adding a parameter) — `caller`'s own signature and relationship set
/// stay identical, so `diff` (and therefore `impact`) must report
/// exactly one changed concept, with `caller` showing up in its blast
/// radius rather than as a change of its own.
#[test]
fn standalone_binary_impact_reports_the_blast_radius_between_two_refs() {
    let workspace = tempfile::tempdir().unwrap();
    let repo = workspace.path().join("impact-repo");
    fs::create_dir_all(repo.join("src")).unwrap();

    let git = |args: &[&str]| {
        let status = Command::new("git")
            .args(args)
            .current_dir(&repo)
            .status()
            .expect("failed to run git");
        assert!(status.success(), "git {args:?} failed");
    };

    git(&["init", "-q"]);
    git(&["config", "user.name", "okf-rs e2e tests"]);
    git(&["config", "user.email", "e2e@example.invalid"]);
    git(&["config", "commit.gpgsign", "false"]);

    fs::write(
        repo.join("src/lib.rs"),
        "pub fn callee() -> i32 {\n    1\n}\n\npub fn caller() -> i32 {\n    callee()\n}\n",
    )
    .unwrap();
    git(&["add", "-A"]);
    git(&["commit", "-q", "-m", "c1"]);
    let c1 = String::from_utf8(
        Command::new("git")
            .args(["rev-parse", "HEAD"])
            .current_dir(&repo)
            .output()
            .unwrap()
            .stdout,
    )
    .unwrap()
    .trim()
    .to_string();

    fs::write(
        repo.join("src/lib.rs"),
        "pub fn callee(x: i32) -> i32 {\n    x + 1\n}\n\npub fn caller() -> i32 {\n    callee(0)\n}\n",
    )
    .unwrap();
    git(&["add", "-A"]);
    git(&["commit", "-q", "-m", "c2"]);
    let c2 = String::from_utf8(
        Command::new("git")
            .args(["rev-parse", "HEAD"])
            .current_dir(&repo)
            .output()
            .unwrap()
            .stdout,
    )
    .unwrap()
    .trim()
    .to_string();

    let impact = run(&repo, &["impact", &c1, &c2, "."]);
    assert_success(&impact, &["impact"]);
    let impact_out = stdout_of(&impact);
    assert!(impact_out.contains("~ functions/src/callee"));
    assert!(impact_out.contains("blast radius: 1 concept(s): functions/src/caller"));

    // No changes between a ref and itself.
    let no_impact = run(&repo, &["impact", &c2, &c2, "."]);
    assert_success(&no_impact, &["impact (no-op)"]);
    assert!(stdout_of(&no_impact).contains("No concept-level changes"));
}

/// `okf-rs review` renders the same underlying impact analysis as
/// `impact`, but as a Markdown, PR-comment-ready report with a leading
/// sticky-comment marker, and supports `--fail-on-risk` for CI gating.
#[test]
fn standalone_binary_review_renders_markdown_and_supports_fail_on_risk_gating() {
    let workspace = tempfile::tempdir().unwrap();
    let repo = workspace.path().join("review-repo");
    fs::create_dir_all(repo.join("src")).unwrap();

    let git = |args: &[&str]| {
        let status = Command::new("git")
            .args(args)
            .current_dir(&repo)
            .status()
            .expect("failed to run git");
        assert!(status.success(), "git {args:?} failed");
    };

    git(&["init", "-q"]);
    git(&["config", "user.name", "okf-rs e2e tests"]);
    git(&["config", "user.email", "e2e@example.invalid"]);
    git(&["config", "commit.gpgsign", "false"]);

    fs::write(
        repo.join("src/lib.rs"),
        "pub fn callee() -> i32 {\n    1\n}\n\npub fn caller() -> i32 {\n    callee()\n}\n",
    )
    .unwrap();
    git(&["add", "-A"]);
    git(&["commit", "-q", "-m", "c1"]);
    let c1 = String::from_utf8(
        Command::new("git")
            .args(["rev-parse", "HEAD"])
            .current_dir(&repo)
            .output()
            .unwrap()
            .stdout,
    )
    .unwrap()
    .trim()
    .to_string();

    fs::write(
        repo.join("src/lib.rs"),
        "pub fn callee(x: i32) -> i32 {\n    x + 1\n}\n\npub fn caller() -> i32 {\n    callee(0)\n}\n",
    )
    .unwrap();
    git(&["add", "-A"]);
    git(&["commit", "-q", "-m", "c2"]);
    let c2 = String::from_utf8(
        Command::new("git")
            .args(["rev-parse", "HEAD"])
            .current_dir(&repo)
            .output()
            .unwrap()
            .stdout,
    )
    .unwrap()
    .trim()
    .to_string();

    let review = run(&repo, &["review", &c1, &c2, "."]);
    assert_success(&review, &["review"]);
    let review_out = stdout_of(&review);
    assert!(review_out.starts_with("<!-- okf-rs-review -->"));
    assert!(review_out.contains("okf-rs impact report:"));
    assert!(review_out.contains("| ✏️ changed | `functions/src/callee` |"));
    assert!(review_out.contains("Deterministic, structural blast-radius analysis"));

    // `--fail-on-risk 1` must fail: `callee`'s blast radius is exactly 1
    // (`caller`).
    let gated_fail = run(&repo, &["review", &c1, &c2, ".", "--fail-on-risk", "1"]);
    assert!(
        !gated_fail.status.success(),
        "expected --fail-on-risk 1 to fail the command"
    );
    assert!(stdout_of(&gated_fail).contains("okf-rs impact report:"));

    // `--fail-on-risk 2` must not fail: nothing has a blast radius that big.
    let gated_pass = run(&repo, &["review", &c1, &c2, ".", "--fail-on-risk", "2"]);
    assert_success(&gated_pass, &["review --fail-on-risk 2"]);

    // No changes between a ref and itself.
    let no_review = run(&repo, &["review", &c2, &c2, "."]);
    assert_success(&no_review, &["review (no-op)"]);
    assert!(stdout_of(&no_review).contains("No concept-level changes"));
}

/// `okf-rs generate --lsp` asks a real language server
/// (`textDocument/definition`) to disambiguate call edges Tree-sitter's
/// own name-only matching can't: two `run` functions in different
/// modules (`a` and `b`), with `a::caller` calling the bare name `run()`.
/// Real Rust scoping makes that genuinely unambiguous — `a` never imports
/// `b::run` — which is exactly what `rust-analyzer` (and a real compiler)
/// resolves it to, but Tree-sitter alone can't tell the two `run`s apart
/// and drops the edge. Skipped, not failed, when `rust-analyzer` isn't
/// installed, matching how `okf-analyzer`'s and `okf-lsp`'s own tests
/// treat this optional, best-effort integration.
#[test]
fn standalone_binary_generate_lsp_disambiguates_a_call_via_rust_analyzer() {
    if !rust_analyzer_available() {
        eprintln!("skipping: rust-analyzer not installed");
        return;
    }

    let workspace = tempfile::tempdir().unwrap();
    let project = workspace.path().join("project");
    fs::create_dir_all(project.join("src")).unwrap();
    fs::write(
        project.join("Cargo.toml"),
        "[package]\nname = \"demo\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    )
    .unwrap();
    fs::write(project.join("src/lib.rs"), "mod a;\nmod b;\n").unwrap();
    fs::write(
        project.join("src/a.rs"),
        "pub fn run() {}\npub fn caller() { run(); }\n",
    )
    .unwrap();
    fs::write(project.join("src/b.rs"), "pub fn run() {}\n").unwrap();

    // Without `--lsp`: two same-named `run` functions make the call
    // genuinely ambiguous by name alone, so Tree-sitter resolution leaves
    // it unresolved rather than guessing.
    let generate = run(&project, &["generate", "."]);
    assert_success(&generate, &["generate"]);
    let callees = run(
        &project,
        &[
            "graph",
            "callees",
            "functions/src/a/caller",
            "--project",
            ".",
        ],
    );
    assert_success(&callees, &["graph callees (pre-lsp)"]);
    assert!(stdout_of(&callees).contains("doesn't call anything"));

    // With `--lsp`: rust-analyzer resolves the same call specifically to
    // `a::run`, never `b::run` — proven by regenerating in place (this
    // also exercises the LSP path alongside the incremental-index cache
    // from the prior run, which caches Tree-sitter extraction only, not
    // resolved relationships) and re-querying the same callee edge.
    let generate_lsp = run(&project, &["generate", ".", "--lsp"]);
    assert_success(&generate_lsp, &["generate --lsp"]);
    let callees_lsp = run(
        &project,
        &[
            "graph",
            "callees",
            "functions/src/a/caller",
            "--project",
            ".",
        ],
    );
    assert_success(&callees_lsp, &["graph callees (post-lsp)"]);
    let callees_lsp_out = stdout_of(&callees_lsp);
    assert!(callees_lsp_out.contains("functions/src/a/run"));
    assert!(!callees_lsp_out.contains("functions/src/b/run"));
}

/// `--check-determinism` runs analysis twice, independently, and diffs
/// the two renders — on the default tree-sitter-only path (no `--lsp`,
/// no external process involved) that should always agree, since the
/// only input is the source text itself. Also confirms it doesn't write
/// a bundle at all: `--output` is never consulted on this path, so
/// `knowledge/` shouldn't exist afterward.
#[test]
fn standalone_binary_check_determinism_reports_deterministic_on_the_tree_sitter_path() {
    let workspace = tempfile::tempdir().unwrap();
    let project = workspace.path().join("project");
    fs::create_dir_all(project.join("src")).unwrap();
    fs::write(
        project.join("Cargo.toml"),
        "[package]\nname = \"demo\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    )
    .unwrap();
    fs::write(
        project.join("src/lib.rs"),
        "pub fn callee() {}\npub fn caller() { callee(); }\n",
    )
    .unwrap();

    let check = run(&project, &["generate", ".", "--check-determinism"]);
    assert_success(&check, &["generate --check-determinism"]);
    let out = stdout_of(&check);
    assert!(out.contains("Deterministic"), "unexpected output: {out}");
    assert!(out.contains("4 concepts"), "unexpected output: {out}");
    assert!(
        !project.join("knowledge").exists(),
        "--check-determinism shouldn't write a bundle to the default output directory"
    );
}

/// `--check-determinism --enrich` is rejected up front, before any
/// analysis or network call — enrichment's own non-determinism (a live
/// endpoint's response isn't guaranteed byte-identical across calls)
/// would otherwise be reported as a false positive unrelated to what
/// this flag actually checks.
#[test]
fn standalone_binary_check_determinism_rejects_enrich() {
    let workspace = tempfile::tempdir().unwrap();
    let project = workspace.path().join("project");
    fs::create_dir_all(project.join("src")).unwrap();
    fs::write(project.join("src/lib.rs"), "pub fn f() {}\n").unwrap();

    let check = run(
        &project,
        &["generate", ".", "--check-determinism", "--enrich"],
    );
    assert!(
        !check.status.success(),
        "expected --check-determinism --enrich to fail"
    );
    let err = stderr_of(&check);
    assert!(
        err.contains("--check-determinism") && err.contains("--enrich"),
        "unexpected stderr: {err}"
    );
}

/// `--check-fresh` verifies the bundle already on disk still matches a
/// fresh `generate` — the "bundle is up to date with source" CI check.
/// Right after a real `generate`, the two must agree.
#[test]
fn standalone_binary_check_fresh_reports_up_to_date_right_after_generate() {
    let workspace = tempfile::tempdir().unwrap();
    let project = workspace.path().join("project");
    fs::create_dir_all(project.join("src")).unwrap();
    fs::write(
        project.join("Cargo.toml"),
        "[package]\nname = \"demo\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    )
    .unwrap();
    fs::write(project.join("src/lib.rs"), "pub fn f() {}\n").unwrap();

    let generate = run(&project, &["generate", "."]);
    assert_success(&generate, &["generate"]);

    let check = run(&project, &["generate", ".", "--check-fresh"]);
    assert_success(&check, &["generate --check-fresh"]);
    let out = stdout_of(&check);
    assert!(out.contains("Up to date"), "unexpected output: {out}");
}

/// Adding a new function after `generate` has already run makes the
/// committed bundle stale — `--check-fresh` must catch that, exit
/// non-zero, and name the file that's out of sync, without silently
/// overwriting `knowledge/` itself.
#[test]
fn standalone_binary_check_fresh_reports_stale_after_a_source_change() {
    let workspace = tempfile::tempdir().unwrap();
    let project = workspace.path().join("project");
    fs::create_dir_all(project.join("src")).unwrap();
    fs::write(
        project.join("Cargo.toml"),
        "[package]\nname = \"demo\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    )
    .unwrap();
    fs::write(project.join("src/lib.rs"), "pub fn f() {}\n").unwrap();

    let generate = run(&project, &["generate", "."]);
    assert_success(&generate, &["generate"]);
    let bundle_before = fs::read_to_string(project.join("knowledge/functions/index.md")).unwrap();

    // Source changes (a new function) without regenerating -- the
    // committed bundle no longer reflects the project.
    fs::write(project.join("src/lib.rs"), "pub fn f() {}\npub fn g() {}\n").unwrap();

    let check = run(&project, &["generate", ".", "--check-fresh"]);
    assert!(
        !check.status.success(),
        "expected --check-fresh to fail on a stale bundle"
    );
    let out = stdout_of(&check);
    assert!(out.contains("Stale"), "unexpected output: {out}");
    assert!(out.contains("g.md"), "unexpected output: {out}");
    // Read-only: the committed bundle itself is untouched, not
    // silently regenerated out from under the CI check.
    let bundle_after = fs::read_to_string(project.join("knowledge/functions/index.md")).unwrap();
    assert_eq!(bundle_before, bundle_after);
}

/// `--check-fresh` needs an existing bundle to compare against -- on a
/// project that's never been generated, it should fail with a clear
/// message pointing at `generate`, not report every file as "stale".
#[test]
fn standalone_binary_check_fresh_fails_clearly_with_no_existing_bundle() {
    let workspace = tempfile::tempdir().unwrap();
    let project = workspace.path().join("project");
    fs::create_dir_all(project.join("src")).unwrap();
    fs::write(project.join("src/lib.rs"), "pub fn f() {}\n").unwrap();

    let check = run(&project, &["generate", ".", "--check-fresh"]);
    assert!(!check.status.success());
    let err = stderr_of(&check);
    assert!(
        err.contains("generate"),
        "expected the error to point at running generate first: {err}"
    );
}

/// `--check-fresh --enrich` is rejected up front, for the same reason as
/// `--check-determinism --enrich`: comparing against a freshly analyzed
/// (unenriched) bundle would flag every AI-written description as
/// spurious drift, not a real staleness signal.
#[test]
fn standalone_binary_check_fresh_rejects_enrich() {
    let workspace = tempfile::tempdir().unwrap();
    let project = workspace.path().join("project");
    fs::create_dir_all(project.join("src")).unwrap();
    fs::write(project.join("src/lib.rs"), "pub fn f() {}\n").unwrap();

    let check = run(&project, &["generate", ".", "--check-fresh", "--enrich"]);
    assert!(
        !check.status.success(),
        "expected --check-fresh --enrich to fail"
    );
    let err = stderr_of(&check);
    assert!(
        err.contains("--check-fresh") && err.contains("--enrich"),
        "unexpected stderr: {err}"
    );
}

/// `--check-determinism` and `--check-fresh` check different things and
/// can't both be requested in the same invocation.
#[test]
fn standalone_binary_rejects_check_determinism_and_check_fresh_together() {
    let workspace = tempfile::tempdir().unwrap();
    let project = workspace.path().join("project");
    fs::create_dir_all(project.join("src")).unwrap();
    fs::write(project.join("src/lib.rs"), "pub fn f() {}\n").unwrap();

    let check = run(
        &project,
        &["generate", ".", "--check-determinism", "--check-fresh"],
    );
    assert!(!check.status.success());
    let err = stderr_of(&check);
    assert!(
        err.contains("--check-determinism") && err.contains("--check-fresh"),
        "unexpected stderr: {err}"
    );
}

/// `okf-rs watch` regenerates the bundle once immediately on startup and
/// then blocks watching for filesystem changes. This test only exercises
/// that baseline regenerate — it starts the real subprocess, waits for
/// its startup + first-regenerate lines (bounded by a timeout so a
/// regression that hangs the command fails the test instead of the test
/// suite itself), then kills it.
#[test]
fn standalone_binary_watch_regenerates_the_bundle_on_startup() {
    let workspace = tempfile::tempdir().unwrap();
    let project = workspace.path().join("project");
    copy_repo_into(&project);

    let mut child = Command::new(bin_path())
        .args(["watch", "."])
        .current_dir(&project)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("failed to spawn `okf-rs watch`");

    let stdout = child.stdout.take().expect("child stdout was not piped");
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        for line in BufReader::new(stdout).lines() {
            match line {
                Ok(line) => {
                    if tx.send(line).is_err() {
                        break;
                    }
                }
                Err(_) => break,
            }
        }
    });

    let mut lines = Vec::new();
    for _ in 0..2 {
        match rx.recv_timeout(Duration::from_secs(30)) {
            Ok(line) => lines.push(line),
            Err(_) => break,
        }
    }

    let _ = child.kill();
    let _ = child.wait();

    assert!(
        lines.iter().any(|l| l.contains("Watching")),
        "expected a startup message, got: {lines:?}"
    );
    assert!(
        lines
            .iter()
            .any(|l| l.contains("Regenerated") && l.contains("concepts")),
        "expected the baseline regenerate to be reported, got: {lines:?}"
    );
    assert!(project.join("knowledge/index.md").exists());
}

/// A minimal, single-endpoint OpenAI-compatible mock server: reads one
/// HTTP/1.1 request off each accepted connection, ignores everything
/// about it except that it arrived, and replies with a canned
/// `chat.completions`-shaped JSON body — enough for `okf-rs generate
/// --enrich` to point its `--enrich-base-url` at, without a real network
/// call or a real LLM in the test environment.
/// Reads one HTTP/1.1 request off `stream` far enough to get its body
/// (headers up to the blank line, then exactly `Content-Length` bytes)
/// and discards it — shared by every mock OpenAI-compatible endpoint
/// below, which only differ in what JSON they reply with.
fn read_http_request_body(stream: &TcpStream) {
    let mut reader = BufReader::new(stream.try_clone().unwrap());
    let mut content_length = 0usize;
    loop {
        let mut line = String::new();
        if reader.read_line(&mut line).unwrap_or(0) == 0 {
            break;
        }
        if line.trim_end().is_empty() {
            break;
        }
        if let Some(value) = line.trim_end().strip_prefix("Content-Length: ") {
            content_length = value.trim().parse().unwrap_or(0);
        }
    }
    let mut body = vec![0u8; content_length];
    let _ = reader.read_exact(&mut body);
}

/// Writes a well-formed `200 OK` HTTP/1.1 response carrying `payload` as
/// its JSON body.
fn write_json_response(mut stream: &TcpStream, payload: &str) {
    let response = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
        payload.len(),
        payload
    );
    let _ = stream.write_all(response.as_bytes());
}

fn start_mock_enrich_server(reply_content: &'static str) -> (String, Arc<AtomicUsize>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let request_count = Arc::new(AtomicUsize::new(0));
    let counter = Arc::clone(&request_count);

    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(stream) = stream else { break };
            counter.fetch_add(1, Ordering::SeqCst);
            read_http_request_body(&stream);
            let payload =
                format!(r#"{{"choices":[{{"message":{{"content":"{reply_content}"}}}}]}}"#);
            write_json_response(&stream, &payload);
        }
    });

    (format!("http://127.0.0.1:{port}/v1"), request_count)
}

/// A mock `/embeddings` endpoint replying with the same fixed vector for
/// every request, regardless of `input` — enough for an end-to-end smoke
/// test of the CLI's `search --semantic` wiring (real ranking behavior
/// against varying inputs is already covered by `okf-enrich`/`okf-query`'s
/// own unit tests, which use a keyed mock).
fn start_mock_embedding_server() -> (String, Arc<AtomicUsize>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let request_count = Arc::new(AtomicUsize::new(0));
    let counter = Arc::clone(&request_count);

    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(stream) = stream else { break };
            counter.fetch_add(1, Ordering::SeqCst);
            read_http_request_body(&stream);
            write_json_response(&stream, r#"{"data":[{"embedding":[1.0,0.0,0.0]}]}"#);
        }
    });

    (format!("http://127.0.0.1:{port}/v1"), request_count)
}

/// `okf-rs generate --enrich` fills in missing function/module
/// descriptions via an OpenAI-compatible endpoint, and a second run
/// against the same output reuses them from the bundle already on disk
/// instead of re-querying the (mock) endpoint.
#[test]
fn standalone_binary_generate_enrich_fills_descriptions_and_a_second_run_reuses_them() {
    let workspace = tempfile::tempdir().unwrap();
    let project = workspace.path().join("project");
    fs::create_dir_all(project.join("src")).unwrap();
    fs::write(
        project.join("src/lib.rs"),
        "pub fn undocumented() -> i32 {\n    1\n}\n",
    )
    .unwrap();

    let (base_url, request_count) = start_mock_enrich_server("a generated description");
    let enrich_args = [
        "generate",
        ".",
        "--enrich",
        "--enrich-base-url",
        &base_url,
        "--enrich-model",
        "test-model",
    ];

    // The tiny fixture above has two enrichable concepts: the `src/lib.rs`
    // Module, and the `undocumented` Function (no manifest, so no Package).
    let first = run(&project, &enrich_args);
    assert_success(&first, &["generate --enrich"]);
    let first_out = stdout_of(&first);
    assert!(first_out.contains("Enriched 2 concepts (2 generated, 0 reused"));
    assert_eq!(request_count.load(Ordering::SeqCst), 2);

    let bundle_file = project.join("knowledge/functions/src/undocumented.md");
    let content = fs::read_to_string(&bundle_file).unwrap();
    assert!(content.contains("description: a generated description"));

    // A second run: the concepts still have no doc comment in source, but
    // the bundle written above already carries a description for both,
    // so they must be reused rather than triggering another mock-server
    // call.
    let second = run(&project, &enrich_args);
    assert_success(&second, &["generate --enrich (second run)"]);
    let second_out = stdout_of(&second);
    assert!(second_out.contains("Enriched 2 concepts (0 generated, 2 reused"));
    assert_eq!(
        request_count.load(Ordering::SeqCst),
        2,
        "the second run should not have queried the enrichment endpoint again"
    );

    let validate = run(&project, &["validate", "--project", "."]);
    assert_success(&validate, &["validate"]);
    assert!(stdout_of(&validate).contains("no issues found"));
}

/// `--enrich` without an endpoint configured (no `--enrich-base-url`/
/// `--enrich-model`, no matching env var) is a clear, non-zero-exit
/// error naming both the flag and its env var fallback — not a panic or
/// an opaque network error from a client that was never configured.
#[test]
fn standalone_binary_generate_enrich_without_an_endpoint_is_a_clear_error() {
    let workspace = tempfile::tempdir().unwrap();
    let project = workspace.path();
    fs::write(project.join("lib.rs"), "pub fn f() {}\n").unwrap();

    let result = Command::new(bin_path())
        .args(["generate", ".", "--enrich"])
        .current_dir(project)
        .env_remove("OKF_ENRICH_BASE_URL")
        .env_remove("OKF_ENRICH_MODEL")
        .output()
        .unwrap();
    assert!(!result.status.success());
    let stderr = stderr_of(&result);
    assert!(stderr.contains("--enrich-base-url"));
    assert!(stderr.contains("OKF_ENRICH_BASE_URL"));
}

/// A late `--enrich` failure (the endpoint is configured but unreachable,
/// so it fails partway through — after analysis, and potentially after
/// enriching some concepts already) must not discard the analysis work
/// that already happened: the bundle is still written to disk from
/// whatever `result.concepts` ended up with, and only the exit code and
/// stderr report the enrichment failure.
#[test]
fn standalone_binary_generate_enrich_endpoint_failure_still_writes_the_bundle() {
    let workspace = tempfile::tempdir().unwrap();
    let project = workspace.path().join("project");
    fs::create_dir_all(project.join("src")).unwrap();
    fs::write(
        project.join("src/lib.rs"),
        "pub fn undocumented() -> i32 {\n    1\n}\n",
    )
    .unwrap();

    // A `127.0.0.1` port nothing is listening on: connection refused, the
    // same shape of failure a real unreachable/misconfigured endpoint
    // would produce.
    let result = Command::new(bin_path())
        .args([
            "generate",
            ".",
            "--enrich",
            "--enrich-base-url",
            "http://127.0.0.1:1/v1",
            "--enrich-model",
            "test-model",
        ])
        .current_dir(&project)
        .output()
        .unwrap();

    assert!(!result.status.success());

    let bundle_file = project.join("knowledge/functions/src/undocumented.md");
    assert!(
        bundle_file.exists(),
        "the analysis result must still be written even though enrichment failed"
    );
    let content = fs::read_to_string(&bundle_file).unwrap();
    assert!(!content.contains("description:"));

    assert!(project.join(".okf-cache.json").exists());
}

/// `okf-rs suggest-links` builds on `--enrich`: after descriptions exist
/// (via `generate --enrich`), it finds full-text-search candidates with
/// no existing relationship and asks the (mock) endpoint whether each
/// looks like a genuinely missing link.
#[test]
fn standalone_binary_suggest_links_reports_a_suggestion_from_the_endpoint() {
    let workspace = tempfile::tempdir().unwrap();
    let project = workspace.path().join("project");
    fs::create_dir_all(project.join("src")).unwrap();
    fs::write(
        project.join("src/lib.rs"),
        "pub fn verify_token() -> bool {\n    true\n}\n\npub fn decode_jwt() -> i32 {\n    0\n}\n",
    )
    .unwrap();

    // Step 1: populate descriptions so there's something for full-text
    // search to match candidates on.
    let (describe_url, _) = start_mock_enrich_server("handles authentication tokens");
    let generate = run(
        &project,
        &[
            "generate",
            ".",
            "--enrich",
            "--enrich-base-url",
            &describe_url,
            "--enrich-model",
            "test-model",
        ],
    );
    assert_success(&generate, &["generate --enrich"]);

    // Step 2: suggest-links, against a second mock endpoint that always
    // says the candidate pair looks related.
    let (suggest_url, suggest_requests) = start_mock_enrich_server("these likely belong together");
    let suggest = run(
        &project,
        &[
            "suggest-links",
            "--project",
            ".",
            "--enrich-base-url",
            &suggest_url,
            "--enrich-model",
            "test-model",
        ],
    );
    assert_success(&suggest, &["suggest-links"]);
    let out = stdout_of(&suggest);
    assert!(out.contains("these likely belong together"));
    assert!(
        suggest_requests.load(Ordering::SeqCst) > 0,
        "expected at least one candidate to be judged"
    );
}

/// `okf-rs search --semantic` ranks concepts by embedding-cosine
/// similarity instead of lexical/exact matching. End-to-end smoke test
/// against a mock `/embeddings` endpoint: only a described concept is
/// ever embedded and returned, and an undescribed concept never shows up
/// (real ranking-order behavior is covered by `okf-enrich`/`okf-query`'s
/// own unit tests against a keyed mock). Descriptions are hand-written
/// directly into the generated bundle (rather than via `--enrich`)
/// specifically so only one of the two concepts ends up described.
#[test]
fn standalone_binary_search_semantic_ranks_by_embedding_similarity() {
    let workspace = tempfile::tempdir().unwrap();
    let project = workspace.path().join("project");
    fs::create_dir_all(project.join("src")).unwrap();
    fs::write(
        project.join("src/lib.rs"),
        "pub fn verify_token() -> bool {\n    true\n}\n\npub fn undescribed() -> i32 {\n    0\n}\n",
    )
    .unwrap();

    let generate = run(&project, &["generate", "."]);
    assert_success(&generate, &["generate"]);

    let verify_token_path = project.join("knowledge/functions/src/verify_token.md");
    let content = fs::read_to_string(&verify_token_path).unwrap();
    let with_description = content.replacen(
        "---\n",
        "---\ndescription: Verifies a signed authentication token.\n",
        1,
    );
    fs::write(&verify_token_path, with_description).unwrap();

    let (embed_url, embed_requests) = start_mock_embedding_server();
    let search = run(
        &project,
        &[
            "search",
            "authentication",
            "--project",
            ".",
            "--semantic",
            "--enrich-base-url",
            &embed_url,
            "--enrich-model",
            "test-embedding-model",
        ],
    );
    assert_success(&search, &["search --semantic"]);
    let out = stdout_of(&search);
    assert!(out.contains("verify_token"));
    assert!(
        !out.contains("undescribed"),
        "an undescribed concept must never be embedded or returned: {out}"
    );
    assert!(
        embed_requests.load(Ordering::SeqCst) > 0,
        "expected at least one embedding request"
    );

    // `--ranked` and `--semantic` are mutually exclusive.
    let conflict = run(
        &project,
        &["search", "x", "--project", ".", "--ranked", "--semantic"],
    );
    assert!(!conflict.status.success());
}

/// Commands that read an existing bundle fail with a clear, non-zero-exit
/// error (rather than a panic or opaque I/O error) when `generate` hasn't
/// been run yet.
#[test]
fn standalone_binary_reports_a_clear_error_when_no_bundle_exists_yet() {
    let workspace = tempfile::tempdir().unwrap();
    let project = workspace.path();
    fs::write(project.join("src.rs"), "pub fn f() {}\n").unwrap();

    let search = run(project, &["search", "anything", "--project", "."]);
    assert!(!search.status.success());
    assert!(stderr_of(&search).contains("okf-rs generate"));

    let validate = run(project, &["validate", "--project", "."]);
    assert!(!validate.status.success());

    let callers = run(
        project,
        &["graph", "callers", "functions/anything", "--project", "."],
    );
    assert!(!callers.status.success());
    assert!(stderr_of(&callers).contains("okf-rs generate"));
}
