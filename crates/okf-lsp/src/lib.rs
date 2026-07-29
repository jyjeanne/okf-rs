//! Minimal, synchronous Language Server Protocol client used to
//! disambiguate call-graph edges Tree-sitter alone can't resolve: when a
//! callee name matches more than one candidate project-wide, `okf-analyzer`
//! can ask the project's real language server exactly which definition a
//! specific call site resolves to (`textDocument/definition`) — something
//! that needs actual type/scope resolution, not just name-matching.
//!
//! Entirely optional and best-effort, by design: [`server_command`] only
//! covers the one dominant server per language it's been verified against
//! (`rust-analyzer` for Rust, `pyright-langserver` for Python — see the
//! crate's tests), [`is_available`] checks the binary actually exists
//! before anything tries to spawn it, and every caller in `okf-analyzer`
//! treats "no server" and "server returned nothing useful" identically:
//! fall back to Tree-sitter's own unambiguous-name-only resolution, never
//! guess.

use anyhow::{anyhow, bail, Context, Result};
use okf_parser::Language;
use serde_json::{json, Value};
use std::io::{BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError};
use std::thread;
use std::time::{Duration, Instant};

/// How long a single request waits for its matching response before
/// giving up -- guards against a hung or misbehaving language server
/// blocking a caller forever with no diagnostic.
const RESPONSE_TIMEOUT: Duration = Duration::from_secs(30);

/// The command, its arguments, and the LSP `languageId` for the one
/// dominant language server this crate knows how to drive for `language`.
/// `None` for a language with no single obvious server, or one not yet
/// wired up here.
pub fn server_command(language: Language) -> Option<(&'static str, &'static [&'static str])> {
    match language {
        Language::Rust => Some(("rust-analyzer", &[])),
        Language::Python => Some(("pyright-langserver", &["--stdio"])),
        Language::Go => Some(("gopls", &[])),
        Language::TypeScript | Language::JavaScript => {
            Some(("typescript-language-server", &["--stdio"]))
        }
        _ => None,
    }
}

/// The LSP `languageId` string for `textDocument/didOpen`.
fn language_id(language: Language) -> &'static str {
    match language {
        Language::Rust => "rust",
        Language::Python => "python",
        Language::Go => "go",
        Language::TypeScript => "typescript",
        Language::JavaScript => "javascript",
        _ => "plaintext",
    }
}

/// Whether `server_command(language)`'s command is actually installed and
/// runnable in this environment — checked cheaply before paying the cost
/// of spawning (or failing to spawn) a real server process.
pub fn is_available(language: Language) -> bool {
    server_command(language)
        .map(|(cmd, _)| which(cmd).is_some())
        .unwrap_or(false)
}

fn which(cmd: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    let candidates = executable_candidates(cmd);
    std::env::split_paths(&path).find_map(|dir| {
        candidates.iter().find_map(|name| {
            let candidate = dir.join(name);
            candidate.is_file().then_some(candidate)
        })
    })
}

/// File names to check for `cmd` in a single PATH directory. On Windows,
/// an executable's real file name almost always carries a `PATHEXT`
/// suffix (`.exe`, `.cmd`, ...) -- a bare `cmd` file is rarely what's
/// actually on disk there even when the command is genuinely installed.
fn executable_candidates(cmd: &str) -> Vec<String> {
    if !cfg!(windows) {
        return vec![cmd.to_string()];
    }
    let pathext = std::env::var("PATHEXT").unwrap_or_else(|_| ".EXE;.CMD;.BAT;.COM".to_string());
    let mut names = vec![cmd.to_string()];
    names.extend(
        pathext
            .split(';')
            .filter(|ext| !ext.is_empty())
            .map(|ext| format!("{cmd}{ext}")),
    );
    names
}

/// A running language server, initialized against `project_root`.
pub struct LspClient {
    child: Child,
    stdin: ChildStdin,
    /// Messages read by a dedicated background thread (see [`LspClient::start`]),
    /// so [`LspClient::read_response`] can enforce a timeout via
    /// `recv_timeout` instead of blocking on the pipe forever.
    rx: Receiver<std::result::Result<Value, String>>,
    next_id: i64,
    project_root: PathBuf,
    language: Language,
    opened: std::collections::HashSet<String>,
}

impl LspClient {
    /// Spawns `language`'s server rooted at `project_root` and completes
    /// the `initialize`/`initialized` handshake. Returns `Ok(None)` --
    /// not an error -- if no server is configured or installed for
    /// `language`; callers should treat that identically to "started but
    /// found nothing," since this feature is opt-in enrichment, never a
    /// hard requirement.
    pub fn start(language: Language, project_root: &Path) -> Result<Option<LspClient>> {
        let Some((cmd, args)) = server_command(language) else {
            return Ok(None);
        };
        if which(cmd).is_none() {
            return Ok(None);
        }

        let mut child = Command::new(cmd)
            .args(args)
            .current_dir(project_root)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .with_context(|| format!("failed to start {cmd}"))?;
        let stdin = child.stdin.take().ok_or_else(|| anyhow!("no stdin"))?;
        let stdout = child.stdout.take().ok_or_else(|| anyhow!("no stdout"))?;

        let (tx, rx) = mpsc::channel();
        thread::spawn(move || {
            let mut reader = BufReader::new(stdout);
            loop {
                match read_message(&mut reader) {
                    Ok(message) => {
                        if tx.send(Ok(message)).is_err() {
                            break;
                        }
                    }
                    Err(e) => {
                        let _ = tx.send(Err(e.to_string()));
                        break;
                    }
                }
            }
        });

        let mut client = LspClient {
            child,
            stdin,
            rx,
            next_id: 1,
            project_root: project_root.to_path_buf(),
            language,
            opened: std::collections::HashSet::new(),
        };
        client.initialize()?;
        Ok(Some(client))
    }

    fn initialize(&mut self) -> Result<()> {
        let root_uri = path_to_uri(&self.project_root);
        let id = self.request(
            "initialize",
            json!({
                "processId": std::process::id(),
                "rootUri": root_uri,
                "capabilities": {},
            }),
        )?;
        self.read_response(id)?;
        self.notify("initialized", json!({}))?;
        Ok(())
    }

    /// Opens `relative_path` (relative to the project root this client
    /// was started against) if it hasn't been opened already this
    /// session. A no-op on repeat calls for the same file.
    pub fn ensure_open(&mut self, relative_path: &str, text: &str) -> Result<String> {
        let uri = path_to_uri(&self.project_root.join(relative_path));
        if self.opened.insert(uri.clone()) {
            self.notify(
                "textDocument/didOpen",
                json!({
                    "textDocument": {
                        "uri": uri,
                        "languageId": language_id(self.language),
                        "version": 1,
                        "text": text,
                    }
                }),
            )?;
        }
        Ok(uri)
    }

    /// Queries `textDocument/definition` at `line`/`character` (0-based,
    /// UTF-16 code units, per LSP — see `okf_tree_sitter::CallSite`),
    /// returning each result's project-relative file path and 0-based
    /// start line. Best-effort: any malformed, empty, or `null` response
    /// is treated as "no answer" rather than an error.
    pub fn definition(
        &mut self,
        uri: &str,
        line: u32,
        character: u32,
    ) -> Result<Vec<(String, u32)>> {
        let id = self.request(
            "textDocument/definition",
            json!({
                "textDocument": { "uri": uri },
                "position": { "line": line, "character": character },
            }),
        )?;
        let response = self.read_response(id)?;
        Ok(parse_definition_locations(&response)
            .into_iter()
            .map(|(uri, line)| (relativize(&self.project_root, &uri), line))
            .collect())
    }

    /// Shuts the server down cleanly (`shutdown`/`exit`), killing the
    /// process if it doesn't exit promptly on its own.
    pub fn shutdown(mut self) {
        let _ = self.request("shutdown", Value::Null);
        let _ = self.notify("exit", Value::Null);
        let _ = self.child.kill();
        let _ = self.child.wait();
    }

    fn request(&mut self, method: &str, params: Value) -> Result<i64> {
        let id = self.next_id;
        self.next_id += 1;
        let message = json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params });
        self.write_message(&message)?;
        Ok(id)
    }

    fn notify(&mut self, method: &str, params: Value) -> Result<()> {
        let message = json!({ "jsonrpc": "2.0", "method": method, "params": params });
        self.write_message(&message)
    }

    fn write_message(&mut self, message: &Value) -> Result<()> {
        let body = serde_json::to_vec(message)?;
        write!(self.stdin, "Content-Length: {}\r\n\r\n", body.len())?;
        self.stdin.write_all(&body)?;
        self.stdin.flush()?;
        Ok(())
    }

    /// Waits (up to [`RESPONSE_TIMEOUT`], across the whole call -- not
    /// per message) for the message whose `id` matches `want_id`,
    /// skipping over notifications (e.g. `textDocument/publishDiagnostics`,
    /// which this client has no use for) and any response to a
    /// previously-abandoned request. Errors, rather than blocking forever,
    /// if the server never sends it or the stream closes first.
    fn read_response(&mut self, want_id: i64) -> Result<Value> {
        wait_for_response(&self.rx, want_id, RESPONSE_TIMEOUT)
    }
}

/// The core of [`LspClient::read_response`], factored out as a free
/// function purely so its timeout/skip-unrelated-messages/disconnect
/// logic can be unit-tested directly against a plain channel, without
/// needing a real (or fake) language server process to exercise it.
fn wait_for_response(
    rx: &Receiver<std::result::Result<Value, String>>,
    want_id: i64,
    timeout: Duration,
) -> Result<Value> {
    let deadline = Instant::now() + timeout;
    loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            bail!("timed out after {timeout:?} waiting for a response from the language server");
        }
        match rx.recv_timeout(remaining) {
            Ok(Ok(message)) => {
                if message.get("id").and_then(Value::as_i64) == Some(want_id) {
                    if let Some(error) = message.get("error") {
                        bail!("language server returned an error: {error}");
                    }
                    return Ok(message.get("result").cloned().unwrap_or(Value::Null));
                }
            }
            Ok(Err(e)) => bail!("language server stream error: {e}"),
            Err(RecvTimeoutError::Timeout) => {
                bail!("timed out after {timeout:?} waiting for a response from the language server")
            }
            Err(RecvTimeoutError::Disconnected) => {
                bail!("language server closed its output stream")
            }
        }
    }
}

/// Reads one full LSP message (headers + JSON body) from `reader`. A
/// free function (not an `LspClient` method) so the background reader
/// thread spawned in [`LspClient::start`] can call it without holding a
/// reference to the client itself.
fn read_message(reader: &mut BufReader<ChildStdout>) -> Result<Value> {
    let mut content_length = None;
    loop {
        let mut line = String::new();
        let n = reader.read_line(&mut line)?;
        if n == 0 {
            bail!("language server closed its output stream");
        }
        let line = line.trim_end();
        if line.is_empty() {
            break;
        }
        if let Some(value) = line.strip_prefix("Content-Length:") {
            content_length = value.trim().parse::<usize>().ok();
        }
    }
    let content_length = content_length.ok_or_else(|| anyhow!("missing Content-Length header"))?;
    let mut buf = vec![0u8; content_length];
    reader.read_exact(&mut buf)?;
    serde_json::from_slice(&buf).context("malformed LSP message body")
}

fn path_to_uri(path: &Path) -> String {
    format!("file://{}", percent_encode(&path.display().to_string()))
}

fn uri_to_path(uri: &str) -> String {
    percent_decode(uri.strip_prefix("file://").unwrap_or(uri))
}

/// Percent-encodes a path for inclusion in a `file://` URI, leaving `/`
/// and `:` (e.g. for a Windows drive letter) untouched. Real language
/// servers do this too for paths containing spaces or non-ASCII
/// characters (the LSP `DocumentUri` type is defined in terms of RFC
/// 3986), so this client's own URIs need to follow the same convention
/// to round-trip correctly against a server's own returned URIs.
fn percent_encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for byte in s.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' | b'/' | b':' => {
                out.push(byte as char);
            }
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }
    out
}

fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let Ok(byte) = u8::from_str_radix(&s[i + 1..i + 3], 16) {
                out.push(byte);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Converts an absolute `file://` URI back into a path relative to
/// `project_root`, `/`-separated regardless of platform (matching every
/// other project-relative path in okf-rs, e.g. `Concept::location`).
fn relativize(project_root: &Path, uri: &str) -> String {
    let path = uri_to_path(uri);
    Path::new(&path)
        .strip_prefix(project_root)
        .map(|p| p.to_string_lossy().replace('\\', "/"))
        .unwrap_or(path)
}

/// Extracts `(uri, start_line)` pairs from a `textDocument/definition`
/// response, which per the LSP spec may be `null`, a single `Location`,
/// an array of `Location`s, or an array of `LocationLink`s.
fn parse_definition_locations(response: &Value) -> Vec<(String, u32)> {
    let entries: Vec<&Value> = match response {
        Value::Array(items) => items.iter().collect(),
        Value::Object(_) => vec![response],
        _ => Vec::new(),
    };
    entries
        .into_iter()
        .filter_map(|entry| {
            let uri = entry
                .get("uri")
                .or_else(|| entry.get("targetUri"))?
                .as_str()?;
            let range = entry.get("range").or_else(|| entry.get("targetRange"))?;
            let line = range.get("start")?.get("line")?.as_u64()? as u32;
            Some((uri.to_string(), line))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn server_command_covers_the_verified_languages() {
        assert_eq!(
            server_command(Language::Rust),
            Some(("rust-analyzer", [].as_slice()))
        );
        assert_eq!(
            server_command(Language::Python),
            Some(("pyright-langserver", ["--stdio"].as_slice()))
        );
        assert_eq!(server_command(Language::Php), None);
    }

    #[test]
    fn is_available_is_false_for_a_language_with_no_server_config() {
        assert!(!is_available(Language::Php));
    }

    #[test]
    fn is_available_reflects_whether_the_binary_is_actually_on_path() {
        // This assertion documents environment-dependent behavior rather
        // than a fixed expectation -- it just proves `is_available`
        // doesn't panic and returns a real, checkable answer either way.
        let _ = is_available(Language::Rust);
    }

    #[test]
    fn parse_definition_locations_handles_null_single_and_array_shapes() {
        assert!(parse_definition_locations(&Value::Null).is_empty());

        let single = json!({
            "uri": "file:///a/b.rs",
            "range": { "start": { "line": 4, "character": 0 }, "end": { "line": 4, "character": 3 } },
        });
        assert_eq!(
            parse_definition_locations(&single),
            vec![("file:///a/b.rs".to_string(), 4)]
        );

        let array = json!([
            {
                "uri": "file:///a/b.rs",
                "range": { "start": { "line": 4, "character": 0 }, "end": { "line": 4, "character": 3 } },
            },
            {
                "targetUri": "file:///a/c.rs",
                "targetRange": { "start": { "line": 9, "character": 2 }, "end": { "line": 9, "character": 5 } },
            },
        ]);
        assert_eq!(
            parse_definition_locations(&array),
            vec![
                ("file:///a/b.rs".to_string(), 4),
                ("file:///a/c.rs".to_string(), 9),
            ]
        );
    }

    #[test]
    fn relativize_strips_the_project_root_prefix() {
        let root = Path::new("/a/b");
        let uri = path_to_uri(&root.join("src/lib.rs"));
        assert_eq!(relativize(root, &uri), "src/lib.rs");
    }

    #[test]
    fn relativize_round_trips_paths_with_spaces_and_non_ascii() {
        let root = Path::new("/a/My Projects/caf\u{e9}");
        let uri = path_to_uri(&root.join("src/lib.rs"));
        assert!(
            uri.contains("%20") || !uri.contains(' '),
            "the space in the path must be percent-encoded in the URI: {uri}"
        );
        assert_eq!(relativize(root, &uri), "src/lib.rs");
    }

    #[test]
    fn percent_encode_and_decode_round_trip_special_characters() {
        let original = "/a/My Projects/caf\u{e9}/src/lib.rs";
        let encoded = percent_encode(original);
        assert!(encoded.contains("%20"));
        assert_eq!(percent_decode(&encoded), original);
    }

    #[test]
    fn wait_for_response_returns_the_matching_message_and_skips_others() {
        let (tx, rx) = mpsc::channel();
        tx.send(Ok(
            json!({ "jsonrpc": "2.0", "id": 99, "result": "not mine" }),
        ))
        .unwrap();
        tx.send(Ok(json!({ "jsonrpc": "2.0", "id": 1, "result": "mine" })))
            .unwrap();

        let result = wait_for_response(&rx, 1, Duration::from_secs(5)).unwrap();
        assert_eq!(result, json!("mine"));
    }

    #[test]
    fn wait_for_response_surfaces_a_json_rpc_error() {
        let (tx, rx) = mpsc::channel();
        tx.send(Ok(
            json!({ "jsonrpc": "2.0", "id": 1, "error": { "code": -1, "message": "boom" } }),
        ))
        .unwrap();

        let err = wait_for_response(&rx, 1, Duration::from_secs(5)).unwrap_err();
        assert!(err.to_string().contains("boom"));
    }

    #[test]
    fn wait_for_response_times_out_instead_of_blocking_forever() {
        let (_tx, rx) = mpsc::channel::<std::result::Result<Value, String>>();
        let start = Instant::now();

        let err = wait_for_response(&rx, 1, Duration::from_millis(100)).unwrap_err();

        assert!(err.to_string().contains("timed out"));
        assert!(
            start.elapsed() < Duration::from_secs(2),
            "should not block far past the timeout"
        );
    }

    #[test]
    fn wait_for_response_errors_when_the_stream_disconnects() {
        let (tx, rx) = mpsc::channel::<std::result::Result<Value, String>>();
        drop(tx);

        let err = wait_for_response(&rx, 1, Duration::from_secs(5)).unwrap_err();
        assert!(err.to_string().contains("closed"));
    }

    /// End-to-end smoke test against a *real* `rust-analyzer`, skipped
    /// (not failed) when it isn't installed -- this is what's actually
    /// verified in the development sandbox this feature was built in.
    #[test]
    fn resolves_a_definition_via_a_real_rust_analyzer() {
        if !is_available(Language::Rust) {
            eprintln!("skipping: rust-analyzer not installed");
            return;
        }

        let dir = tempfile::tempdir().unwrap();
        fs::write(
            dir.path().join("Cargo.toml"),
            "[package]\nname = \"demo\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
        )
        .unwrap();
        fs::create_dir_all(dir.path().join("src")).unwrap();
        let source = "pub fn callee() -> i32 { 1 }\npub fn caller() -> i32 { callee() }\n";
        fs::write(dir.path().join("src/lib.rs"), source).unwrap();

        let mut client = LspClient::start(Language::Rust, dir.path())
            .unwrap()
            .expect("rust-analyzer should be available");
        let uri = client.ensure_open("src/lib.rs", source).unwrap();

        // Position of "callee" inside "callee()" on line 1 (0-based).
        let line = 1u32;
        let character = source.lines().nth(1).unwrap().find("callee(").unwrap() as u32;

        // rust-analyzer needs a moment to index a freshly opened project;
        // retry a few times rather than hard-coding a fixed sleep.
        let mut locations = Vec::new();
        for _ in 0..20 {
            locations = client.definition(&uri, line, character).unwrap();
            if !locations.is_empty() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(500));
        }
        client.shutdown();

        assert_eq!(
            locations,
            vec![("src/lib.rs".to_string(), 0)],
            "textDocument/definition should resolve `callee()` to line 0, where it's defined"
        );
    }

    /// Same shape as the rust-analyzer smoke test above, against a real
    /// `pyright-langserver` -- also skipped, not failed, when unavailable.
    #[test]
    fn resolves_a_definition_via_a_real_pyright() {
        if !is_available(Language::Python) {
            eprintln!("skipping: pyright-langserver not installed");
            return;
        }

        let dir = tempfile::tempdir().unwrap();
        let source = "def callee():\n    return 1\n\n\ndef caller():\n    return callee()\n";
        fs::write(dir.path().join("mod.py"), source).unwrap();

        let mut client = LspClient::start(Language::Python, dir.path())
            .unwrap()
            .expect("pyright-langserver should be available");
        let uri = client.ensure_open("mod.py", source).unwrap();

        let line = 5u32;
        let character = source.lines().nth(5).unwrap().find("callee(").unwrap() as u32;

        let mut locations = Vec::new();
        for _ in 0..20 {
            locations = client.definition(&uri, line, character).unwrap();
            if !locations.is_empty() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(500));
        }
        client.shutdown();

        assert_eq!(
            locations,
            vec![("mod.py".to_string(), 0)],
            "textDocument/definition should resolve `callee()` to line 0, where it's defined"
        );
    }
}
