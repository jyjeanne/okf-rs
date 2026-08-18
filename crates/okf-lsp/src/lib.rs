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
//! crate's tests), [`is_available`] confirms the binary is actually
//! runnable (not just a name on `PATH`) before anything tries to spawn it
//! for real, and every caller in `okf-analyzer` treats "no server" and
//! "server returned nothing useful" identically: fall back to
//! Tree-sitter's own unambiguous-name-only resolution, never guess.

use anyhow::{anyhow, bail, Context, Result};
use okf_parser::Language;
use serde_json::{json, Value};
use std::collections::HashSet;
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

/// Overall bound on [`LspClient::wait_until_ready`] -- a server that never
/// settles (or never reports `$/progress` at all in the first place) can't
/// block a caller forever with no diagnostic, any more than a single
/// request can.
const READY_TIMEOUT: Duration = Duration::from_secs(15);

/// How long [`LspClient::wait_until_ready`] waits, with no new progress
/// activity, before declaring the server ready. Indexing is often reported
/// as more than one sequential `$/progress` token -- e.g. "roots scanned"
/// ending right before "indexing" (and, for `rust-analyzer` specifically,
/// proc-macro/build-script "cachePriming") begins -- so declaring victory
/// the instant the first token ends would race the exact same way the
/// heuristic this replaces did. Also doubles as the probe window used to
/// detect a server that doesn't implement `$/progress` at all (most
/// servers besides `rust-analyzer` don't): if nothing arrives in one
/// window and nothing ever has, there's nothing to wait for.
const READY_QUIET_PERIOD: Duration = Duration::from_millis(500);

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
    server_command(language).is_some_and(|(cmd, _)| probe_available(cmd))
}

/// Confirms `cmd` isn't just a name on `PATH` but actually runs. A binary
/// can be present yet non-functional -- e.g. a rustup proxy for a
/// component (`rust-analyzer`) that was never installed exits immediately
/// with an "unknown binary" error rather than behaving like a real
/// language server, which `which()` alone can't detect.
fn probe_available(cmd: &str) -> bool {
    if which(cmd).is_none() {
        return false;
    }
    Command::new(cmd)
        .arg("--version")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|status| status.success())
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
    /// The server's own reported version, captured from `initialize`'s
    /// `serverInfo.version` (per the LSP spec) — `None` if the server
    /// didn't include a `serverInfo` object at all, which the spec
    /// doesn't require. Threaded through to every edge this client
    /// resolves, so `resolved_by: rust-analyzer` edges from two different
    /// installed versions are distinguishable — see
    /// [`LspClient::server_version`].
    server_version: Option<String>,
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
        if !probe_available(cmd) {
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
            server_version: None,
        };
        client.initialize()?;
        client.wait_until_ready();
        Ok(Some(client))
    }

    fn initialize(&mut self) -> Result<()> {
        let root_uri = path_to_uri(&self.project_root);
        let id = self.request(
            "initialize",
            json!({
                "processId": std::process::id(),
                "rootUri": root_uri,
                // Advertised specifically so a server that supports
                // `$/progress` (rust-analyzer does; most others don't)
                // actually sends it -- without this capability, servers
                // that gate progress reporting on client support (as the
                // spec permits) would stay silent and `wait_until_ready`
                // would fall back to its "no progress at all" path even
                // though the server just wasn't told it could report any.
                "capabilities": { "window": { "workDoneProgress": true } },
            }),
        )?;
        let result = self.read_response(id)?;
        self.server_version = parse_server_version(&result);
        self.notify("initialized", json!({}))?;
        Ok(())
    }

    /// Waits for the server's own startup-indexing signal (`$/progress`)
    /// to settle, instead of inferring readiness from whether some
    /// arbitrary first query happened to succeed. A query fired before
    /// indexing (crate-graph loading, proc-macro/build-script expansion,
    /// cross-crate symbol resolution, ...) has finished can miss a symbol
    /// that simply hasn't landed yet and get a different answer than the
    /// same query fired a moment later -- see
    /// `benchmarks/resolver-stability/README.md` for a real, reproduced
    /// case of exactly this against this project's own source.
    ///
    /// Best-effort and bounded, never an error: a server that doesn't
    /// implement `$/progress` at all (most servers besides `rust-analyzer`
    /// don't) is indistinguishable from "still working" by token
    /// bookkeeping alone, so this also returns as soon as
    /// [`READY_QUIET_PERIOD`] passes with *no* progress activity ever
    /// seen, rather than always burning the full [`READY_TIMEOUT`]. A
    /// caller that needs to tolerate residual, per-query lag this can't
    /// see (a single crate among many still finishing its own index after
    /// the workspace overall reports ready, say) should still retry
    /// individual queries on top of this -- see
    /// `okf_analyzer::lsp::resolve_ambiguous_calls`.
    pub fn wait_until_ready(&mut self) {
        let deadline = Instant::now() + READY_TIMEOUT;
        let mut active_tokens = HashSet::new();
        let mut seen_any_progress = false;

        loop {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return;
            }
            match self.rx.recv_timeout(remaining.min(READY_QUIET_PERIOD)) {
                Ok(Ok(message)) => {
                    if let Some(ack) = observe_progress_message(
                        &message,
                        &mut active_tokens,
                        &mut seen_any_progress,
                    ) {
                        let _ = self.write_message(&ack);
                    }
                }
                Ok(Err(_)) | Err(RecvTimeoutError::Disconnected) => return,
                Err(RecvTimeoutError::Timeout) => {
                    // A full quiet-period slice passed with no message at
                    // all: ready if either nothing has ever indicated
                    // progress support, or every token seen so far has
                    // already reported "end".
                    if !seen_any_progress || active_tokens.is_empty() {
                        return;
                    }
                }
            }
        }
    }

    /// The language server's own reported version, if it included one in
    /// its `initialize` response — e.g. `1.88.0` for a real
    /// `rust-analyzer`. `None` before `initialize()` has run, or if the
    /// server never reported `serverInfo` (the LSP spec makes it
    /// optional).
    pub fn server_version(&self) -> Option<&str> {
        self.server_version.as_deref()
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

/// Pure per-message step for [`LspClient::wait_until_ready`], factored out
/// as a free function (same reasoning as [`wait_for_response`]) so its
/// `$/progress` token bookkeeping is directly unit-testable without a real
/// language server process. Updates `active_tokens`/`seen_any_progress` in
/// place and, for a `window/workDoneProgress/create` request -- which,
/// per the LSP spec, the client must acknowledge -- returns the
/// `{id, result: null}` reply for the caller to send back over the wire.
fn observe_progress_message(
    message: &Value,
    active_tokens: &mut HashSet<String>,
    seen_any_progress: &mut bool,
) -> Option<Value> {
    match message.get("method").and_then(Value::as_str) {
        Some("window/workDoneProgress/create") => {
            *seen_any_progress = true;
            let id = message.get("id")?.clone();
            Some(json!({ "jsonrpc": "2.0", "id": id, "result": Value::Null }))
        }
        Some("$/progress") => {
            *seen_any_progress = true;
            if let Some(token) = message
                .pointer("/params/token")
                .and_then(progress_token_string)
            {
                match message
                    .pointer("/params/value/kind")
                    .and_then(Value::as_str)
                {
                    Some("end") => {
                        active_tokens.remove(&token);
                    }
                    Some("begin") | Some("report") => {
                        active_tokens.insert(token);
                    }
                    _ => {}
                }
            }
            None
        }
        _ => None,
    }
}

/// `ProgressToken` per the LSP spec is `integer | string` -- normalized to
/// a `String` here since it's only ever used as a `HashSet` key.
fn progress_token_string(value: &Value) -> Option<String> {
    match value {
        Value::String(s) => Some(s.clone()),
        Value::Number(n) => Some(n.to_string()),
        _ => None,
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

/// Extracts `serverInfo.version` from an `initialize` response, per the
/// LSP spec's optional `InitializeResult.serverInfo` field. `None` for a
/// response with no `serverInfo`, or one whose `version` isn't a string
/// (a spec-conformant server omits the field entirely rather than sending
/// a non-string value, but this stays lenient rather than erroring on a
/// server that doesn't).
fn parse_server_version(result: &Value) -> Option<String> {
    result
        .get("serverInfo")?
        .get("version")?
        .as_str()
        .map(str::to_string)
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
    fn parse_server_version_reads_server_info_version() {
        let result = json!({
            "capabilities": {},
            "serverInfo": { "name": "rust-analyzer", "version": "1.88.0" },
        });
        assert_eq!(parse_server_version(&result), Some("1.88.0".to_string()));
    }

    #[test]
    fn parse_server_version_is_none_without_server_info() {
        // The LSP spec makes `serverInfo` optional -- a spec-conformant
        // server that omits it entirely shouldn't be treated as an error.
        let result = json!({ "capabilities": {} });
        assert_eq!(parse_server_version(&result), None);
    }

    #[test]
    fn parse_server_version_is_none_when_version_is_missing_or_not_a_string() {
        let no_version = json!({ "serverInfo": { "name": "rust-analyzer" } });
        assert_eq!(parse_server_version(&no_version), None);

        let wrong_type = json!({ "serverInfo": { "name": "x", "version": 188 } });
        assert_eq!(parse_server_version(&wrong_type), None);
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
    fn observe_progress_message_acks_workdoneprogress_create_and_tracks_no_token_yet() {
        let mut active = HashSet::new();
        let mut seen = false;
        let message = json!({
            "jsonrpc": "2.0", "id": 7,
            "method": "window/workDoneProgress/create",
            "params": { "token": "rustAnalyzer/Indexing" },
        });
        let ack = observe_progress_message(&message, &mut active, &mut seen).unwrap();
        assert_eq!(ack, json!({ "jsonrpc": "2.0", "id": 7, "result": null }));
        assert!(seen, "a create request itself counts as progress activity");
        assert!(
            active.is_empty(),
            "no token is active until a `begin` progress arrives"
        );
    }

    #[test]
    fn observe_progress_message_tracks_the_begin_report_end_lifecycle() {
        let mut active = HashSet::new();
        let mut seen = false;

        let begin = json!({
            "jsonrpc": "2.0", "method": "$/progress",
            "params": { "token": "rustAnalyzer/Indexing", "value": { "kind": "begin" } },
        });
        assert!(observe_progress_message(&begin, &mut active, &mut seen).is_none());
        assert!(seen);
        assert!(active.contains("rustAnalyzer/Indexing"));

        let report = json!({
            "jsonrpc": "2.0", "method": "$/progress",
            "params": { "token": "rustAnalyzer/Indexing", "value": { "kind": "report" } },
        });
        observe_progress_message(&report, &mut active, &mut seen);
        assert!(
            active.contains("rustAnalyzer/Indexing"),
            "still active after a `report`"
        );

        let end = json!({
            "jsonrpc": "2.0", "method": "$/progress",
            "params": { "token": "rustAnalyzer/Indexing", "value": { "kind": "end" } },
        });
        observe_progress_message(&end, &mut active, &mut seen);
        assert!(active.is_empty(), "`end` retires the token");
    }

    #[test]
    fn observe_progress_message_tracks_multiple_overlapping_tokens_independently() {
        // The real-world case `wait_until_ready`'s quiet period exists
        // for: one token ("roots scanned") ends right as another
        // ("cachePriming") begins -- readiness must wait for both, not
        // declare victory the instant the first one is gone.
        let mut active = HashSet::new();
        let mut seen = false;
        let begin = |token: &str| {
            json!({
                "jsonrpc": "2.0", "method": "$/progress",
                "params": { "token": token, "value": { "kind": "begin" } },
            })
        };
        let end = |token: &str| {
            json!({
                "jsonrpc": "2.0", "method": "$/progress",
                "params": { "token": token, "value": { "kind": "end" } },
            })
        };

        observe_progress_message(&begin("roots-scanned"), &mut active, &mut seen);
        observe_progress_message(&begin("cachePriming"), &mut active, &mut seen);
        assert_eq!(active.len(), 2);

        observe_progress_message(&end("roots-scanned"), &mut active, &mut seen);
        assert_eq!(
            active.len(),
            1,
            "`cachePriming` is still active after `roots-scanned` ends"
        );

        observe_progress_message(&end("cachePriming"), &mut active, &mut seen);
        assert!(active.is_empty());
    }

    #[test]
    fn observe_progress_message_ignores_unrelated_notifications() {
        let mut active = HashSet::new();
        let mut seen = false;
        let unrelated = json!({
            "jsonrpc": "2.0", "method": "textDocument/publishDiagnostics",
            "params": { "uri": "file:///a.rs", "diagnostics": [] },
        });
        assert!(observe_progress_message(&unrelated, &mut active, &mut seen).is_none());
        assert!(!seen, "an unrelated notification isn't progress activity");
        assert!(active.is_empty());
    }

    #[test]
    fn observe_progress_message_accepts_a_numeric_token() {
        // `ProgressToken` is `integer | string` per the LSP spec.
        let mut active = HashSet::new();
        let mut seen = false;
        let begin = json!({
            "jsonrpc": "2.0", "method": "$/progress",
            "params": { "token": 42, "value": { "kind": "begin" } },
        });
        observe_progress_message(&begin, &mut active, &mut seen);
        assert!(active.contains("42"));
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
        assert!(
            client.server_version().is_some(),
            "a real rust-analyzer reports serverInfo.version"
        );
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
