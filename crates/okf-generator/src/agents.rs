//! Generates/updates AI agent entry-point files (`CLAUDE.md`, `AGENTS.md`,
//! `.github/copilot-instructions.md`) that point at the OKF bundle, per
//! the specification's [AI Agent
//! Compatibility](../../../docs/specification.md#ai-agent-compatibility)
//! section.
//!
//! These files commonly already exist with project-specific content
//! before `okf-rs` ever runs, so writing them is idempotent and
//! non-destructive: the okf-rs section is delimited by marker comments,
//! and only the content between those markers is ever touched. A file
//! with no markers gets the section appended, not overwritten.
//!
//! `AGENTS.md` is the source of truth for the okf-rs section; `CLAUDE.md`
//! gets a one-line `@AGENTS.md` import instead of a duplicate copy. This
//! matches Claude Code's own documented multi-tool convention, and avoids
//! a real compatibility trap: OpenCode (and other AGENTS.md-first agents)
//! use *only* AGENTS.md when both files are present, so a duplicated
//! CLAUDE.md would silently go unread by them the moment an AGENTS.md
//! exists — including any project-specific content a user wrote into
//! CLAUDE.md outside the okf-rs markers.

use anyhow::{Context, Result};
use std::fs;
use std::path::{Path, PathBuf};

const BEGIN_MARKER: &str = "<!-- okf-rs:begin -->";
const END_MARKER: &str = "<!-- okf-rs:end -->";

const TARGETS: [&str; 3] = ["CLAUDE.md", "AGENTS.md", ".github/copilot-instructions.md"];

fn render_section(bundle_output: &str) -> String {
    format!(
        "{BEGIN_MARKER}\n\
## Knowledge base

This project's structural knowledge — modules, types, functions, and their call graph — is available as an [OKF](https://cloud.google.com/blog/products/data-analytics/how-the-open-knowledge-format-can-improve-data-sharing) bundle in `{bundle_output}/`. It's plain markdown with YAML frontmatter; browse `{bundle_output}/index.md` for an overview, or query it with the CLI:

- `okf-rs search <query>` — find a symbol, type, or module by name or tag
- `okf-rs graph callers <id>` / `okf-rs graph callees <id>` — trace the call graph from a concept id (ids are shown by `search`)
- `okf-rs graph api` — list the public API surface
- `okf-rs graph cycles` — list call-graph cycles

Regenerate the bundle after code changes with `okf-rs generate`.
{END_MARKER}\n"
    )
}

/// The section written to `CLAUDE.md`: an `@AGENTS.md` import rather than
/// a duplicate of `render_section`'s content, so `AGENTS.md` stays the
/// single source of truth.
fn render_import_section() -> String {
    format!("{BEGIN_MARKER}\n@AGENTS.md\n{END_MARKER}\n")
}

/// Replaces the marked section in `existing` with `section`, or appends
/// `section` if no markers are present, or returns `section` as-is if
/// there's no existing content.
fn merge(existing: Option<String>, section: &str) -> String {
    let Some(content) = existing else {
        return section.to_string();
    };
    match (content.find(BEGIN_MARKER), content.find(END_MARKER)) {
        (Some(start), Some(end)) if end > start => {
            let end = end + END_MARKER.len();
            // `section` already supplies its own trailing newline, so
            // drop the one immediately following the old end marker to
            // avoid growing a blank line on every re-run.
            let remainder = content[end..].strip_prefix('\n').unwrap_or(&content[end..]);
            format!("{}{}{}", &content[..start], section, remainder)
        }
        _ => {
            let mut merged = content;
            if !merged.ends_with('\n') {
                merged.push('\n');
            }
            if !merged.ends_with("\n\n") {
                merged.push('\n');
            }
            merged.push_str(section);
            merged
        }
    }
}

/// Writes/updates `CLAUDE.md`, `AGENTS.md`, and
/// `.github/copilot-instructions.md` under `project_root`, pointing at
/// `bundle_output` (typically the same relative path recorded in
/// `okf.toml`, e.g. `knowledge`). Returns the paths written.
pub fn write_agent_entrypoints(project_root: &Path, bundle_output: &str) -> Result<Vec<PathBuf>> {
    let section = render_section(bundle_output);
    let import_section = render_import_section();
    let mut written = Vec::new();
    for relative in TARGETS {
        let section = if relative == "CLAUDE.md" {
            &import_section
        } else {
            &section
        };
        let target = project_root.join(relative);
        if let Some(parent) = target.parent() {
            fs::create_dir_all(parent)
                .with_context(|| format!("failed to create {}", parent.display()))?;
        }
        let existing = fs::read_to_string(&target).ok();
        let merged = merge(existing, section);
        fs::write(&target, merged)
            .with_context(|| format!("failed to write {}", target.display()))?;
        written.push(target);
    }
    Ok(written)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn creates_new_files_with_the_section() {
        let dir = tempfile::tempdir().unwrap();
        let written = write_agent_entrypoints(dir.path(), "knowledge").unwrap();
        assert_eq!(written.len(), 3);

        // CLAUDE.md imports AGENTS.md rather than duplicating its
        // content, so it stays readable by tools (e.g. OpenCode) that
        // ignore CLAUDE.md outright whenever an AGENTS.md is present.
        let claude = fs::read_to_string(dir.path().join("CLAUDE.md")).unwrap();
        assert!(claude.contains(BEGIN_MARKER));
        assert!(claude.contains("@AGENTS.md"));
        assert!(!claude.contains("knowledge/index.md"));

        let agents = fs::read_to_string(dir.path().join("AGENTS.md")).unwrap();
        assert!(agents.contains(BEGIN_MARKER));
        assert!(agents.contains("knowledge/index.md"));

        let copilot =
            fs::read_to_string(dir.path().join(".github/copilot-instructions.md")).unwrap();
        assert!(copilot.contains("knowledge/index.md"));
    }

    #[test]
    fn preserves_existing_content_and_updates_marked_section_in_place() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(
            dir.path().join("CLAUDE.md"),
            "# My Project\n\nSome existing instructions the user wrote.\n",
        )
        .unwrap();

        write_agent_entrypoints(dir.path(), "knowledge").unwrap();
        let first = fs::read_to_string(dir.path().join("CLAUDE.md")).unwrap();
        assert!(first.contains("Some existing instructions the user wrote."));
        assert!(first.contains(BEGIN_MARKER));
        assert!(first.contains("@AGENTS.md"));

        // Running it again with a different bundle path updates AGENTS.md's
        // marked section (CLAUDE.md's import line is stable either way),
        // preserves the user's own CLAUDE.md content, and doesn't
        // duplicate the section.
        write_agent_entrypoints(dir.path(), "docs/kb").unwrap();
        let second = fs::read_to_string(dir.path().join("CLAUDE.md")).unwrap();
        assert!(second.contains("Some existing instructions the user wrote."));
        assert_eq!(second.matches(BEGIN_MARKER).count(), 1);

        let agents = fs::read_to_string(dir.path().join("AGENTS.md")).unwrap();
        assert!(agents.contains("docs/kb/index.md"));
        assert!(!agents.contains("knowledge/index.md"));
    }

    #[test]
    fn repeat_runs_do_not_grow_a_trailing_blank_line() {
        let dir = tempfile::tempdir().unwrap();
        write_agent_entrypoints(dir.path(), "knowledge").unwrap();
        let once = fs::read_to_string(dir.path().join("AGENTS.md")).unwrap();

        for _ in 0..3 {
            write_agent_entrypoints(dir.path(), "knowledge").unwrap();
        }
        let repeated = fs::read_to_string(dir.path().join("AGENTS.md")).unwrap();
        assert_eq!(once, repeated);
    }

    #[test]
    fn claude_md_import_does_not_duplicate_on_repeat_runs() {
        let dir = tempfile::tempdir().unwrap();
        write_agent_entrypoints(dir.path(), "knowledge").unwrap();
        write_agent_entrypoints(dir.path(), "knowledge").unwrap();

        let claude = fs::read_to_string(dir.path().join("CLAUDE.md")).unwrap();
        assert_eq!(claude.matches("@AGENTS.md").count(), 1);
        assert_eq!(claude.matches(BEGIN_MARKER).count(), 1);
    }
}
