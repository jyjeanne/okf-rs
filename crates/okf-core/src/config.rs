use anyhow::{Context, Result};
use std::fs;
use std::path::{Path, PathBuf};

/// Minimal per-project config written by `okf-rs init` and read by the
/// other commands so `--output`/bundle paths don't need repeating on every
/// invocation. `output` is relative to the project root it was recorded
/// for — see [`load`] for how callers should resolve it. Anything not
/// found here (or no `okf.toml` at all) falls back to the literal default
/// `knowledge`.
pub struct Config {
    pub output: PathBuf,
    /// `okf-rs diff --ci`'s exit-code policy — see [`DiffPolicy`]. Absent
    /// entirely from most `okf.toml` files (it's optional, with sane
    /// defaults); only present once a project has actually opted into a
    /// non-default `[diff]` policy.
    pub diff: DiffPolicy,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            output: PathBuf::from("knowledge"),
            diff: DiffPolicy::default(),
        }
    }
}

/// `okf-rs diff --ci`'s configurable policy: how a resolver-level or
/// confidence-level relationship change (see
/// `okf_analyzer::RelationshipChangeKind`) should affect the exit code.
/// A source-level change (a concept added/removed, a signature change, or
/// a relationship whose target/kind itself differs) is **never**
/// configurable here — it always fails `--ci`, matching every other
/// `--ci`/`--check-*` flag's "warnings are real problems by default"
/// posture. Read from `okf.toml`'s `[diff]` table (see [`load`]);
/// defaults apply to any key that table omits, or if the table itself is
/// absent.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DiffPolicy {
    /// Default [`PolicyAction::Warn`]: a resolver identity/version change
    /// (`ResolverChange`) or a combined resolver-and-confidence change
    /// (`ProvenanceChange`) is reported but doesn't fail `--ci` on its
    /// own — the tool that produced an edge changed, not necessarily the
    /// edge itself.
    pub resolver_changes: PolicyAction,
    /// Default [`PolicyAction::Ignore`]: a confidence-only change
    /// (`ConfidenceChange`) is the least consequential category — same
    /// resolver, same target, just a different confidence level — so it's
    /// silent by default under `--ci` unless a project opts in.
    pub confidence_changes: PolicyAction,
}

impl Default for DiffPolicy {
    fn default() -> Self {
        DiffPolicy {
            resolver_changes: PolicyAction::Warn,
            confidence_changes: PolicyAction::Ignore,
        }
    }
}

/// One `[diff]` policy setting's effect on `okf-rs diff --ci`'s exit code
/// and on how that category is rendered (`❌`/`⚠️`/`ℹ️`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PolicyAction {
    /// Reported with a count, never fails the exit code.
    Warn,
    /// Reported with a count, fails the exit code — the same severity a
    /// source-level change always has.
    Fail,
    /// Reported with a count (still visible — `--ci` never hides a real
    /// difference, it only decides whether that difference fails the
    /// build), never fails the exit code.
    Ignore,
}

impl PolicyAction {
    fn parse(s: &str) -> Option<Self> {
        match s {
            "warn" => Some(PolicyAction::Warn),
            "fail" => Some(PolicyAction::Fail),
            "ignore" => Some(PolicyAction::Ignore),
            _ => None,
        }
    }
}

const CONFIG_FILE: &str = "okf.toml";

/// Loads `<project_root>/okf.toml`, if any. A missing file is the normal,
/// silent case (most projects haven't run `init`) and falls back to
/// [`Config::default`]. A file that exists but fails to read or parse is
/// different — that's a real configuration error, not an absent one — so
/// it's reported on stderr before falling back, rather than silently
/// masking it (a bad `okf.toml` would otherwise make every later command
/// quietly target the wrong bundle with no indication why).
///
/// The returned `output` is still relative to `project_root`; join it
/// yourself (`project_root.join(config.output)`) before using it, since
/// this function has no way to know whether the caller's current
/// directory is `project_root`.
pub fn load(project_root: &Path) -> Config {
    let path = project_root.join(CONFIG_FILE);
    let content = match fs::read_to_string(&path) {
        Ok(content) => content,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Config::default(),
        Err(e) => {
            eprintln!(
                "warning: failed to read {}: {e}; using defaults",
                path.display()
            );
            return Config::default();
        }
    };
    let value = match content.parse::<toml::Value>() {
        Ok(value) => value,
        Err(e) => {
            eprintln!(
                "warning: failed to parse {}: {e}; using defaults",
                path.display()
            );
            return Config::default();
        }
    };
    let output = match value.get("output").and_then(|v| v.as_str()) {
        Some(output) => PathBuf::from(output),
        None => {
            eprintln!(
                "warning: {} has no `output` key; using default \"knowledge\"",
                path.display()
            );
            Config::default().output
        }
    };
    let diff = parse_diff_policy(&path, value.get("diff"));
    Config { output, diff }
}

/// Parses an `okf.toml`'s optional `[diff]` table into a [`DiffPolicy`],
/// starting from [`DiffPolicy::default`] and overriding only the keys
/// actually present. An unrecognized action value (anything but
/// `"warn"`/`"fail"`/`"ignore"`) warns and keeps that one key's default,
/// the same "a bad value doesn't invalidate everything else" posture
/// [`load`] itself already takes toward a missing `output` key.
fn parse_diff_policy(config_path: &Path, table: Option<&toml::Value>) -> DiffPolicy {
    let mut policy = DiffPolicy::default();
    let Some(table) = table else {
        return policy;
    };

    if let Some(raw) = table.get("resolver_changes").and_then(|v| v.as_str()) {
        match PolicyAction::parse(raw) {
            Some(action) => policy.resolver_changes = action,
            None => eprintln!(
                "warning: {} has an unrecognized `diff.resolver_changes` value `{raw}` \
                 (expected \"warn\", \"fail\", or \"ignore\"); using the default \"warn\"",
                config_path.display()
            ),
        }
    }
    if let Some(raw) = table.get("confidence_changes").and_then(|v| v.as_str()) {
        match PolicyAction::parse(raw) {
            Some(action) => policy.confidence_changes = action,
            None => eprintln!(
                "warning: {} has an unrecognized `diff.confidence_changes` value `{raw}` \
                 (expected \"warn\", \"fail\", or \"ignore\"); using the default \"ignore\"",
                config_path.display()
            ),
        }
    }
    policy
}

pub fn write_default(project_root: &Path, output: &Path) -> Result<PathBuf> {
    let path = project_root.join(CONFIG_FILE);
    let content = format!(
        "# okf-rs project configuration\noutput = \"{}\"\n",
        output.display()
    );
    fs::write(&path, content).with_context(|| format!("failed to write {}", path.display()))?;
    Ok(path)
}

/// Resolves a bundle directory the way every okf-rs command does: an
/// explicit path always wins (used as-is, relative to the caller's
/// current directory — standard CLI convention); otherwise falls back to
/// `okf.toml`'s `output`, joined against `project_root` — not the current
/// directory — since that's what it was recorded against.
pub fn resolve_bundle(project_root: &Path, explicit: Option<PathBuf>) -> PathBuf {
    explicit.unwrap_or_else(|| project_root.join(load(project_root).output))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(dir: &Path, content: &str) {
        fs::write(dir.join(CONFIG_FILE), content).unwrap();
    }

    #[test]
    fn missing_config_file_is_every_default() {
        let dir = tempfile::tempdir().unwrap();
        let config = load(dir.path());
        assert_eq!(config.output, PathBuf::from("knowledge"));
        assert_eq!(config.diff, DiffPolicy::default());
    }

    #[test]
    fn a_diff_table_absent_from_an_otherwise_valid_config_uses_default_policy() {
        let dir = tempfile::tempdir().unwrap();
        write(dir.path(), "output = \"kb\"\n");
        let config = load(dir.path());
        assert_eq!(config.output, PathBuf::from("kb"));
        assert_eq!(config.diff, DiffPolicy::default());
    }

    #[test]
    fn reads_a_fully_specified_diff_policy() {
        let dir = tempfile::tempdir().unwrap();
        write(
            dir.path(),
            "output = \"kb\"\n\n[diff]\nresolver_changes = \"fail\"\nconfidence_changes = \"warn\"\n",
        );
        let config = load(dir.path());
        assert_eq!(config.diff.resolver_changes, PolicyAction::Fail);
        assert_eq!(config.diff.confidence_changes, PolicyAction::Warn);
    }

    #[test]
    fn a_partially_specified_diff_table_only_overrides_the_keys_present() {
        let dir = tempfile::tempdir().unwrap();
        write(
            dir.path(),
            "output = \"kb\"\n\n[diff]\nresolver_changes = \"ignore\"\n",
        );
        let config = load(dir.path());
        assert_eq!(config.diff.resolver_changes, PolicyAction::Ignore);
        // Not mentioned in the table -- stays at the default, not
        // clobbered to some zero-value.
        assert_eq!(config.diff.confidence_changes, PolicyAction::Ignore);
    }

    #[test]
    fn an_unrecognized_policy_value_falls_back_to_the_default_for_that_key_only() {
        let dir = tempfile::tempdir().unwrap();
        write(
            dir.path(),
            "output = \"kb\"\n\n[diff]\nresolver_changes = \"maybe\"\nconfidence_changes = \"fail\"\n",
        );
        let config = load(dir.path());
        // The bad value doesn't take down the whole table.
        assert_eq!(config.diff.resolver_changes, PolicyAction::Warn);
        assert_eq!(config.diff.confidence_changes, PolicyAction::Fail);
    }

    #[test]
    fn a_missing_output_key_still_reads_the_diff_table() {
        // Regression guard: `load` used to discard the whole `Config` --
        // including anything parsed from `[diff]` -- the moment `output`
        // was absent. It should only fall back to the default `output`,
        // not lose the rest of an otherwise-valid file.
        let dir = tempfile::tempdir().unwrap();
        write(dir.path(), "[diff]\nresolver_changes = \"fail\"\n");
        let config = load(dir.path());
        assert_eq!(config.output, PathBuf::from("knowledge"));
        assert_eq!(config.diff.resolver_changes, PolicyAction::Fail);
    }
}
