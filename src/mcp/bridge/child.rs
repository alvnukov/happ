//! Drives an external language server over LSP stdio.
//!
//! Reading a child's stdout inline would let a wedged server hang the MCP
//! connection forever, so a reader thread owns the pipe and the request path
//! only ever waits on a channel with a deadline. A server that stops answering
//! costs one slow tool call, not the session.

use lsp_server::{Message, Notification, Request, RequestId, Response};
use serde_json::{json, Value as JsonValue};
use std::collections::{HashMap, HashSet};
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use super::{path_to_uri, severity_label, uri_to_path, Diagnostic, LanguageSpec, LspProvider};

/// How long to wait for one response. Generous, because the first request to a
/// cold `rust-analyzer` or `gopls` waits on indexing.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(90);

/// How long to wait for a server to publish diagnostics after a file is opened.
/// Absence of diagnostics is itself an answer, so this cannot be long.
const DIAGNOSTICS_QUIET_PERIOD: Duration = Duration::from_secs(5);

/// How long to wait for a cold server to finish loading a workspace before its
/// first answer. rust-analyzer runs `cargo check` here, which on a large
/// project is minutes, not seconds.
const WARMUP_BUDGET: Duration = Duration::from_secs(120);

/// How long the server must stay silent, with nothing in flight, to count as
/// idle.
const SETTLED_QUIET_PERIOD: Duration = Duration::from_millis(800);

/// How long to wait between retries while a server is still warming up.
const WARMUP_RETRY_PERIOD: Duration = Duration::from_secs(1);

/// How many times to re-ask a cold server before accepting an empty answer.
const WARMUP_RETRIES: usize = 8;

/// Cap on distinct retained complaints: enough to explain a refusal, not enough
/// for a chatty server to grow without bound over a long session.
const MAX_DISTINCT_COMPLAINTS: usize = 32;

/// Cap on one complaint, in characters. A server is free to print a stack trace
/// per line; the caller is not free to stop paying for it.
const MAX_COMPLAINT_CHARS: usize = 2 * 1024;

/// How many distinct complaints an answer carries, and how much of each.
const COMPLAINTS_SHOWN: usize = 4;
const COMPLAINT_LINES_SHOWN: usize = 4;

/// What a server has said about itself, kept without repetition.
///
/// Two things make raw capture unusable. A server repeats itself -- rust-analyzer
/// logs some conditions once per expression it touches, and one question about
/// one clean file came back with seventy copies of `inference diagnostic in
/// desugared expr` -- so messages are stored distinctly with a count of how
/// often each recurred, the recurrence itself being worth knowing.
///
/// And a server has two voices. `window/logMessage` and the server-status
/// extension are specified, addressed to the client, and already phrased for a
/// reader. Its stderr is unspecified vendor logging: timestamps, log levels and
/// paragraphs split across lines that no longer reassemble. So the spoken
/// channel is preferred whenever there is one, and stderr is kept for the case
/// that channel cannot cover -- a server that dies before it can speak LSP at
/// all, where stderr is the only place the reason exists.
#[derive(Default)]
struct Complaints {
    /// What the server said over the protocol.
    spoken: Vec<(String, usize)>,
    /// What it printed to its own stderr.
    printed: Vec<(String, usize)>,
}

impl Complaints {
    fn spoke(&mut self, message: &str) {
        record(&mut self.spoken, message);
    }

    fn printed(&mut self, line: &str) {
        record(&mut self.printed, line);
    }

    fn summary(&self) -> Option<String> {
        let entries = if self.spoken.is_empty() {
            &self.printed
        } else {
            &self.spoken
        };
        render(entries)
    }
}

fn record(entries: &mut Vec<(String, usize)>, message: &str) {
    let message = message.trim();
    if message.is_empty() {
        return;
    }
    if let Some(entry) = entries.iter_mut().find(|(seen, _)| seen == message) {
        entry.1 += 1;
        return;
    }
    if entries.len() < MAX_DISTINCT_COMPLAINTS {
        entries.push((message.chars().take(MAX_COMPLAINT_CHARS).collect(), 1));
    }
}

/// The first few distinct messages, each shortened to its opening lines.
///
/// A server's complaint is front-loaded -- "rustc 1.93.1 is not supported by the
/// following package" precedes the paragraph on how to upgrade -- so the opening
/// lines carry the diagnosis and the server's own log has the rest.
fn render(entries: &[(String, usize)]) -> Option<String> {
    if entries.is_empty() {
        return None;
    }
    let mut lines: Vec<String> = entries
        .iter()
        .take(COMPLAINTS_SHOWN)
        .map(|(message, seen)| {
            let text = first_lines(message, COMPLAINT_LINES_SHOWN);
            if *seen > 1 {
                format!("{text}\n(repeated {seen} times)")
            } else {
                text
            }
        })
        .collect();
    if entries.len() > COMPLAINTS_SHOWN {
        lines.push(format!(
            "({} further distinct messages, in the server's own log.)",
            entries.len() - COMPLAINTS_SHOWN
        ));
    }
    Some(lines.join("\n"))
}

/// Keeps the opening lines of `text`, saying how many were dropped.
fn first_lines(text: &str, keep: usize) -> String {
    let lines: Vec<&str> = text
        .lines()
        .map(str::trim_end)
        .filter(|line| !line.is_empty())
        .collect();
    if lines.len() <= keep {
        return lines.join("\n");
    }
    format!(
        "{}\n... ({} more lines)",
        lines[..keep].join("\n"),
        lines.len() - keep
    )
}

pub(crate) struct ChildProvider {
    child: Child,
    stdin: ChildStdin,
    incoming: Receiver<Message>,
    next_id: i32,
    opened: HashMap<PathBuf, i32>,
    language_id: &'static str,
    capabilities: JsonValue,
    /// Latest diagnostics per file, as published by the server.
    published: HashMap<PathBuf, Vec<Diagnostic>>,
    /// What the server has said about itself, kept for failure messages.
    complaints: Arc<Mutex<Complaints>>,
    /// Progress tokens the server has begun and not yet ended.
    working: HashSet<String>,
    /// Whether the one-off wait for start-up work has already been paid.
    warmed: bool,
    /// Set once the server has answered something non-empty. Until then its
    /// empty answers are treated as "not ready yet" rather than as facts.
    proven: bool,
    /// The server has declared itself quiescent, if it speaks that extension.
    quiescent: bool,
    /// Whether the server ever sent `experimental/serverStatus`, which decides
    /// whether quiescence is a signal we can wait for at all.
    reports_status: bool,
}

impl ChildProvider {
    pub(crate) fn start(
        spec: &LanguageSpec,
        command: &[String],
        root: &Path,
    ) -> Result<Self, String> {
        let (program, args) = command
            .split_first()
            .ok_or_else(|| format!("no command configured for {}", spec.id))?;

        let mut child = Command::new(program)
            .args(args)
            .current_dir(root)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            // A language server's own logging must never reach our stdout,
            // which carries MCP frames -- but it is also the only place a
            // server explains why it refused to start, so it is captured
            // rather than discarded.
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|err| format!("cannot start '{program}': {err}"))?;

        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| format!("'{program}' gave no stdin"))?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| format!("'{program}' gave no stdout"))?;
        let stderr = child
            .stderr
            .take()
            .ok_or_else(|| format!("'{program}' gave no stderr"))?;

        // Drained continuously: a server that fills its stderr pipe and blocks
        // would look exactly like one that hung.
        let complaints = Arc::new(Mutex::new(Complaints::default()));
        let sink = Arc::clone(&complaints);
        thread::spawn(move || {
            for line in BufReader::new(stderr).lines().map_while(Result::ok) {
                let Ok(mut buffer) = sink.lock() else {
                    return;
                };
                buffer.printed(&line);
            }
        });

        let (sender, incoming) = mpsc::channel();
        thread::spawn(move || {
            let mut reader = BufReader::new(stdout);
            while let Ok(Some(message)) = Message::read(&mut reader) {
                if sender.send(message).is_err() {
                    break;
                }
            }
        });

        let mut provider = Self {
            child,
            stdin,
            incoming,
            next_id: 0,
            opened: HashMap::new(),
            language_id: spec.language_id,
            capabilities: JsonValue::Null,
            published: HashMap::new(),
            complaints,
            working: HashSet::new(),
            warmed: false,
            proven: false,
            quiescent: false,
            reports_status: false,
        };
        provider
            .initialize(root)
            .map_err(|err| provider.explain(err))?;
        Ok(provider)
    }

    /// Adds whatever the server printed to its own stderr, which is routinely
    /// the only thing that says *why* -- a rustup shim reporting an uninstalled
    /// component looks identical to a hang without it.
    fn explain(&self, err: String) -> String {
        match self
            .complaints
            .lock()
            .ok()
            .and_then(|buffer| buffer.summary())
        {
            Some(complaint) => format!("{err}\nthe server said: {complaint}"),
            None => err,
        }
    }

    fn initialize(&mut self, root: &Path) -> Result<(), String> {
        let root_uri = path_to_uri(root)?;
        let result = self.send_request(
            "initialize",
            json!({
                "processId": std::process::id(),
                "rootUri": root_uri,
                "workspaceFolders": [{ "uri": root_uri, "name": "workspace" }],
                "clientInfo": { "name": "happ", "version": env!("CARGO_PKG_VERSION") },
                "capabilities": {
                    "textDocument": {
                        "synchronization": { "dynamicRegistration": false },
                        "hover": { "contentFormat": ["plaintext", "markdown"] },
                        "definition": { "linkSupport": false },
                        "references": {},
                        "documentSymbol": { "hierarchicalDocumentSymbolSupport": true },
                        "publishDiagnostics": {},
                        "callHierarchy": {},
                        "implementation": { "linkSupport": false },
                        "typeDefinition": { "linkSupport": false },
                    },
                    "workspace": {
                        "symbol": {},
                        "workspaceFolders": true,
                    },
                    // rust-analyzer only reports readiness when asked to:
                    // docs/dev/lsp-extensions.md, "Server Status".
                    "experimental": { "serverStatusNotification": true },
                },
            }),
        )?;
        self.capabilities = result
            .get("capabilities")
            .cloned()
            .unwrap_or(JsonValue::Null);
        self.notify("initialized", json!({}))
    }

    fn notify(&mut self, method: &str, params: JsonValue) -> Result<(), String> {
        self.send(Message::Notification(Notification {
            method: method.to_string(),
            params,
        }))
    }

    fn send(&mut self, message: Message) -> Result<(), String> {
        message
            .write(&mut self.stdin)
            .map_err(|err| format!("write to language server: {err}"))?;
        self.stdin
            .flush()
            .map_err(|err| format!("flush language server stdin: {err}"))
    }

    /// Waits for the response to `id`, absorbing everything else that arrives.
    ///
    /// Servers interleave progress notifications, diagnostics and requests of
    /// their own with responses; all of them have to be drained or the pipe
    /// stalls.
    fn await_response(&mut self, id: RequestId) -> Result<JsonValue, String> {
        let deadline = Instant::now() + REQUEST_TIMEOUT;
        loop {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return Err(format!(
                    "language server did not answer within {}s",
                    REQUEST_TIMEOUT.as_secs()
                ));
            }
            match self.incoming.recv_timeout(remaining) {
                Ok(Message::Response(response)) if response.id == id => {
                    if let Some(error) = response.error {
                        return Err(format!(
                            "language server error {}: {}",
                            error.code, error.message
                        ));
                    }
                    return Ok(response.result.unwrap_or(JsonValue::Null));
                }
                Ok(Message::Response(_)) => {}
                Ok(Message::Notification(notification)) => self.absorb(&notification),
                Ok(Message::Request(request)) => self.decline(request)?,
                Err(RecvTimeoutError::Timeout) => {
                    return Err(format!(
                        "language server did not answer within {}s",
                        REQUEST_TIMEOUT.as_secs()
                    ))
                }
                Err(RecvTimeoutError::Disconnected) => {
                    return Err("language server exited".to_string())
                }
            }
        }
    }

    fn absorb(&mut self, notification: &Notification) {
        if notification.method == "$/progress" {
            self.track_progress(&notification.params);
            return;
        }
        if notification.method == "experimental/serverStatus" {
            self.track_server_status(&notification.params);
            return;
        }
        if notification.method == "window/showMessage" || notification.method == "window/logMessage"
        {
            self.record_complaint(&notification.params);
            return;
        }
        if notification.method != "textDocument/publishDiagnostics" {
            return;
        }
        let Some(uri) = notification
            .params
            .get("uri")
            .and_then(JsonValue::as_str)
            .and_then(uri_to_path)
        else {
            return;
        };
        let diagnostics = notification
            .params
            .get("diagnostics")
            .and_then(JsonValue::as_array)
            .map(|items| items.iter().map(decode_diagnostic).collect())
            .unwrap_or_default();
        self.published.insert(uri, diagnostics);
    }

    fn send_request(&mut self, method: &str, params: JsonValue) -> Result<JsonValue, String> {
        self.next_id += 1;
        let id = RequestId::from(self.next_id);
        self.send(Message::Request(Request {
            id: id.clone(),
            method: method.to_string(),
            params,
        }))?;
        self.await_response(id)
    }

    /// Reads rust-analyzer's `experimental/serverStatus`.
    ///
    /// `quiescent` is the server's own statement that VFS loading is done and
    /// no workspace fetch is in flight -- the only reliable readiness signal it
    /// offers. Guessing from silence instead makes the same request answer 29
    /// symbols or none depending on which indexing phase it lands between.
    fn track_server_status(&mut self, params: &JsonValue) {
        self.reports_status = true;
        if params
            .get("quiescent")
            .and_then(JsonValue::as_bool)
            .unwrap_or(false)
        {
            self.quiescent = true;
        }
        let health = params.get("health").and_then(JsonValue::as_str);
        if matches!(health, Some("error") | Some("warning")) {
            let message = params
                .get("message")
                .and_then(JsonValue::as_str)
                .unwrap_or("the server reported a degraded state");
            if let Ok(mut buffer) = self.complaints.lock() {
                buffer.spoke(message);
            }
        }
    }

    /// Keeps errors and warnings the server reports about itself. These carry
    /// the reason a workspace failed to load, which is otherwise invisible.
    fn record_complaint(&mut self, params: &JsonValue) {
        let severe = matches!(
            params.get("type").and_then(JsonValue::as_u64),
            Some(1) | Some(2)
        );
        if !severe {
            return;
        }
        let Some(message) = params.get("message").and_then(JsonValue::as_str) else {
            return;
        };
        let Ok(mut buffer) = self.complaints.lock() else {
            return;
        };
        buffer.spoke(message);
    }

    /// Follows `$/progress` so the bridge knows when the server is still
    /// loading a workspace rather than idle.
    fn track_progress(&mut self, params: &JsonValue) {
        let Some(token) = params.get("token").map(progress_token) else {
            return;
        };
        match params
            .get("value")
            .and_then(|value| value.get("kind"))
            .and_then(JsonValue::as_str)
        {
            Some("begin") => {
                self.working.insert(token);
            }
            Some("end") => {
                self.working.remove(&token);
            }
            _ => {}
        }
    }

    /// Waits for the server to finish its start-up work.
    ///
    /// A cold server answers instantly and emptily, so a hover looks like "no
    /// such thing" and a broken file looks clean. Readiness is taken from what
    /// each server documents:
    ///
    /// - rust-analyzer: `experimental/serverStatus` with `quiescent: true`,
    ///   which it only sends once the client declares
    ///   `experimental.serverStatusNotification`
    ///   (rust-analyzer docs/dev/lsp-extensions.md, "Server Status").
    /// - gopls and the rest: the LSP `$/progress` cycle, whose `kind: "end"`
    ///   marks the initial workspace load complete.
    ///
    /// Going quiet is the last-resort fallback, for servers that report
    /// neither -- it is a guess, and it is only reached when there is no
    /// signal to wait on.
    fn settle(&mut self, budget: Duration) {
        let deadline = Instant::now() + budget;
        loop {
            // A server that states its own readiness is believed over any
            // heuristic; one that never does is judged by going quiet.
            if self.reports_status && self.quiescent {
                return;
            }
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return;
            }
            let quiet = if self.working.is_empty() && !self.reports_status {
                SETTLED_QUIET_PERIOD.min(remaining)
            } else {
                remaining
            };
            match self.incoming.recv_timeout(quiet) {
                Ok(Message::Notification(notification)) => self.absorb(&notification),
                Ok(Message::Request(request)) => {
                    if self.decline(request).is_err() {
                        return;
                    }
                }
                Ok(Message::Response(_)) => {}
                Err(RecvTimeoutError::Timeout) => return,
                Err(RecvTimeoutError::Disconnected) => return,
            }
        }
    }

    /// Blocks until start-up work is done, once per server.
    fn ensure_warm(&mut self) {
        if self.warmed {
            return;
        }
        self.warmed = true;
        self.settle(WARMUP_BUDGET);
    }

    /// Answers a server-initiated request so it does not block waiting on us.
    ///
    /// happ is not an editor: it cannot show messages, apply edits or register
    /// capabilities, so the honest answer to most of these is an empty result.
    fn decline(&mut self, request: Request) -> Result<(), String> {
        let result = match request.method.as_str() {
            "workspace/configuration" => json!([JsonValue::Null]),
            "client/registerCapability" | "client/unregisterCapability" => JsonValue::Null,
            "window/workDoneProgress/create" => JsonValue::Null,
            "workspace/workspaceFolders" => JsonValue::Null,
            _ => JsonValue::Null,
        };
        self.send(Message::Response(Response {
            id: request.id,
            result: Some(result),
            error: None,
        }))
    }
}

impl LspProvider for ChildProvider {
    fn request(&mut self, method: &str, params: JsonValue) -> Result<JsonValue, String> {
        self.ensure_warm();
        let mut result = self.send_request(method, params.clone())?;

        // A server between indexing phases is silent but not ready, and answers
        // every question with nothing. Observed with rust-analyzer on a large
        // workspace: the same request returns 29 symbols or none depending on
        // timing. Retrying until it proves it can answer removes the race;
        // afterwards an empty answer is taken at face value.
        if !self.proven {
            for _ in 0..WARMUP_RETRIES {
                if !is_empty_result(&result) {
                    break;
                }
                self.settle(WARMUP_RETRY_PERIOD);
                result = self.send_request(method, params.clone())?;
            }
        }
        if !is_empty_result(&result) {
            self.proven = true;
        }
        Ok(result)
    }

    fn open(&mut self, path: &Path) -> Result<(), String> {
        let text = std::fs::read_to_string(path)
            .map_err(|err| format!("read {}: {err}", path.display()))?;
        let uri = path_to_uri(path)?;

        // Reopening a file the server already knows would be a protocol error;
        // a version bump through didChange is the way to refresh it.
        if let Some(version) = self.opened.get_mut(path) {
            *version += 1;
            let version = *version;
            return self.notify(
                "textDocument/didChange",
                json!({
                    "textDocument": { "uri": uri, "version": version },
                    "contentChanges": [{ "text": text }],
                }),
            );
        }

        self.opened.insert(path.to_path_buf(), 1);
        self.notify(
            "textDocument/didOpen",
            json!({
                "textDocument": {
                    "uri": uri,
                    "languageId": self.language_id,
                    "version": 1,
                    "text": text,
                },
            }),
        )
    }

    fn diagnostics(&mut self, path: &Path) -> Result<Vec<Diagnostic>, String> {
        self.open(path)?;
        self.ensure_warm();
        // Opening a file makes the server re-check it, so let that land too
        // before concluding the file is clean.
        self.settle(DIAGNOSTICS_QUIET_PERIOD);

        // Prefer a pull request when the server supports one: it answers
        // immediately instead of leaving us guessing how long to listen.
        if self.capabilities.get("diagnosticProvider").is_some() {
            let uri = path_to_uri(path)?;
            let result = self.request(
                "textDocument/diagnostic",
                json!({ "textDocument": { "uri": uri } }),
            )?;
            if let Some(items) = result.get("items").and_then(JsonValue::as_array) {
                return Ok(items.iter().map(decode_diagnostic).collect());
            }
        }

        // Otherwise wait for the server to push them, and treat silence as a
        // clean file rather than as a failure.
        let deadline = Instant::now() + DIAGNOSTICS_QUIET_PERIOD;
        loop {
            if let Some(found) = self.published.get(path) {
                return Ok(found.clone());
            }
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return Ok(Vec::new());
            }
            match self.incoming.recv_timeout(remaining) {
                Ok(Message::Notification(notification)) => self.absorb(&notification),
                Ok(Message::Request(request)) => self.decline(request)?,
                Ok(Message::Response(_)) => {}
                Err(RecvTimeoutError::Timeout) => return Ok(Vec::new()),
                Err(RecvTimeoutError::Disconnected) => {
                    return Err("language server exited".to_string())
                }
            }
        }
    }

    fn health(&self) -> Option<String> {
        self.complaints.lock().ok()?.summary()
    }

    fn supported_methods(&self) -> Vec<String> {
        let has = |key: &str| self.capabilities.get(key).is_some_and(|v| !v.is_null());
        let mut methods = Vec::new();
        for (capability, method) in [
            ("definitionProvider", "textDocument/definition"),
            ("referencesProvider", "textDocument/references"),
            ("hoverProvider", "textDocument/hover"),
            ("documentSymbolProvider", "textDocument/documentSymbol"),
            ("workspaceSymbolProvider", "workspace/symbol"),
            ("implementationProvider", "textDocument/implementation"),
            ("typeDefinitionProvider", "textDocument/typeDefinition"),
            ("callHierarchyProvider", "textDocument/prepareCallHierarchy"),
        ] {
            if has(capability) {
                methods.push(method.to_string());
            }
        }
        methods.push("textDocument/publishDiagnostics".to_string());
        methods
    }
}

impl Drop for ChildProvider {
    fn drop(&mut self) {
        // Best effort: ask politely, then make sure it is gone. A language
        // server outliving happ would keep indexing a project nobody is reading.
        let _ = self.request("shutdown", JsonValue::Null);
        let _ = self.notify("exit", JsonValue::Null);
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// Whether a response carries no information -- the shape a language server
/// returns while it is still loading.
fn is_empty_result(result: &JsonValue) -> bool {
    match result {
        JsonValue::Null => true,
        JsonValue::Array(items) => items.is_empty(),
        JsonValue::Object(map) => map.is_empty(),
        _ => false,
    }
}

/// Progress tokens are either a string or a number; both identify one task.
fn progress_token(token: &JsonValue) -> String {
    match token {
        JsonValue::String(text) => text.clone(),
        other => other.to_string(),
    }
}

fn decode_diagnostic(raw: &JsonValue) -> Diagnostic {
    let (line, column) = super::from_lsp_position(
        raw.get("range")
            .and_then(|range| range.get("start"))
            .unwrap_or(&JsonValue::Null),
    );
    Diagnostic {
        line,
        column,
        severity: severity_label(raw.get("severity").and_then(JsonValue::as_u64)),
        code: raw.get("code").and_then(|code| match code {
            JsonValue::String(text) => Some(text.clone()),
            JsonValue::Number(number) => Some(number.to_string()),
            _ => None,
        }),
        message: raw
            .get("message")
            .and_then(JsonValue::as_str)
            .unwrap_or_default()
            .to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn diagnostics_decode_to_one_based_positions() {
        let decoded = decode_diagnostic(&json!({
            "range": { "start": { "line": 4, "character": 2 } },
            "severity": 1,
            "code": "E0308",
            "message": "mismatched types",
        }));
        assert_eq!((decoded.line, decoded.column), (5, 3));
        assert_eq!(decoded.severity, "error");
        assert_eq!(decoded.code.as_deref(), Some("E0308"));
    }

    #[test]
    fn a_numeric_diagnostic_code_survives_as_text() {
        let decoded = decode_diagnostic(&json!({
            "range": { "start": { "line": 0, "character": 0 } },
            "code": 2345,
            "message": "argument mismatch",
        }));
        assert_eq!(decoded.code.as_deref(), Some("2345"));
        assert_eq!(decoded.severity, "info", "absent severity defaults to info");
    }

    #[test]
    fn a_server_that_refuses_to_start_has_its_own_explanation_reported() {
        // Stands in for the common real case: a rustup or npm shim that exists
        // on PATH, prints why it cannot run, and exits. Without the captured
        // stderr this is indistinguishable from a server that simply hung.
        let spec = super::super::registry::spec_for_language("go").expect("go spec");
        let err = ChildProvider::start(
            spec,
            &[
                "sh".to_string(),
                "-c".to_string(),
                "echo 'component not installed' >&2; exit 1".to_string(),
            ],
            Path::new("."),
        )
        .err()
        .expect("start must fail");
        assert!(
            err.contains("component not installed"),
            "the server's own reason must survive: {err}"
        );
    }

    /// Observed against rust-analyzer 1.94.0: one `code` call about a clean
    /// file returned seventy identical log lines, which the caller pays for.
    #[test]
    fn a_repeated_complaint_is_kept_once_with_its_count() {
        let mut complaints = Complaints::default();
        for _ in 0..70 {
            complaints.spoke("inference diagnostic in desugared expr");
        }
        let summary = complaints.summary().expect("a complaint was recorded");
        assert_eq!(
            summary.matches("inference diagnostic").count(),
            1,
            "{summary}"
        );
        assert!(summary.contains("repeated 70 times"), "{summary}");
    }

    #[test]
    fn a_long_complaint_keeps_its_opening_lines() {
        let mut complaints = Complaints::default();
        complaints.spoke(
            "error: rustc 1.93.1 is not supported\n  zq@1.5.1 requires rustc 1.94\nthree\nfour\nfive\nsix",
        );
        let summary = complaints.summary().expect("a complaint was recorded");
        assert!(
            summary.contains("zq@1.5.1 requires rustc 1.94"),
            "{summary}"
        );
        assert!(summary.contains("(2 more lines)"), "{summary}");
    }

    #[test]
    fn distinct_complaints_are_capped_and_the_rest_are_counted() {
        let mut complaints = Complaints::default();
        for n in 0..MAX_DISTINCT_COMPLAINTS * 2 {
            complaints.spoke(&format!("problem {n}"));
        }
        let summary = complaints.summary().expect("complaints were recorded");
        assert!(summary.contains("problem 0"), "{summary}");
        assert!(
            !summary.contains("problem 40"),
            "the cap must hold: {summary}"
        );
        assert!(
            summary.contains(&format!(
                "{} further distinct messages",
                MAX_DISTINCT_COMPLAINTS - COMPLAINTS_SHOWN
            )),
            "{summary}"
        );
    }

    #[test]
    fn a_silent_server_has_nothing_to_complain_about() {
        assert!(Complaints::default().summary().is_none());
        let mut complaints = Complaints::default();
        complaints.spoke("   \n  ");
        assert!(
            complaints.summary().is_none(),
            "blank lines are not complaints"
        );
    }

    /// `window/logMessage` is what LSP specifies for a server to tell its client
    /// something; stderr is whatever the vendor felt like printing.
    #[test]
    fn what_the_server_said_outranks_what_it_printed() {
        let mut complaints = Complaints::default();
        complaints.printed("2026-07-29T01:36:33.568701+03:00 ERROR internal log noise");
        assert!(
            complaints
                .summary()
                .expect("stderr is all there is so far")
                .contains("internal log noise"),
            "stderr must still explain a server that never spoke"
        );

        complaints.spoke("cannot load the workspace: Cargo.toml is malformed");
        let summary = complaints.summary().expect("the server spoke");
        assert!(summary.contains("Cargo.toml is malformed"), "{summary}");
        assert!(
            !summary.contains("internal log noise"),
            "the spoken channel must not be diluted by the printed one: {summary}"
        );
    }

    #[test]
    fn starting_a_server_that_is_not_installed_names_the_command() {
        let spec = super::super::registry::spec_for_language("go").expect("go spec");
        let err = ChildProvider::start(
            spec,
            &["happ-no-such-language-server".to_string()],
            Path::new("."),
        )
        .err()
        .expect("start must fail");
        assert!(err.contains("happ-no-such-language-server"), "{err}");
    }
}
