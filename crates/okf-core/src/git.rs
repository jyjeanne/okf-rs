//! Minimal, best-effort git helpers shared by every caller that wants to
//! stamp an artifact with *which commit produced it* — currently just
//! [`head_revision`], used by `okf-generator`'s bundle-root reproducibility
//! metadata (see `ROADMAP.md`'s "Artifact-level reproducibility metadata"
//! item). Deliberately separate from `okf-cli`'s own git shell-outs
//! (`WorktreeCheckout`, `git_repo_root`): those exist to check out and
//! compare *other* refs for `diff`/`impact`/`review`, a materially
//! different job from "what commit is the working tree at right now,"
//! and living here lets both `okf-cli` and `okf-watch` share one
//! implementation instead of each shelling out to `git` independently.

use std::path::Path;
use std::process::{Command, Stdio};

/// The commit `HEAD` currently points to at `project_root`, or `None` if
/// `project_root` isn't inside a git repository (or `git` itself isn't
/// available) — never an error, since reproducibility metadata is
/// best-effort: its absence means "unknown," not "invalid," the same
/// posture every other optional provenance field in this project already
/// takes. Says nothing about whether the working tree is clean; the
/// commit is still meaningful (and still recorded) even with local
/// modifications on top of it — see `ROADMAP.md` for why this doesn't
/// try to detect or gate on that.
pub fn head_revision(project_root: &Path) -> Option<String> {
    let output = Command::new("git")
        .args(["rev-parse", "HEAD"])
        .current_dir(project_root)
        .stderr(Stdio::null())
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let revision = String::from_utf8(output.stdout).ok()?;
    let revision = revision.trim();
    if revision.is_empty() {
        None
    } else {
        Some(revision.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command as StdCommand;

    fn git(dir: &Path, args: &[&str]) {
        let status = StdCommand::new("git")
            .args(args)
            .current_dir(dir)
            .status()
            .expect("failed to run git");
        assert!(status.success(), "git {args:?} failed");
    }

    #[test]
    fn none_outside_a_git_repository() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(head_revision(dir.path()), None);
    }

    #[test]
    fn some_the_current_head_sha_inside_a_real_repository() {
        let dir = tempfile::tempdir().unwrap();
        git(dir.path(), &["init", "-q"]);
        git(dir.path(), &["config", "user.name", "okf-rs tests"]);
        git(
            dir.path(),
            &["config", "user.email", "tests@example.invalid"],
        );
        git(dir.path(), &["config", "commit.gpgsign", "false"]);
        std::fs::write(dir.path().join("f"), "x").unwrap();
        git(dir.path(), &["add", "-A"]);
        git(dir.path(), &["commit", "-q", "-m", "c1"]);

        let expected = StdCommand::new("git")
            .args(["rev-parse", "HEAD"])
            .current_dir(dir.path())
            .output()
            .unwrap()
            .stdout;
        let expected = String::from_utf8(expected).unwrap().trim().to_string();

        assert_eq!(head_revision(dir.path()), Some(expected));
    }

    #[test]
    fn none_in_a_freshly_initialized_repo_with_no_commits_yet() {
        let dir = tempfile::tempdir().unwrap();
        git(dir.path(), &["init", "-q"]);
        assert_eq!(head_revision(dir.path()), None);
    }

    #[test]
    fn still_returns_the_head_sha_when_the_working_tree_is_dirty() {
        // The commit is still meaningful with local modifications on top
        // of it -- see this module's own docs for why this deliberately
        // doesn't try to detect or gate on dirtiness.
        let dir = tempfile::tempdir().unwrap();
        git(dir.path(), &["init", "-q"]);
        git(dir.path(), &["config", "user.name", "okf-rs tests"]);
        git(
            dir.path(),
            &["config", "user.email", "tests@example.invalid"],
        );
        git(dir.path(), &["config", "commit.gpgsign", "false"]);
        std::fs::write(dir.path().join("f"), "x").unwrap();
        git(dir.path(), &["add", "-A"]);
        git(dir.path(), &["commit", "-q", "-m", "c1"]);

        // Dirty the working tree without committing.
        std::fs::write(dir.path().join("f"), "y").unwrap();

        assert!(head_revision(dir.path()).is_some());
    }
}
