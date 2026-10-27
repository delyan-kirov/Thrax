//! `thrax lsp` end to end: spawn the server, speak JSON-RPC to it over stdio,
//! and check what an editor would see.

use std::io::{BufRead, BufReader, Write};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};

use utilities::Json;

struct Client {
    child: Child,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
    next_id: i64,
}

impl Client {
    fn start() -> Client {
        let mut child = Command::new(env!("CARGO_BIN_EXE_thrax"))
            .arg("lsp")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .expect("spawn thrax lsp");
        let stdin = child.stdin.take().expect("stdin");
        let stdout = BufReader::new(child.stdout.take().expect("stdout"));
        Client { child, stdin, stdout, next_id: 1 }
    }

    fn send(&mut self, msg: Json) {
        let body = msg.to_string();
        write!(self.stdin, "Content-Length: {}\r\n\r\n{body}", body.len()).expect("write");
        self.stdin.flush().expect("flush");
    }

    fn read(&mut self) -> Json {
        let mut len = 0usize;
        loop {
            let mut line = String::new();
            assert!(self.stdout.read_line(&mut line).expect("read") > 0, "server closed its output");
            let line = line.trim_end();
            if line.is_empty() {
                break;
            }
            if let Some(v) = line.strip_prefix("Content-Length:") {
                len = v.trim().parse().expect("length");
            }
        }
        let mut body = vec![0; len];
        std::io::Read::read_exact(&mut self.stdout, &mut body).expect("body");
        Json::parse(std::str::from_utf8(&body).expect("utf-8")).expect("json")
    }

    fn request(&mut self, method: &str, params: Json) -> Json {
        let id = self.next_id;
        self.next_id += 1;
        self.send(Json::obj([
            ("jsonrpc", Json::str("2.0")),
            ("id", Json::int(id)),
            ("method", Json::str(method)),
            ("params", params),
        ]));
        loop {
            let msg = self.read();
            if msg.get("id").as_i64() == Some(id) {
                return msg.get("result").clone();
            }
        }
    }

    fn notify(&mut self, method: &str, params: Json) {
        self.send(Json::obj([("jsonrpc", Json::str("2.0")), ("method", Json::str(method)), ("params", params)]));
    }

    /// The next diagnostics published for `uri`.
    fn diagnostics(&mut self, uri: &str) -> Vec<Json> {
        loop {
            let msg = self.read();
            if msg.get("method").as_str() == Some("textDocument/publishDiagnostics")
                && msg.get("params").get("uri").as_str() == Some(uri)
            {
                return msg.get("params").get("diagnostics").as_arr().to_vec();
            }
        }
    }
}

fn pos(line: i64, character: i64) -> Json {
    Json::obj([("line", Json::int(line)), ("character", Json::int(character))])
}

fn doc(uri: &str) -> Json {
    Json::obj([("uri", Json::str(uri))])
}

#[test]
fn diagnostics_fixes_hover_definition_and_formatting() {
    let dir = std::env::temp_dir().join(format!("thrax-lsp-test-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("DEMO.thx");
    let src = "$ @mod DEMO\n\n$ double : @Int -> @Int = \\x = x * 2\n\n$ h = \\x = (x + 1\n\n$ k : @Int = dobule 3\n\n$ y : @Float64 = 2.\n";
    std::fs::write(&path, src).unwrap();
    let path = path.canonicalize().unwrap();
    let uri = thrax::lsp::path_to_uri(&path);

    let mut c = Client::start();
    let init = c.request(
        "initialize",
        Json::obj([
            ("capabilities", Json::obj([("general", Json::obj([("positionEncodings", Json::Arr(vec![Json::str("utf-8")]))]))])),
            ("initializationOptions", Json::obj([("lsp", Json::obj([("grace", Json::Bool(false))]))])),
        ]),
    );
    assert_eq!(init.get("capabilities").get("positionEncoding").as_str(), Some("utf-8"));
    c.notify("initialized", Json::obj([]));
    c.notify(
        "textDocument/didOpen",
        Json::obj([(
            "textDocument",
            Json::obj([
                ("uri", Json::str(uri.clone())),
                ("languageId", Json::str("thrax")),
                ("version", Json::int(1)),
                ("text", Json::str(src)),
            ]),
        )]),
    );

    // Every error is reported, the first one as the focus.
    let diags = c.diagnostics(&uri);
    let codes: Vec<&str> = diags.iter().filter_map(|d| d.get("code").as_str()).collect();
    assert!(codes.contains(&"UNEXPECTED_TOKEN"), "{codes:?}");
    assert!(codes.contains(&"TYPE_UNBOUND"), "{codes:?}");
    assert!(codes.contains(&"NUMBER_PARSING_FAILURE"), "{codes:?}");
    let focus: Vec<&Json> = diags.iter().filter(|d| d.get("data").get("role").as_str() == Some("focus")).collect();
    assert_eq!(focus.len(), 1, "{diags:?}");
    assert_eq!(focus[0].get("severity").as_i64(), Some(1));

    // The typo comes with a quick fix.
    let typo = diags.iter().find(|d| d.get("code").as_str() == Some("TYPE_UNBOUND")).unwrap();
    let actions = c.request(
        "textDocument/codeAction",
        Json::obj([
            ("textDocument", doc(&uri)),
            ("range", typo.get("range").clone()),
            ("context", Json::obj([("diagnostics", Json::Arr(vec![]))])),
        ]),
    );
    let typo_fix = actions
        .as_arr()
        .iter()
        .find(|a| a.get("title").as_str().is_some_and(|t| t.starts_with("did you mean `double`?")))
        .unwrap_or_else(|| panic!("{actions}"));
    // An uncertain fix is tried before it is offered; this one removes the error.
    assert!(typo_fix.get("title").as_str().unwrap().contains("checked: 1 error(s) fewer"), "{typo_fix}");
    assert_eq!(typo_fix.get("isPreferred").as_bool(), Some(true));
    let all = c.request(
        "textDocument/codeAction",
        Json::obj([
            ("textDocument", doc(&uri)),
            ("range", Json::obj([("start", pos(0, 0)), ("end", pos(0, 0))])),
            ("context", Json::obj([("only", Json::Arr(vec![Json::str("source.fixAll")])), ("diagnostics", Json::Arr(vec![]))])),
        ]),
    );
    let fix_all = &all.as_arr()[0];
    let edits = fix_all.get("edit").get("changes").get(&uri).as_arr();
    assert!(edits.iter().any(|e| e.get("newText").as_str() == Some("2.0")), "{fix_all}");

    // Hover shows a type; definition jumps to `double`.
    let hover = c.request("textDocument/hover", Json::obj([("textDocument", doc(&uri)), ("position", pos(2, 31))]));
    let shown = hover.get("contents").get("value").as_str().unwrap_or("");
    assert!(shown.contains("@Int"), "{hover}");
    let fixed = src.replace("dobule", "double");
    c.notify(
        "textDocument/didChange",
        Json::obj([
            ("textDocument", Json::obj([("uri", Json::str(uri.clone())), ("version", Json::int(2))])),
            ("contentChanges", Json::Arr(vec![Json::obj([("text", Json::str(fixed.clone()))])])),
        ]),
    );
    let def = c.request("textDocument/definition", Json::obj([("textDocument", doc(&uri)), ("position", pos(6, 14))]));
    assert_eq!(def.get("range").get("start").get("line").as_i64(), Some(2), "{def}");

    // Formatting a clean file.
    let clean = "$ @mod DEMO\n$ x=1+2\n";
    c.notify(
        "textDocument/didChange",
        Json::obj([
            ("textDocument", Json::obj([("uri", Json::str(uri.clone())), ("version", Json::int(3))])),
            ("contentChanges", Json::Arr(vec![Json::obj([("text", Json::str(clean))])])),
        ]),
    );
    let edits = c.request("textDocument/formatting", Json::obj([("textDocument", doc(&uri)), ("options", Json::obj([]))]));
    assert_eq!(edits.as_arr().len(), 1, "{edits}");
    assert_eq!(edits.as_arr()[0].get("newText").as_str(), Some(" = 1 + "), "{edits}");

    // Highlighting comes from the lexer: `$`, `x`, `=`, `1`, `+`, `2` after the header.
    let tokens = c.request("textDocument/semanticTokens/full", Json::obj([("textDocument", doc(&uri))]));
    assert_eq!(tokens.get("data").as_arr().len() % 5, 0, "{tokens}");
    assert!(tokens.get("data").as_arr().len() / 5 >= 9, "{tokens}");

    let symbols = c.request("textDocument/documentSymbol", Json::obj([("textDocument", doc(&uri))]));
    assert_eq!(symbols.as_arr()[0].get("name").as_str(), Some("x"), "{symbols}");

    // Completion offers visible names with their types.
    let typing = "$ @mod DEMO\n$ double : @Int -> @Int = \\x = x * 2\n$ k : @Int = dou\n";
    c.notify(
        "textDocument/didChange",
        Json::obj([
            ("textDocument", Json::obj([("uri", Json::str(uri.clone())), ("version", Json::int(4))])),
            ("contentChanges", Json::Arr(vec![Json::obj([("text", Json::str(typing))])])),
        ]),
    );
    let done = c.request("textDocument/completion", Json::obj([("textDocument", doc(&uri)), ("position", pos(2, 16))]));
    let labels: Vec<&str> = done.get("items").as_arr().iter().filter_map(|i| i.get("label").as_str()).collect();
    assert!(labels.contains(&"double"), "{labels:?}");

    // Saving runs the compile-time checks, whose failures show up too.
    let checked = "$ @mod DEMO\n$ two : @Int = 2\n$ @e <| assert (two == 3) \"two is not three\"\n";
    std::fs::write(&path, checked).unwrap();
    c.notify(
        "textDocument/didChange",
        Json::obj([
            ("textDocument", Json::obj([("uri", Json::str(uri.clone())), ("version", Json::int(5))])),
            ("contentChanges", Json::Arr(vec![Json::obj([("text", Json::str(checked))])])),
        ]),
    );
    c.notify("textDocument/didSave", Json::obj([("textDocument", doc(&uri))]));
    let saved = loop {
        let ds = c.diagnostics(&uri);
        if ds.iter().any(|d| d.get("code").as_str() == Some("META_ABORT")) {
            break ds;
        }
    };
    let abort = saved.iter().find(|d| d.get("code").as_str() == Some("META_ABORT")).unwrap();
    assert_eq!(abort.get("range").get("start").get("line").as_i64(), Some(2), "{abort}");

    c.request("shutdown", Json::Null);
    c.notify("exit", Json::Null);
    let status = c.child.wait().unwrap();
    assert!(status.success());
    let _ = std::fs::remove_dir_all(&dir);
}

// A module that exists only as an unsaved buffer beside the document resolves
// where its file would be, and a definition in it is a jump target: an editor
// with a new file, or one with no filesystem at all (the browser playground).
#[test]
fn unsaved_sibling_buffer_is_a_module() {
    let dir = std::env::temp_dir().join(format!("thrax-lsp-sibling-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let dir = dir.canonicalize().unwrap();
    let main_src = "$ @mod MAIN\n$ with GREET\n$ n : @Int = GREET.hello\n";
    std::fs::write(dir.join("MAIN.thx"), main_src).unwrap();
    let main_uri = thrax::lsp::path_to_uri(&dir.join("MAIN.thx"));
    let greet_uri = thrax::lsp::path_to_uri(&dir.join("GREET.thx"));
    assert!(!dir.join("GREET.thx").exists());

    let mut c = Client::start();
    c.request(
        "initialize",
        Json::obj([("initializationOptions", Json::obj([("lsp", Json::obj([("grace", Json::Bool(false))]))]))]),
    );
    c.notify("initialized", Json::obj([]));
    for (uri, text) in [(&greet_uri, "$ @mod GREET\n$ hello : @Int = 1\n"), (&main_uri, main_src)] {
        c.notify(
            "textDocument/didOpen",
            Json::obj([(
                "textDocument",
                Json::obj([
                    ("uri", Json::str(uri.as_str())),
                    ("languageId", Json::str("thrax")),
                    ("version", Json::int(1)),
                    ("text", Json::str(text)),
                ]),
            )]),
        );
    }
    let diags = c.diagnostics(&main_uri);
    assert!(diags.is_empty(), "{diags:?}");

    let def = c.request("textDocument/definition", Json::obj([("textDocument", doc(&main_uri)), ("position", pos(2, 20))]));
    assert_eq!(def.get("uri").as_str(), Some(greet_uri.as_str()), "{def}");
    assert_eq!(def.get("range").get("start").get("line").as_i64(), Some(1), "{def}");

    c.request("shutdown", Json::Null);
    c.notify("exit", Json::Null);
    assert!(c.child.wait().unwrap().success());
    let _ = std::fs::remove_dir_all(&dir);
}

/// The 0-based line of `needle` in CORE.thx.
fn core_line(needle: &str) -> i64 {
    let core = include_str!("../../../library/CORE.thx");
    core.lines().position(|l| l.starts_with(needle)).unwrap_or_else(|| panic!("no `{needle}` in CORE")) as i64
}

#[test]
fn builtins_are_defined_in_core() {
    let dir = std::env::temp_dir().join(format!("thrax-lsp-builtins-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("USE.thx");
    let src = "$ @mod USE\n$ n : @Int = @iadd 1 2\n$ b : @Bool = not @true\n$ v : @Int = @vec_len [1, 2]\n";
    std::fs::write(&path, src).unwrap();
    let uri = thrax::lsp::path_to_uri(&path.canonicalize().unwrap());
    let core_path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../library/CORE.thx").canonicalize().unwrap();
    let core_uri = thrax::lsp::path_to_uri(&core_path);

    let mut c = Client::start();
    c.request(
        "initialize",
        Json::obj([
            ("capabilities", Json::obj([("general", Json::obj([("positionEncodings", Json::Arr(vec![Json::str("utf-8")]))]))])),
            ("initializationOptions", Json::obj([("lsp", Json::obj([("grace", Json::Bool(false))]))])),
        ]),
    );
    c.notify("initialized", Json::obj([]));
    let open = |c: &mut Client, uri: &str, text: &str| {
        c.notify(
            "textDocument/didOpen",
            Json::obj([(
                "textDocument",
                Json::obj([
                    ("uri", Json::str(uri)),
                    ("languageId", Json::str("thrax")),
                    ("version", Json::int(1)),
                    ("text", Json::str(text)),
                ]),
            )]),
        );
    };
    open(&mut c, &uri, src);
    let diags = c.diagnostics(&uri);
    assert!(diags.is_empty(), "{diags:?}");

    for (line, col, decl) in [(1, 15, "$ @iadd "), (2, 16, "$ not "), (3, 15, "$ @vec_len ")] {
        let def = c.request("textDocument/definition", Json::obj([("textDocument", doc(&uri)), ("position", pos(line, col))]));
        assert_eq!(def.get("uri").as_str(), Some(core_uri.as_str()), "{decl}: {def}");
        assert_eq!(def.get("range").get("start").get("line").as_i64(), Some(core_line(decl)), "{decl}: {def}");
    }

    // CORE itself, open in the editor, is clean.
    open(&mut c, &core_uri, include_str!("../../../library/CORE.thx"));
    let diags = c.diagnostics(&core_uri);
    assert!(diags.is_empty(), "{diags:?}");
    c.notify("textDocument/didSave", Json::obj([("textDocument", doc(&core_uri))]));
    let diags = c.diagnostics(&core_uri);
    assert!(diags.is_empty(), "{diags:?}");

    c.request("shutdown", Json::Null);
    c.notify("exit", Json::Null);
    assert!(c.child.wait().unwrap().success());
    let _ = std::fs::remove_dir_all(&dir);
}
