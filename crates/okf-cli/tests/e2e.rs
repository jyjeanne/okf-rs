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
use std::net::TcpListener;
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
fn start_mock_enrich_server() -> (String, Arc<AtomicUsize>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let request_count = Arc::new(AtomicUsize::new(0));
    let counter = Arc::clone(&request_count);

    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { break };
            counter.fetch_add(1, Ordering::SeqCst);
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

            let payload = r#"{"choices":[{"message":{"content":"a generated description"}}]}"#;
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                payload.len(),
                payload
            );
            let _ = stream.write_all(response.as_bytes());
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

    let (base_url, request_count) = start_mock_enrich_server();
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
