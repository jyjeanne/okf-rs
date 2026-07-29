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
}

impl Default for Config {
    fn default() -> Self {
        Config {
            output: PathBuf::from("knowledge"),
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
    match value.get("output").and_then(|v| v.as_str()) {
        Some(output) => Config {
            output: PathBuf::from(output),
        },
        None => {
            eprintln!(
                "warning: {} has no `output` key; using default \"knowledge\"",
                path.display()
            );
            Config::default()
        }
    }
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
