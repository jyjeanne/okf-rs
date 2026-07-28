use anyhow::{Context, Result};
use std::fs;
use std::path::{Path, PathBuf};

/// Minimal per-project config written by `okf-rs init` and read by the
/// other commands so `--output`/bundle paths don't need repeating on every
/// invocation. Anything not found here (or no `okf.toml` at all) falls
/// back to the literal default `knowledge`.
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

pub fn load(project_root: &Path) -> Config {
    let path = project_root.join(CONFIG_FILE);
    let Ok(content) = fs::read_to_string(&path) else {
        return Config::default();
    };
    let Ok(value) = content.parse::<toml::Value>() else {
        return Config::default();
    };
    let output = value
        .get("output")
        .and_then(|v| v.as_str())
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("knowledge"));
    Config { output }
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
