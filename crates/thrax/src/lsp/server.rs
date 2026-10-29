//! The server loop and its request handlers.
//!
//! One thread reads stdin and forwards each message; the main thread owns every
//! document and analysis (the checker is single-threaded). Edits are coalesced:
//! a burst of `didChange` is checked once, after `lsp.debounce_ms` of quiet.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{mpsc, Arc};
use std::time::Duration;

use utilities::config::{Config, Value};
use utilities::{Applicability, Code, Diagnostic, Json, Severity, Span};

use super::analysis::{self, Snapshot};
use super::clock::Instant;
use super::text::{Document, Encoding, Position};
use crate::highlight::{self, Class};
use super::{path_to_uri, read_message, uri_to_path, write_message};

/// LSP `DiagnosticSeverity`.
const ERROR: i64 = 1;
const WARNING: i64 = 2;
const INFORMATION: i64 = 3;
const HINT: i64 = 4;

/// The most uncertain fixes tried per code-action request.
const MAX_FIX_TRIALS: usize = 8;

/// The most errors replaced by a hole to find which one matters most.
const MAX_RANK_TRIALS: usize = 8;

/// LSP `DiagnosticTag.Unnecessary`: rendered faded.
const UNNECESSARY: i64 = 1;

struct Open {
    doc: Document,
    path: PathBuf,
    /// The latest analysis and the document version it saw.
    snapshot: Option<(i64, Snapshot)>,
    /// Where and when the document was last typed into.
    last_edit: Option<(usize, Instant)>,
    /// Keeps the document's checked dependencies between edits.
    analyzer: analysis::Analyzer,
    /// What the last save's compile-time run reported, for the version saved.
    saved: Option<(i64, Vec<Diagnostic>)>,
    /// The error that counterfactual ranking found matters most, for the
    /// version ranked: its span and how many other errors go with it.
    focus: Option<(i64, Span, usize)>,
    /// Why the last analysis could not load the document's modules, for the
    /// version analyzed, when that has a place in the document.
    failure: Option<(i64, Diagnostic)>,
}

pub struct Server {
    enc: Encoding,
    open: HashMap<String, Open>,
    /// Documents to check once `due` passes, the one typed into first.
    dirty: Vec<String>,
    due: Option<Instant>,
    /// Documents whose held-back errors are due to be shown.
    grace_due: Vec<(String, Instant)>,
    /// Documents to rank counterfactually once typing pauses.
    rank_due: Vec<(String, Instant)>,
    /// Settings the client sent, re-applied over each file's `thrax.toml`.
    client: Json,
    shutting_down: bool,
    out: Sink,
    /// Sources imports resolve against as if they were files, without being
    /// open or checked themselves (see [`Server::provide`]).
    provided: HashMap<PathBuf, String>,
    /// Messages the reader has received, and how many of them were handled:
    /// when they differ, a message is waiting and analysis should yield to it.
    arrived: Arc<AtomicUsize>,
    handled: usize,
}

/// Where outgoing messages go.
enum Sink {
    /// Framed onto a stream, the stdio transport.
    Stream(Box<dyn std::io::Write>),
    /// Kept for an embedder to take with [`Server::take_outbox`], when the
    /// server is driven one call at a time (the browser playground).
    Queue(Vec<Json>),
}

/// The protocol's channel: the original stdout. Compile-time code the save
/// check runs may print, and anything printed to stdout would corrupt the
/// protocol, so fd 1 is pointed at stderr (the editor's log) for the server's
/// lifetime and messages go to a duplicate of the original.
#[cfg(unix)]
fn protocol_output() -> Box<dyn std::io::Write> {
    use std::os::fd::FromRawFd;
    extern "C" {
        fn dup(fd: i32) -> i32;
        fn dup2(from: i32, to: i32) -> i32;
    }
    // SAFETY: plain descriptor calls; `protocol` is a fresh descriptor this
    // process owns, handed to exactly one `File`.
    unsafe {
        let protocol = dup(1);
        if protocol < 0 || dup2(2, 1) < 0 {
            return Box::new(std::io::stdout());
        }
        Box::new(std::fs::File::from_raw_fd(protocol))
    }
}

#[cfg(not(unix))]
fn protocol_output() -> Box<dyn std::io::Write> {
    Box::new(std::io::stdout())
}

pub fn run() -> std::process::ExitCode {
    let (tx, rx) = mpsc::channel::<Option<Json>>();
    let arrived = Arc::new(AtomicUsize::new(0));
    let counter = Arc::clone(&arrived);
    std::thread::spawn(move || {
        let stdin = std::io::stdin();
        let mut input = stdin.lock();
        loop {
            match read_message(&mut input) {
                Ok(Some(msg)) => {
                    counter.fetch_add(1, Ordering::SeqCst);
                    if tx.send(Some(msg)).is_err() {
                        return;
                    }
                }
                _ => {
                    let _ = tx.send(None);
                    return;
                }
            }
        }
    });
    let mut server = Server::new(Sink::Stream(protocol_output()), arrived);
    loop {
        let wake = server.next_wake();
        let msg = match wake {
            Some(at) => match rx.recv_timeout(at.saturating_duration_since(Instant::now())) {
                Ok(m) => Some(m),
                Err(mpsc::RecvTimeoutError::Timeout) => None,
                Err(mpsc::RecvTimeoutError::Disconnected) => return std::process::ExitCode::FAILURE,
            },
            None => match rx.recv() {
                Ok(m) => Some(m),
                Err(_) => return std::process::ExitCode::FAILURE,
            },
        };
        let mut batch = Vec::new();
        if let Some(m) = msg {
            batch.push(m);
        }
        while let Ok(m) = rx.try_recv() {
            batch.push(m);
        }
        for m in batch {
            let Some(m) = m else {
                return std::process::ExitCode::FAILURE;
            };
            server.handled += 1;
            if let Some(code) = server.handle(m) {
                return code;
            }
        }
        server.tick();
    }
}

impl Server {
    fn new(out: Sink, arrived: Arc<AtomicUsize>) -> Server {
        Server {
            enc: Encoding::Utf16,
            open: HashMap::new(),
            dirty: Vec::new(),
            due: None,
            grace_due: Vec::new(),
            rank_due: Vec::new(),
            client: Json::Null,
            shutting_down: false,
            out,
            provided: HashMap::new(),
            arrived,
            handled: 0,
        }
    }

    /// A server with no transport: the embedder passes each message to
    /// [`Server::handle`], calls [`Server::tick`] when [`Server::wake_in`] says,
    /// and collects what the server sent with [`Server::take_outbox`].
    pub fn queued() -> Server {
        Server::new(Sink::Queue(Vec::new()), Arc::new(AtomicUsize::new(0)))
    }

    /// Make `text` the source of `path` for import resolution, where there is
    /// no file to read: a module the embedder supplies, like the browser
    /// playground's `HOST`. Unlike an open document it is never checked or
    /// reported on.
    pub fn provide(&mut self, path: PathBuf, text: String) {
        self.provided.insert(path, text);
    }

    /// The messages sent since the last call, oldest first. Empty for a
    /// stdio server, which writes them as it goes.
    pub fn take_outbox(&mut self) -> Vec<Json> {
        match &mut self.out {
            Sink::Queue(q) => std::mem::take(q),
            Sink::Stream(_) => Vec::new(),
        }
    }

    /// How long until [`Server::tick`] has work, or `None` while it has none.
    pub fn wake_in(&self) -> Option<Duration> {
        self.next_wake().map(|at| at.saturating_duration_since(Instant::now()))
    }

    fn config(&self, path: &Path) -> Config {
        let (mut config, _) = Config::for_path(path);
        if let Json::Obj(sections) = &self.client {
            for (section, keys) in sections {
                let Json::Obj(keys) = keys else { continue };
                for (key, v) in keys {
                    let value = match v {
                        Json::Bool(b) => Value::Bool(*b),
                        Json::Num(n) => Value::Int(*n as i64),
                        Json::Str(s) => Value::Str(s.clone()),
                        _ => continue,
                    };
                    let _ = config.set(section, key, value);
                }
            }
        }
        config
    }

    fn next_wake(&self) -> Option<Instant> {
        let later = self.grace_due.iter().chain(&self.rank_due).map(|(_, at)| *at);
        self.due.into_iter().chain(later).min()
    }

    fn send(&mut self, msg: Json) {
        match &mut self.out {
            Sink::Stream(out) => {
                let _ = write_message(out, &msg);
            }
            Sink::Queue(q) => q.push(msg),
        }
    }

    fn reply(&mut self, id: &Json, result: Json) {
        self.send(Json::obj([("jsonrpc", Json::str("2.0")), ("id", id.clone()), ("result", result)]));
    }

    fn reply_error(&mut self, id: &Json, code: i64, message: &str) {
        let error = Json::obj([("code", Json::int(code)), ("message", Json::str(message))]);
        self.send(Json::obj([("jsonrpc", Json::str("2.0")), ("id", id.clone()), ("error", error)]));
    }

    fn notify(&mut self, method: &str, params: Json) {
        self.send(Json::obj([("jsonrpc", Json::str("2.0")), ("method", Json::str(method)), ("params", params)]));
    }

    /// Handle one message; `Some(code)` ends the server.
    pub fn handle(&mut self, msg: Json) -> Option<std::process::ExitCode> {
        let method = msg.get("method").as_str().unwrap_or("").to_string();
        let id = msg.get("id").clone();
        let params = msg.get("params");
        if self.shutting_down && method != "exit" && !id.is_null() {
            self.reply_error(&id, -32600, "the server is shutting down");
            return None;
        }
        match method.as_str() {
            "initialize" => {
                let result = self.initialize(params);
                self.reply(&id, result);
            }
            "initialized" | "$/cancelRequest" | "$/setTrace" => {}
            "shutdown" => {
                self.shutting_down = true;
                self.reply(&id, Json::Null);
            }
            "exit" => {
                return Some(if self.shutting_down {
                    std::process::ExitCode::SUCCESS
                } else {
                    std::process::ExitCode::FAILURE
                });
            }
            "workspace/didChangeConfiguration" => {
                let settings = params.get("settings");
                self.client = match settings.get("thrax") {
                    Json::Null => settings.clone(),
                    t => t.clone(),
                };
                self.mark_all_dirty(Duration::ZERO);
            }
            "workspace/didChangeWatchedFiles" => self.mark_all_dirty(Duration::ZERO),
            "textDocument/didOpen" => self.did_open(params),
            "textDocument/didChange" => self.did_change(params),
            "textDocument/didSave" => self.did_save(params),
            "textDocument/didClose" => {
                if let Some(uri) = params.get("textDocument").get("uri").as_str() {
                    let uri = uri.to_string();
                    self.open.remove(&uri);
                    self.dirty.retain(|u| *u != uri);
                    self.notify(
                        "textDocument/publishDiagnostics",
                        Json::obj([("uri", Json::str(uri)), ("diagnostics", Json::Arr(Vec::new()))]),
                    );
                }
            }
            "textDocument/codeAction" => {
                let result = self.code_actions(params);
                self.reply(&id, result);
            }
            "textDocument/hover" => {
                let result = self.hover(params);
                self.reply(&id, result);
            }
            "textDocument/definition" => {
                let result = self.definition(params);
                self.reply(&id, result);
            }
            "textDocument/completion" => {
                let result = self.completion(params);
                self.reply(&id, result);
            }
            "textDocument/semanticTokens/full" => {
                let result = self.semantic_tokens(params);
                self.reply(&id, result);
            }
            "textDocument/documentSymbol" => {
                let result = self.document_symbols(params);
                self.reply(&id, result);
            }
            "textDocument/formatting" | "textDocument/rangeFormatting" => {
                let result = self.formatting(params, method == "textDocument/rangeFormatting");
                self.reply(&id, result);
            }
            _ if !id.is_null() => self.reply_error(&id, -32601, &format!("method not found: {method}")),
            _ => {}
        }
        None
    }

    fn initialize(&mut self, params: &Json) -> Json {
        let offered = params.get("capabilities").get("general").get("positionEncodings");
        let has = |name: &str| offered.as_arr().iter().any(|e| e.as_str() == Some(name));
        self.enc = if has("utf-8") {
            Encoding::Utf8
        } else if has("utf-32") {
            Encoding::Utf32
        } else {
            Encoding::Utf16
        };
        let opts = params.get("initializationOptions");
        if !opts.is_null() {
            self.client = opts.clone();
        }
        let sync = Json::obj([
            ("openClose", Json::Bool(true)),
            ("change", Json::int(2)),
            ("save", Json::obj([("includeText", Json::Bool(false))])),
        ]);
        let capabilities = Json::obj([
            ("positionEncoding", Json::str(self.enc.name())),
            ("textDocumentSync", sync),
            ("hoverProvider", Json::Bool(true)),
            ("definitionProvider", Json::Bool(true)),
            ("documentSymbolProvider", Json::Bool(true)),
            (
                "semanticTokensProvider",
                Json::obj([
                    (
                        "legend",
                        Json::obj([
                            ("tokenTypes", Json::Arr(Class::ALL.iter().map(|c| Json::str(c.lsp_name())).collect())),
                            ("tokenModifiers", Json::Arr(Vec::new())),
                        ]),
                    ),
                    ("full", Json::Bool(true)),
                ]),
            ),
            ("completionProvider", Json::obj([("triggerCharacters", Json::Arr(vec![Json::str(".")]))])),
            ("documentFormattingProvider", Json::Bool(true)),
            ("documentRangeFormattingProvider", Json::Bool(true)),
            (
                "codeActionProvider",
                Json::obj([("codeActionKinds", Json::Arr(vec![Json::str("quickfix"), Json::str("source.fixAll")]))]),
            ),
        ]);
        Json::obj([
            ("capabilities", capabilities),
            ("serverInfo", Json::obj([("name", Json::str("thrax")), ("version", Json::str(env!("CARGO_PKG_VERSION")))])),
        ])
    }

    fn did_open(&mut self, params: &Json) {
        let td = params.get("textDocument");
        let (Some(uri), Some(text)) = (td.get("uri").as_str(), td.get("text").as_str()) else {
            return;
        };
        let Some(path) = uri_to_path(uri) else { return };
        let version = td.get("version").as_i64().unwrap_or(0);
        self.open.insert(
            uri.to_string(),
            Open {
                doc: Document::new(text.to_string(), version),
                path,
                snapshot: None,
                last_edit: None,
                analyzer: analysis::Analyzer::default(),
                saved: None,
                focus: None,
                failure: None,
            },
        );
        self.mark_dirty(uri, Duration::ZERO);
    }

    fn did_change(&mut self, params: &Json) {
        let td = params.get("textDocument");
        let Some(uri) = td.get("uri").as_str() else { return };
        let enc = self.enc;
        let Some(open) = self.open.get_mut(uri) else { return };
        let mut at = 0;
        for change in params.get("contentChanges").as_arr() {
            let text = change.get("text").as_str().unwrap_or("");
            let range = change.get("range");
            let range = if range.is_null() {
                None
            } else {
                Some((position(range.get("start")), position(range.get("end"))))
            };
            if let Some((start, _)) = range {
                at = open.doc.offset(start, enc);
            }
            open.doc.edit(range, text, enc);
        }
        open.doc.version = td.get("version").as_i64().unwrap_or(open.doc.version + 1);
        open.last_edit = Some((at, Instant::now()));
        let path = open.path.clone();
        let config = self.config(&path);
        if config.word("lsp", "check") == "type" {
            let wait = Duration::from_millis(config.int("lsp", "debounce_ms") as u64);
            self.mark_dirty(uri, wait);
        }
    }

    fn did_save(&mut self, params: &Json) {
        let Some(uri) = params.get("textDocument").get("uri").as_str() else { return };
        let uri = uri.to_string();
        if let Some(open) = self.open.get_mut(&uri) {
            open.last_edit = None;
            let name = open.snapshot.as_ref().map(|(_, s)| s.sources[s.root].0.clone());
            let started = Instant::now();
            let found = crate::driver::compile_time_diagnostics(&open.path.display().to_string());
            let mine: Vec<Diagnostic> = found
                .into_iter()
                .filter(|(m, _)| name.as_deref().is_none_or(|n| n == m))
                .map(|(_, d)| d)
                .collect();
            open.saved = Some((open.doc.version, mine));
            let msg = format!("thrax: ran compile-time code of {} in {:.1} ms", open.path.display(), started.elapsed().as_secs_f64() * 1000.0);
            self.notify("window/logMessage", Json::obj([("type", Json::int(4)), ("message", Json::str(msg))]));
        }
        self.mark_dirty(&uri, Duration::ZERO);
        // Others may import the saved file.
        let others: Vec<String> = self.open.keys().filter(|u| **u != uri).cloned().collect();
        for u in others {
            self.mark_dirty(&u, Duration::from_millis(50));
        }
    }

    fn mark_dirty(&mut self, uri: &str, wait: Duration) {
        if !self.dirty.iter().any(|u| u == uri) {
            self.dirty.push(uri.to_string());
        }
        let at = Instant::now() + wait;
        self.due = Some(self.due.map_or(at, |d| d.max(at)));
    }

    fn mark_all_dirty(&mut self, wait: Duration) {
        let uris: Vec<String> = self.open.keys().cloned().collect();
        for u in uris {
            self.mark_dirty(&u, wait);
        }
    }

    /// Run what is due: checks of dirty documents (stopping early when new
    /// messages arrive, so typing is never queued behind analysis), then any
    /// held-back errors whose grace ran out.
    pub fn tick(&mut self) {
        let now = Instant::now();
        if self.due.is_some_and(|d| d <= now) {
            self.due = None;
            while !self.dirty.is_empty() {
                let uri = self.dirty.remove(0);
                self.analyze(&uri);
                self.publish(&uri);
                self.schedule_rank(&uri);
                if !self.dirty.is_empty() && self.arrived.load(Ordering::SeqCst) > self.handled {
                    self.due = Some(Instant::now());
                    break;
                }
            }
        }
        let now = Instant::now();
        let ready: Vec<String> = self.grace_due.iter().filter(|(_, at)| *at <= now).map(|(u, _)| u.clone()).collect();
        self.grace_due.retain(|(_, at)| *at > now);
        for uri in ready {
            self.publish(&uri);
        }
        let now = Instant::now();
        let ready: Vec<String> = self.rank_due.iter().filter(|(_, at)| *at <= now).map(|(u, _)| u.clone()).collect();
        self.rank_due.retain(|(_, at)| *at > now);
        for uri in ready {
            if self.dirty.is_empty() && self.rank(&uri) {
                self.publish(&uri);
            }
        }
    }

    /// Rank `uri`'s errors once typing has paused, if there are several.
    fn schedule_rank(&mut self, uri: &str) {
        let Some(open) = self.open.get(uri) else { return };
        let Some((_, snap)) = &open.snapshot else { return };
        let errors = snap.found.iter().filter(|f| f.diag.severity() == Severity::Error).count();
        if errors < 2 {
            return;
        }
        let wait = Duration::from_millis(self.config(&open.path).int("lsp", "grace_ms") as u64 / 3);
        self.rank_due.retain(|(u, _)| u != uri);
        self.rank_due.push((uri.to_string(), Instant::now() + wait));
    }

    /// Counterfactual ranking: put a hole where each error is and re-check; the
    /// error whose hole clears the most other errors is the focus. Reports
    /// whether it found one.
    fn rank(&mut self, uri: &str) -> bool {
        let Some(open) = self.open.get_mut(uri) else { return false };
        let Some((version, snap)) = &open.snapshot else { return false };
        if *version != open.doc.version {
            return false;
        }
        let src = &snap.sources[snap.root].2;
        let errors: Vec<&analysis::Found> = snap
            .found
            .iter()
            .filter(|f| f.diag.severity() == Severity::Error && !f.diag.root().span.is_empty() && !is_syntax(f.diag.root().code))
            .take(MAX_RANK_TRIALS)
            .collect();
        let before = snap.found.iter().filter(|f| f.diag.severity() == Severity::Error).count();
        let mut best: Option<(Span, usize)> = None;
        for f in errors {
            let span = f.diag.root().span;
            let holed = utilities::error::apply_edits(src, &[utilities::Edit::new(span, frontend::lexer::HOLE)]);
            let after = analysis::trial(snap, &holed).iter().filter(|d| d.root().code != Code::TypedHole).count();
            let cleared = (before - 1).saturating_sub(after);
            if cleared > 0 && best.is_none_or(|(_, c)| cleared > c) {
                best = Some((span, cleared));
            }
        }
        let version = *version;
        open.focus = best.map(|(span, n)| (version, span, n));
        open.focus.is_some()
    }

    fn analyze(&mut self, uri: &str) {
        let overlay: HashMap<PathBuf, String> = self
            .provided
            .iter()
            .map(|(p, t)| (p.clone(), t.clone()))
            .chain(self.open.values().map(|o| (o.path.clone(), o.doc.text.clone())))
            .collect();
        let Some(open) = self.open.get_mut(uri) else { return };
        let (path, version) = (open.path.clone(), open.doc.version);
        // The new check rewinds the types the old snapshot reads, so it goes first.
        open.snapshot = None;
        let started = Instant::now();
        let result = open.analyzer.analyze(&path, &overlay);
        let took = started.elapsed();
        open.failure = match &result {
            Ok(_) => None,
            Err(e) => e.diagnostic.clone().map(|d| (version, d)),
        };
        match result {
            Ok(snapshot) => {
                let msg = format!(
                    "thrax: checked {} ({} modules) in {:.1} ms",
                    path.display(),
                    snapshot.sources.len(),
                    took.as_secs_f64() * 1000.0
                );
                self.notify("window/logMessage", Json::obj([("type", Json::int(4)), ("message", Json::str(msg))]));
                if let Some(open) = self.open.get_mut(uri) {
                    open.snapshot = Some((version, snapshot));
                }
            }
            Err(msg) => {
                let msg = format!("thrax: {msg}");
                self.notify("window/logMessage", Json::obj([("type", Json::int(1)), ("message", Json::str(msg))]));
            }
        }
    }

    /// A fresh analysis of `uri` for a request about its current text.
    fn current(&mut self, uri: &str) -> Option<&Snapshot> {
        let stale = match self.open.get(uri) {
            Some(o) => o.snapshot.as_ref().is_none_or(|(v, _)| *v != o.doc.version),
            None => return None,
        };
        if stale {
            self.analyze(uri);
            self.publish(uri);
        }
        self.open.get(uri)?.snapshot.as_ref().map(|(_, s)| s)
    }

    fn publish(&mut self, uri: &str) {
        let Some(open) = self.open.get(uri) else { return };
        let Some((_, snap)) = &open.snapshot else {
            self.publish_failure(uri);
            return;
        };
        let config = self.config(&open.path);
        let Some(module) = snap.module_of(&open.path).or(Some(snap.root)) else { return };
        let src = &snap.sources[module].2;
        let doc = Document::new(src.clone(), 0);
        let held = held_region(snap, module, open, &config);
        let saved: Vec<analysis::Found> = match &open.saved {
            Some((v, ds)) if *v == open.doc.version => ds
                .iter()
                .filter(|d| {
                    !snap.found.iter().any(|f| f.diag.root().span == d.root().span && f.diag.root().code == d.root().code)
                })
                .enumerate()
                .map(|(i, d)| analysis::Found { module, diag: d.clone(), rank: 2_000_000 + i })
                .collect(),
            _ => Vec::new(),
        };
        let mut found: Vec<&analysis::Found> =
            snap.found.iter().filter(|f| f.module == module).chain(saved.iter()).collect();
        found.sort_by_key(|f| f.rank);
        found.truncate(config.int("lsp", "max_diagnostics") as usize);
        let lint = config.word("lints", "unreachable_arm").to_string();
        let focus = config.word("lsp", "focus").to_string();

        // Definitions that failed, for spotting errors that follow from them.
        let broken: Vec<(String, Span)> = found
            .iter()
            .filter(|f| f.diag.severity() == Severity::Error)
            .filter_map(|f| enclosing_def(snap, module, f.diag.root().span).map(|n| (n, f.diag.root().span)))
            .collect();

        let mut out = Vec::new();
        let ranked = open.focus.filter(|(v, _, _)| *v == open.doc.version).map(|(_, span, n)| (span, n));
        let mut focus_taken = false;
        for f in &found {
            let d = &f.diag;
            let span = d.root().span;
            let mut severity = match d.severity() {
                Severity::Error => ERROR,
                Severity::Warning => WARNING,
                Severity::Info => INFORMATION,
                Severity::Hint => HINT,
            };
            let mut role = if severity == ERROR { "root" } else { "lint" };
            let mut tags = Vec::new();
            let mut related = Vec::new();
            if d.root().code == Code::UnreachableArm {
                match lint.as_str() {
                    "allow" => continue,
                    "deny" => severity = ERROR,
                    _ => {}
                }
                tags.push(Json::int(UNNECESSARY));
            }
            if severity == ERROR {
                let own = enclosing_def(snap, module, span);
                let cause = broken.iter().find(|(name, at)| {
                    Some(name) != own.as_ref() && *at != span && mentions(src, item_extent(snap, module, span), name)
                });
                if let (Some((name, at)), true) = (cause, focus != "all") {
                    role = "consequence";
                    severity = INFORMATION;
                    related.push(related_info(uri, &doc, *at, &format!("`{name}` fails to check here"), self.enc));
                }
            }
            if severity == ERROR && held.is_some_and(|(lo, hi)| lo <= span.start && span.start <= hi) {
                role = "grace";
                severity = HINT;
            }
            let mut text = message(d);
            if severity == ERROR {
                let is_focus = match ranked {
                    Some((at, _)) => at == span,
                    None => !focus_taken,
                };
                if is_focus && !focus_taken {
                    focus_taken = true;
                    role = "focus";
                    if let Some((_, n)) = ranked {
                        text.push_str(&format!("\n(fixing this may also clear {n} other error(s))"));
                    }
                } else if focus == "one" {
                    severity = WARNING;
                }
            }
            for l in d.labels() {
                related.push(related_info(uri, &doc, l.span, &l.msg, self.enc));
            }
            for frame in &d.frames()[1..] {
                if frame.span != Span::at(0) {
                    related.push(related_info(uri, &doc, frame.span, &frame.msg, self.enc));
                }
            }
            let mut diag = Json::obj([
                ("range", range(&doc, span, self.enc)),
                ("severity", Json::int(severity)),
                ("code", Json::str(d.root().code.name())),
                ("source", Json::str("thrax")),
                ("message", Json::str(text)),
                ("data", Json::obj([("rank", Json::int(f.rank as i64)), ("role", Json::str(role))])),
            ]);
            if !tags.is_empty() {
                diag.set("tags", Json::Arr(tags));
            }
            if !related.is_empty() {
                diag.set("relatedInformation", Json::Arr(related));
            }
            out.push(diag);
        }
        if let (Some((_, at)), true) = (open.last_edit, held.is_some()) {
            let wake = at + Duration::from_millis(config.int("lsp", "grace_ms") as u64);
            let uri = uri.to_string();
            self.grace_due.retain(|(u, _)| *u != uri);
            self.grace_due.push((uri, wake));
        }
        let version = open.snapshot.as_ref().map_or(0, |(v, _)| *v);
        self.notify(
            "textDocument/publishDiagnostics",
            Json::obj([("uri", Json::str(uri)), ("version", Json::int(version)), ("diagnostics", Json::Arr(out))]),
        );
    }

    /// A document whose modules cannot be loaded has no snapshot, only the
    /// load failure, which is its one diagnostic.
    fn publish_failure(&mut self, uri: &str) {
        let Some(open) = self.open.get(uri) else { return };
        let Some((version, d)) = &open.failure else { return };
        let diag = Json::obj([
            ("range", range(&open.doc, d.root().span, self.enc)),
            ("severity", Json::int(ERROR)),
            ("code", Json::str(d.root().code.name())),
            ("source", Json::str("thrax")),
            ("message", Json::str(message(d))),
            ("data", Json::obj([("rank", Json::int(0)), ("role", Json::str("focus"))])),
        ]);
        let version = *version;
        self.notify(
            "textDocument/publishDiagnostics",
            Json::obj([("uri", Json::str(uri)), ("version", Json::int(version)), ("diagnostics", Json::Arr(vec![diag]))]),
        );
    }

    /// The fixes a load failure carries (a misspelt import's `did you mean`),
    /// offered as they are: with nothing loaded there is nothing to try them on.
    fn failure_actions(&self, uri: &str, lo: usize, hi: usize) -> Json {
        let Some(open) = self.open.get(uri) else { return Json::Arr(Vec::new()) };
        let Some((_, d)) = open.failure.as_ref().filter(|(v, _)| *v == open.doc.version) else {
            return Json::Arr(Vec::new());
        };
        let span = d.root().span;
        if !(span.start <= hi && lo <= span.end) {
            return Json::Arr(Vec::new());
        }
        let mut actions = Vec::new();
        for help in d.helps() {
            let Some(edits) = help.located_edits() else { continue };
            let text_edits: Vec<Json> = edits
                .iter()
                .map(|e| Json::obj([("range", range(&open.doc, e.span, self.enc)), ("newText", Json::str(e.text.clone()))]))
                .collect();
            let mut changes = Json::Obj(Vec::new());
            changes.set(uri, Json::Arr(text_edits));
            actions.push(Json::obj([
                ("title", Json::str(help.msg.clone())),
                ("kind", Json::str("quickfix")),
                ("isPreferred", Json::Bool(help.applicability == Applicability::MachineApplicable)),
                ("diagnostics", Json::Arr(vec![Json::obj([
                    ("range", range(&open.doc, span, self.enc)),
                    ("message", Json::str(message(d))),
                    ("code", Json::str(d.root().code.name())),
                ])])),
                ("edit", Json::obj([("changes", changes)])),
            ]));
        }
        Json::Arr(actions)
    }

    fn code_actions(&mut self, params: &Json) -> Json {
        let Some(uri) = params.get("textDocument").get("uri").as_str().map(str::to_string) else {
            return Json::Arr(Vec::new());
        };
        let enc = self.enc;
        let r = params.get("range");
        let only: Vec<String> = params.get("context").get("only").as_arr().iter().filter_map(|k| k.as_str().map(str::to_string)).collect();
        let wants = |kind: &str| only.is_empty() || only.iter().any(|o| kind.starts_with(o.as_str()));
        let Some(open) = self.open.get(&uri) else { return Json::Arr(Vec::new()) };
        let (lo, hi) = (open.doc.offset(position(r.get("start")), enc), open.doc.offset(position(r.get("end")), enc));
        let path = open.path.clone();
        if self.current(&uri).is_none() {
            return if wants("quickfix") { self.failure_actions(&uri, lo, hi) } else { Json::Arr(Vec::new()) };
        }
        let Some(snap) = self.current(&uri) else { return Json::Arr(Vec::new()) };
        let Some(module) = snap.module_of(&path) else { return Json::Arr(Vec::new()) };
        let doc = Document::new(snap.sources[module].2.clone(), 0);
        let mut actions = Vec::new();
        let mut all_safe: Vec<Json> = Vec::new();
        let mut taken: Vec<Span> = Vec::new();
        let src = snap.sources[module].2.clone();
        let errors_now = snap.found.iter().filter(|f| f.module == module && f.diag.severity() == Severity::Error).count();
        let mut trials = 0;
        for f in snap.found.iter().filter(|f| f.module == module) {
            let span = f.diag.root().span;
            let touches = span.start <= hi && lo <= span.end;
            for help in f.diag.helps() {
                let Some(edits) = help.located_edits() else { continue };
                let text_edits: Vec<Json> = edits
                    .iter()
                    .map(|e| Json::obj([("range", range(&doc, e.span, enc)), ("newText", Json::str(e.text.clone()))]))
                    .collect();
                let safe = help.applicability == Applicability::MachineApplicable;
                if safe && !edits.iter().any(|e| taken.iter().any(|t| overlaps(*t, e.span))) {
                    taken.extend(edits.iter().map(|e| e.span));
                    all_safe.extend(text_edits.iter().cloned());
                }
                if touches && wants("quickfix") {
                    // A fix the compiler is unsure of is tried first (Seminal's
                    // idea): one that leaves as many errors is offered plainly,
                    // one that removes errors is preferred, one that adds is dropped.
                    let mut title = help.msg.clone();
                    let mut preferred = safe;
                    if !safe && trials < MAX_FIX_TRIALS {
                        trials += 1;
                        let after = analysis::trial(snap, &utilities::error::apply_edits(&src, edits)).len();
                        if after > errors_now {
                            continue;
                        }
                        if after < errors_now {
                            preferred = true;
                            title = format!("{title} (checked: {} error(s) fewer)", errors_now - after);
                        }
                    }
                    let mut changes = Json::Obj(Vec::new());
                    changes.set(&uri, Json::Arr(text_edits));
                    actions.push(Json::obj([
                        ("title", Json::str(title)),
                        ("kind", Json::str("quickfix")),
                        ("isPreferred", Json::Bool(preferred)),
                        ("diagnostics", Json::Arr(vec![Json::obj([
                            ("range", range(&doc, span, enc)),
                            ("message", Json::str(message(&f.diag))),
                            ("code", Json::str(f.diag.root().code.name())),
                        ])])),
                        ("edit", Json::obj([("changes", changes)])),
                    ]));
                }
            }
        }
        if !all_safe.is_empty() && wants("source.fixAll") {
            let mut changes = Json::Obj(Vec::new());
            changes.set(&uri, Json::Arr(all_safe));
            actions.push(Json::obj([
                ("title", Json::str("Apply every safe fix in this file")),
                ("kind", Json::str("source.fixAll.thrax")),
                ("edit", Json::obj([("changes", changes)])),
            ]));
        }
        Json::Arr(actions)
    }

    fn hover(&mut self, params: &Json) -> Json {
        let Some((uri, offset)) = self.locate(params) else { return Json::Null };
        let enc = self.enc;
        let path = self.open[&uri].path.clone();
        let Some(snap) = self.current(&uri) else { return Json::Null };
        let Some(module) = snap.module_of(&path) else { return Json::Null };
        let src = &snap.sources[module].2;
        if let Some((name, span)) = snap.name_at(module, offset) {
            let checker = snap.checker(module);
            let Some(ty) = checker.own_values().iter().find(|(n, _)| *n == name).map(|(n, t)| checker.show_decl(n, *t))
            else {
                return Json::Null;
            };
            let doc = Document::new(src.clone(), 0);
            return Json::obj([
                ("contents", Json::obj([("kind", Json::str("markdown")), ("value", Json::str(format!("```thrax\n{name} : {ty}\n```")))])),
                ("range", range(&doc, span, enc)),
            ]);
        }
        let Some((e, span)) = snap.expr_at(module, offset) else { return Json::Null };
        let checker = snap.checker(module);
        let Some(ty) = checker.expr_type(e) else { return Json::Null };
        let shown = match snap.ast().expr(e) {
            frontend::Expr::Var { .. } => format!("{} : {ty}", span.slice(src)),
            _ => ty,
        };
        let doc = Document::new(src.clone(), 0);
        Json::obj([
            ("contents", Json::obj([("kind", Json::str("markdown")), ("value", Json::str(format!("```thrax\n{shown}\n```")))])),
            ("range", range(&doc, span, enc)),
        ])
    }

    fn definition(&mut self, params: &Json) -> Json {
        let Some((uri, offset)) = self.locate(params) else { return Json::Null };
        let enc = self.enc;
        let path = self.open[&uri].path.clone();
        let buffers: Vec<(PathBuf, String)> = self.open.iter().map(|(u, o)| (o.path.clone(), u.clone())).collect();
        let Some(snap) = self.current(&uri) else { return Json::Null };
        let Some(module) = snap.module_of(&path) else { return Json::Null };
        let Some((e, _)) = snap.expr_at(module, offset) else { return Json::Null };
        let ast = snap.ast();
        let frontend::Expr::Var { module: qual, name } = ast.expr(e) else { return Json::Null };
        let name = ast.text(*name);
        let checker = snap.checker(module);
        let owner = match qual {
            Some(q) => Some(checker.qualifier_module(ast.text(*q))),
            None => checker.call_modules().get(&e).map(|m| m.to_string()),
        };
        let candidates: Vec<usize> = match owner {
            Some(m) => snap.sources.iter().position(|(n, _, _)| *n == m).into_iter().collect(),
            None => std::iter::once(module).chain(0..snap.sources.len()).collect(),
        };
        for m in candidates {
            if let Some(span) = snap.definition(m, name) {
                let (_, file, src) = &snap.sources[m];
                let doc = Document::new(src.clone(), 0);
                // An open buffer is a target even with no file behind it (a new
                // file, or an editor with no filesystem); an embedded module is not.
                if let Some((_, open_uri)) = buffers.iter().find(|(p, _)| p == Path::new(file)) {
                    return Json::obj([("uri", Json::str(open_uri.clone())), ("range", range(&doc, span, enc))]);
                }
                if !Path::new(file).is_file() {
                    continue;
                }
                let file = Path::new(file).canonicalize().unwrap_or_else(|_| PathBuf::from(file));
                return Json::obj([("uri", Json::str(path_to_uri(&file))), ("range", range(&doc, span, enc))]);
            }
        }
        Json::Null
    }

    /// Names to complete at the cursor: the hole's fits on a `???`, a module's
    /// members after `M.`, otherwise every visible value and the keywords, each
    /// with its type.
    fn completion(&mut self, params: &Json) -> Json {
        let Some((uri, offset)) = self.locate(params) else { return Json::Null };
        let text = self.open[&uri].doc.text.clone();
        let path = self.open[&uri].path.clone();
        let before = &text[..offset.min(text.len())];
        let word_start = before
            .char_indices()
            .rev()
            .find(|(_, c)| !(c.is_ascii_alphanumeric() || *c == '_' || *c == '?'))
            .map_or(0, |(i, c)| i + c.len_utf8());
        let prefix = &before[word_start..];
        let qualifier = before[..word_start].strip_suffix('.').map(|q| {
            let start = q.rfind(|c: char| !(c.is_ascii_alphanumeric() || c == '_')).map_or(0, |i| i + 1);
            q[start..].to_string()
        });
        let Some(snap) = self.current(&uri) else { return Json::Null };
        let Some(module) = snap.module_of(&path) else { return Json::Null };
        let checker = snap.checker(module);
        let item = |label: &str, detail: &str, kind: i64| {
            Json::obj([("label", Json::str(label)), ("kind", Json::int(kind)), ("detail", Json::str(detail))])
        };
        let mut items = Vec::new();
        if prefix == frontend::lexer::HOLE {
            for f in snap.found.iter().filter(|f| f.diag.root().code == Code::TypedHole) {
                if f.diag.root().span.start <= offset && offset <= f.diag.root().span.end {
                    for h in f.diag.helps() {
                        if let Some(name) = h.msg.strip_prefix("fill the hole with `").and_then(|m| m.strip_suffix('`')) {
                            items.push(item(name, &f.diag.root().msg, 6));
                        }
                    }
                }
            }
        } else if let Some(q) = qualifier.filter(|q| q.starts_with(|c: char| c.is_ascii_uppercase())) {
            for (name, ty) in checker.qualified_values(&q) {
                if name.starts_with(prefix) {
                    items.push(item(&name, &ty, 3));
                }
            }
        } else {
            for (name, ty) in checker.visible_values() {
                if name.starts_with(prefix) {
                    items.push(item(name, &ty, if ty.contains("->") { 3 } else { 6 }));
                }
            }
            for kw in ["let", "in", "if", "else", "is", "with", "do", "ctl", "defer"] {
                if kw.starts_with(prefix) {
                    items.push(item(kw, "keyword", 14));
                }
            }
        }
        Json::obj([("isIncomplete", Json::Bool(false)), ("items", Json::Arr(items))])
    }

    /// Highlighting straight from the lexer, so an editor needs no grammar of
    /// its own: the classes are [`Class`].
    fn semantic_tokens(&mut self, params: &Json) -> Json {
        let Some(uri) = params.get("textDocument").get("uri").as_str() else { return Json::Null };
        let Some(open) = self.open.get(uri) else { return Json::Null };
        semantic_token_data(&open.doc, self.enc)
    }

    fn document_symbols(&mut self, params: &Json) -> Json {
        let Some(uri) = params.get("textDocument").get("uri").as_str().map(str::to_string) else {
            return Json::Arr(Vec::new());
        };
        let enc = self.enc;
        let Some(path) = self.open.get(&uri).map(|o| o.path.clone()) else { return Json::Arr(Vec::new()) };
        let Some(snap) = self.current(&uri) else { return Json::Arr(Vec::new()) };
        let Some(module) = snap.module_of(&path) else { return Json::Arr(Vec::new()) };
        let ast = snap.ast();
        let p = &snap.programs[module];
        let doc = Document::new(snap.sources[module].2.clone(), 0);
        let mut out = Vec::new();
        for (item, span) in ast.slice(p.items).iter().zip(ast.slice(p.spans)) {
            let (name, kind) = match item {
                frontend::Item::Def { name, .. } => (*name, 12),
                frontend::Item::Struct { name, .. } => (*name, 23),
                frontend::Item::Union { name, .. } => (*name, 10),
                frontend::Item::Alias { name, .. } => (*name, 26),
                frontend::Item::Effect { name, .. } => (*name, 11),
                _ => continue,
            };
            let detail = match item {
                frontend::Item::Def { .. } => snap
                    .checker(module)
                    .own_values()
                    .iter()
                    .find(|(n, _)| *n == ast.text(name))
                    .map(|(n, t)| snap.checker(module).show_decl(n, *t)),
                _ => None,
            };
            let mut sym = Json::obj([
                ("name", Json::str(ast.text(name))),
                ("kind", Json::int(kind)),
                ("range", range(&doc, span.full, enc)),
                ("selectionRange", range(&doc, span.name.unwrap_or(span.full), enc)),
            ]);
            if let Some(d) = detail {
                sym.set("detail", Json::str(d));
            }
            out.push(sym);
        }
        Json::Arr(out)
    }

    fn formatting(&mut self, params: &Json, ranged: bool) -> Json {
        let Some(uri) = params.get("textDocument").get("uri").as_str().map(str::to_string) else { return Json::Null };
        let enc = self.enc;
        let Some(open) = self.open.get(&uri) else { return Json::Null };
        let config = self.config(&open.path);
        let doc = &open.doc;
        let window = ranged.then(|| {
            let r = params.get("range");
            (doc.offset(position(r.get("start")), enc), doc.offset(position(r.get("end")), enc))
        });
        let style = frontend::format::Style {
            width: config.int("format", "width") as usize,
            indent: config.int("format", "indent") as usize,
        };
        match frontend::format::format_edits(&doc.text, &style, window) {
            Ok(edits) => Json::Arr(
                edits
                    .iter()
                    .map(|e| Json::obj([("range", range(doc, e.span, enc)), ("newText", Json::str(e.text.clone()))]))
                    .collect(),
            ),
            Err(_) => Json::Null,
        }
    }

    /// The document and byte offset a position request is about.
    fn locate(&self, params: &Json) -> Option<(String, usize)> {
        let uri = params.get("textDocument").get("uri").as_str()?.to_string();
        let open = self.open.get(&uri)?;
        let offset = open.doc.offset(position(params.get("position")), self.enc);
        Some((uri, offset))
    }
}

/// `textDocument/semanticTokens/full` for `text` with no document open: what an
/// embedder highlights a buffer with before (or without) opening it. UTF-16
/// positions, the protocol's default.
pub fn highlight(text: &str) -> Json {
    semantic_token_data(&Document::new(text.to_string(), 0), Encoding::Utf16)
}

fn semantic_token_data(doc: &Document, enc: Encoding) -> Json {
    let Ok(tokens) = frontend::Lexer::tokenize_with_comments(&doc.text) else { return Json::Null };
    let mut data = Vec::new();
    let (mut line, mut col) = (0usize, 0usize);
    for t in tokens {
        let text = &doc.text[t.span.start..t.span.end];
        let Some(class) = highlight::class(t.kind, text) else { continue };
        let class = class as usize;
        // A token is reported line by line, so a multi-line one (a block
        // comment) is split at its newlines.
        let mut start = t.span.start;
        for piece in text.split_inclusive('\n') {
            let body = piece.trim_end_matches('\n');
            let at = doc.position(start, enc);
            let end = doc.position(start + body.len(), enc);
            if end.character > at.character {
                let dl = at.line - line;
                let dc = if dl == 0 { at.character - col } else { at.character };
                data.extend([dl, dc, end.character - at.character, class, 0].map(|n| Json::int(n as i64)));
                line = at.line;
                col = at.character;
            }
            start += piece.len();
        }
    }
    Json::obj([("data", Json::Arr(data))])
}

fn position(p: &Json) -> Position {
    Position {
        line: p.get("line").as_i64().unwrap_or(0).max(0) as usize,
        character: p.get("character").as_i64().unwrap_or(0).max(0) as usize,
    }
}

fn range(doc: &Document, span: Span, enc: Encoding) -> Json {
    let a = doc.position(span.start, enc);
    let b = doc.position(span.end, enc);
    let pos = |p: Position| Json::obj([("line", Json::int(p.line as i64)), ("character", Json::int(p.character as i64))]);
    Json::obj([("start", pos(a)), ("end", pos(b))])
}

fn related_info(uri: &str, doc: &Document, span: Span, msg: &str, enc: Encoding) -> Json {
    Json::obj([
        ("location", Json::obj([("uri", Json::str(uri)), ("range", range(doc, span, enc))])),
        ("message", Json::str(msg)),
    ])
}

/// The diagnostic's text for an editor: the message, then its notes and the
/// prose of its helps (a help with edits is also offered as a code action).
fn message(d: &Diagnostic) -> String {
    let mut out = d.root().msg.clone();
    for n in d.notes() {
        out.push_str("\nnote: ");
        out.push_str(n);
    }
    for h in d.helps() {
        out.push_str("\nhelp: ");
        out.push_str(&h.msg);
    }
    out
}

/// A lexical or syntax error: its span is a point in broken text, where a hole
/// cannot go.
fn is_syntax(code: Code) -> bool {
    matches!(
        code,
        Code::UnexpectedToken
            | Code::UnclosedQuote
            | Code::InvalidEscape
            | Code::NumberParsingFailure
            | Code::UnknownSymbol
            | Code::AsciiControlChar
            | Code::NonAsciiChar
    )
}

fn overlaps(a: Span, b: Span) -> bool {
    a.start < b.end && b.start < a.end || a == b
}

/// The source range of the item holding `span`, or `span` itself.
fn item_extent(snap: &Snapshot, module: usize, span: Span) -> Span {
    let ast = snap.ast();
    ast.slice(snap.programs[module].spans)
        .iter()
        .find(|s| s.full.start <= span.start && span.start <= s.full.end)
        .map_or(span, |s| s.full)
}

/// The name of the definition whose item holds `span`.
fn enclosing_def(snap: &Snapshot, module: usize, span: Span) -> Option<String> {
    let ast = snap.ast();
    let p = &snap.programs[module];
    ast.slice(p.items).iter().zip(ast.slice(p.spans)).find_map(|(item, s)| match item {
        frontend::Item::Def { name, .. } if s.full.start <= span.start && span.start <= s.full.end => {
            Some(ast.text(*name).to_string())
        }
        _ => None,
    })
}

/// Whether `name` occurs as a whole word in `src[within]`.
fn mentions(src: &str, within: Span, name: &str) -> bool {
    let text = &src[within.start.min(src.len())..within.end.min(src.len())];
    let word = |c: char| c.is_ascii_alphanumeric() || c == '_';
    text.match_indices(name).any(|(i, _)| {
        let before = text[..i].chars().next_back();
        let after = text[i + name.len()..].chars().next();
        !before.is_some_and(word) && !after.is_some_and(word)
    })
}

/// The stretch of source whose errors are held back: the item around the last
/// edit, while typing has not paused for `lsp.grace_ms`.
fn held_region(snap: &Snapshot, module: usize, open: &Open, config: &Config) -> Option<(usize, usize)> {
    if !config.bool("lsp", "grace") {
        return None;
    }
    let (at, when) = open.last_edit?;
    if when.elapsed() >= Duration::from_millis(config.int("lsp", "grace_ms") as u64) {
        return None;
    }
    let ast = snap.ast();
    let spans = ast.slice(snap.programs[module].spans);
    if let Some(s) = spans.iter().find(|s| s.full.start <= at && at <= s.full.end) {
        return Some((s.full.start, s.full.end));
    }
    // The edited item did not parse, so it has no span: hold back the gap
    // between its neighbours.
    let lo = spans.iter().filter(|s| s.full.end <= at).map(|s| s.full.end).max().unwrap_or(0);
    let hi = spans.iter().filter(|s| s.full.start > at).map(|s| s.full.start).min().unwrap_or(usize::MAX);
    Some((lo, hi))
}
