//! The interactive shell (`thrax repl`): a read-eval-print loop that reads
//! ordinary Thrax `$` items, so working in it feels like writing a `.thx` file.
//!
//! Every global starts with `$`, so each prompt opens with a fixed, undeletable
//! `$ ` and then a deletable `_= ` prefill: `$ _= <expr>` evaluates and prints
//! (the discard name `_` is the "show me" idiom), while deleting the prefill and
//! writing `$ name = ...` defines silently and `$ with MOD` imports. Plain Enter
//! is a newline, so an item spans as many lines as it likes; typing `$` then
//! Enter submits it. That terminating `$` is stripped from the source and simply
//! becomes the `$` opening the next prompt. A line beginning with `:` is a
//! meta-command (Enter runs it).
//!
//! The whole session is recompiled against the standard library on each item,
//! so definitions accumulate and errors just print and leave the session intact.
//! No external crates are allowed, so the line editor is hand-rolled on raw-mode
//! `termios` (appends and backspace, no history or cursor movement). When stdin
//! is not a terminal the shell falls back to a plain line reader with the same
//! `$`-terminates-an-item convention.

use std::io::{self, BufRead, Read, Write};
use std::os::raw::c_int;
use std::path::PathBuf;
use std::process::ExitCode;

use crate::driver;

/// The reserved binding an evaluated expression is compiled under.
const IT: &str = "__it";

/// The deletable text seeded into every fresh prompt after the fixed `$ `.
const PREFILL: &str = "_= ";

pub fn cmd_repl() -> ExitCode {
    let root_dir = std::env::current_dir().unwrap_or_default();
    let mut state = Repl::new(root_dir);

    match term::RawMode::enable() {
        Some(_guard) => run_raw(&mut state),
        None => run_cooked(&mut state),
    }
    ExitCode::SUCCESS
}

/// The interactive editor used when stdin is a terminal.
fn run_raw(state: &mut Repl) {
    let stdin = io::stdin();
    let mut input = stdin.lock();
    let mut ed = Editor::new();
    ed.render();
    loop {
        match term::read_key(&mut input) {
            None | Some(Key::CtrlD) => {
                print!("\r\n");
                let _ = io::stdout().flush();
                break;
            }
            Some(Key::CtrlC) => {
                print!("\r\n");
                ed = Editor::new();
                ed.render();
            }
            Some(Key::Backspace) => ed.backspace(),
            Some(Key::Char(c)) => ed.insert(c),
            Some(Key::Enter) => match ed.on_enter() {
                Enter::Newline => ed.newline(),
                Enter::Command(cmd) => {
                    print!("\r\n");
                    let _ = io::stdout().flush();
                    if state.command(&cmd) {
                        break;
                    }
                    ed = Editor::new();
                    ed.render();
                }
                Enter::Submit(body) => {
                    ed.set_buffer(body.clone());
                    ed.render();
                    print!("\r\n");
                    let _ = io::stdout().flush();
                    state.submit(&body);
                    ed = Editor::new();
                    ed.render();
                }
            },
        }
    }
}

/// A line-at-a-time reader for when stdin is a pipe or file: the same session
/// semantics, but editing is the shell's problem, not ours. An item accumulates
/// over lines and submits when a line ends in `$` (or on a lone `:` command).
fn run_cooked(state: &mut Repl) {
    let stdin = io::stdin();
    let mut buf = String::new();
    for line in stdin.lock().lines() {
        let Ok(line) = line else { break };
        let line = line.trim_end_matches(['\n', '\r']);
        if buf.is_empty() {
            if let Some(cmd) = line.trim_start().strip_prefix(':') {
                if state.command(cmd.trim().trim_end_matches('$').trim()) {
                    break;
                }
                continue;
            }
        }
        if !buf.is_empty() {
            buf.push('\n');
        }
        match line.trim_end().strip_suffix('$') {
            Some(last) => {
                buf.push_str(last);
                state.submit(&buf);
                buf.clear();
            }
            None => buf.push_str(line),
        }
    }
    state.submit(&buf);
}

/// A decoded keypress from the raw-mode terminal.
enum Key {
    Char(char),
    Enter,
    Backspace,
    CtrlC,
    CtrlD,
}

/// What pressing Enter means for the item currently being edited.
enum Enter {
    /// Continue the item on a new line.
    Newline,
    /// Submit this source (the trailing `$` terminator already removed).
    Submit(String),
    /// Run this `:command` (the leading `:` already removed).
    Command(String),
}

/// The current item's editable text, excluding the fixed `$ ` prompt. Rendering
/// reprints the whole block in place, so backspacing across a newline Just Works
/// without cursor bookkeeping. Line wrapping is not accounted for.
struct Editor {
    buffer: String,
    /// Physical lines the last render occupied, so the next one can rewind.
    drawn_lines: usize,
}

impl Editor {
    fn new() -> Self {
        Editor {
            buffer: PREFILL.to_string(),
            drawn_lines: 0,
        }
    }

    fn set_buffer(&mut self, buffer: String) {
        self.buffer = buffer;
    }

    fn insert(&mut self, c: char) {
        self.buffer.push(c);
        self.render();
    }

    fn newline(&mut self) {
        self.buffer.push('\n');
        self.render();
    }

    fn backspace(&mut self) {
        if self.buffer.pop().is_some() {
            self.render();
        }
    }

    /// Decide what this Enter does: a line beginning with `:` is a command, a
    /// trailing `$` submits, anything else extends the item.
    fn on_enter(&self) -> Enter {
        let trimmed = self.buffer.trim_end();
        if !self.buffer.contains('\n') {
            if let Some(cmd) = trimmed.trim_start().strip_prefix(':') {
                return Enter::Command(cmd.trim().trim_end_matches('$').trim().to_string());
            }
        }
        match trimmed.strip_suffix('$') {
            Some(body) => Enter::Submit(body.to_string()),
            None => Enter::Newline,
        }
    }

    /// Reprint the block (`$ ` + buffer) in place.
    fn render(&mut self) {
        let mut out = String::new();
        if self.drawn_lines > 0 {
            if self.drawn_lines > 1 {
                out.push_str(&format!("\x1b[{}A", self.drawn_lines - 1));
            }
            out.push_str("\r\x1b[J");
        }
        out.push_str("$ ");
        out.push_str(&self.buffer);
        print!("{out}");
        let _ = io::stdout().flush();
        self.drawn_lines = 1 + self.buffer.matches('\n').count();
    }
}

struct Repl {
    root_dir: PathBuf,
    /// Full `$ with MOD` lines, in entry order, deduplicated.
    imports: Vec<String>,
    /// Accumulated definitions as `(binding name, full source item)`. Redefining
    /// a name replaces its entry, so the module never has a duplicate `$ name`.
    defs: Vec<(String, String)>,
}

impl Repl {
    fn new(root_dir: PathBuf) -> Self {
        Repl {
            root_dir,
            imports: Vec::new(),
            defs: Vec::new(),
        }
    }

    /// The `@mod REPL` source for the current session, with `extra` appended.
    fn source(&self, extra: &str) -> String {
        let mut s = String::from("@mod REPL\n");
        for imp in &self.imports {
            s.push_str(imp);
            s.push('\n');
        }
        for (_, def) in &self.defs {
            s.push_str(def);
            s.push('\n');
        }
        s.push_str(extra);
        s
    }

    /// Process one completed item (a `$`-prefixed source fragment, minus its
    /// leading `$`). Empty items are ignored.
    fn submit(&mut self, body: &str) {
        let body = body.trim();
        if body.is_empty() {
            return;
        }
        let item = format!("$ {body}");
        if body.split_whitespace().next() == Some("with") {
            self.import(&item);
            return;
        }
        match def_name(body) {
            Some(name) if name == "_" => self.evaluate(body),
            Some(name) if looks_like_name(&name) => self.define(name, &item),
            _ => eprintln!(
                "thrax: `{body}` is not a definition; to evaluate an expression write \
                 `$ _ = {body}`"
            ),
        }
    }

    /// Add a `$ with MOD` import, keeping it only if the session still compiles.
    fn import(&mut self, item: &str) {
        if self.imports.iter().any(|i| i == item) {
            return;
        }
        self.imports.push(item.to_string());
        if let Err(e) = self.recompile() {
            self.imports.pop();
            print!("{e}");
        }
    }

    /// Add or replace a named definition, rolling back if it does not compile.
    fn define(&mut self, name: String, item: &str) {
        let previous = self.defs.iter().position(|(n, _)| *n == name);
        let saved = previous.map(|i| self.defs[i].clone());
        match previous {
            Some(i) => self.defs[i] = (name.clone(), item.to_string()),
            None => self.defs.push((name.clone(), item.to_string())),
        }
        if let Err(e) = self.recompile() {
            match (previous, saved) {
                (Some(i), Some(old)) => self.defs[i] = old,
                _ => self.defs.retain(|(n, _)| *n != name),
            }
            print!("{e}");
        }
    }

    /// Compile and force `$ _ = <body>` (renamed to the reserved `__it`), then
    /// print its value or the diagnostic.
    fn evaluate(&self, body: &str) {
        // `body` starts with the discard name `_`; swap it for the eval binding.
        let it_item = format!("$ {IT}{}", &body[1..]);
        let src = self.source(&format!("{it_item}\n"));
        let session = match driver::compile_session(&src, &self.root_dir) {
            Ok(s) => s,
            Err(e) => {
                print!("{e}");
                return;
            }
        };
        let ir = frontend::ir::lower_modules(&session.lowered);
        match interpreter::machine::eval(&ir, IT) {
            Ok(shown) => println!("{shown}"),
            Err(diag) => print!("{}", diag.render("", "<repl>")),
        }
    }

    /// Type-check the session as it stands; `Ok(())` means it builds.
    fn recompile(&self) -> Result<(), String> {
        driver::compile_session(&self.source(""), &self.root_dir).map(|_| ())
    }

    /// Handle a `:command`. Returns `true` to quit.
    fn command(&mut self, cmd: &str) -> bool {
        let (name, rest) = match cmd.split_once(char::is_whitespace) {
            Some((n, r)) => (n, r.trim()),
            None => (cmd.trim(), ""),
        };
        match name {
            "q" | "quit" => return true,
            "help" | "?" => print!("{HELP}"),
            "reset" => {
                self.imports.clear();
                self.defs.clear();
            }
            "list" | "l" => {
                for imp in &self.imports {
                    println!("{imp}");
                }
                for (_, def) in &self.defs {
                    println!("{def}");
                }
            }
            "t" | "type" => {
                if rest.is_empty() {
                    eprintln!("thrax: `:type` needs an expression, e.g. `:type 1 + 2`");
                } else {
                    self.show_type(rest);
                }
            }
            other => eprintln!("thrax: unknown command `:{other}` (`:help` for the list)"),
        }
        false
    }

    /// Print `expr :: <type>` without evaluating it.
    fn show_type(&self, expr: &str) {
        let src = self.source(&format!("$ {IT} = {expr}\n"));
        match driver::compile_session(&src, &self.root_dir) {
            Ok(session) => match session.decls.iter().find(|(n, _)| n == IT) {
                Some((_, ty)) => println!("{expr} :: {ty}"),
                None => eprintln!("thrax: could not determine the type of `{expr}`"),
            },
            Err(e) => print!("{e}"),
        }
    }
}

/// Whether `name` could be a Thrax binding name: a value/type name starts with a
/// letter, an operator with `(`. A digit/quote/symbol start means the user typed
/// a bare expression, not a definition.
fn looks_like_name(name: &str) -> bool {
    name.starts_with('(') || name.chars().next().is_some_and(char::is_alphabetic)
}

/// The binding name a definition introduces: the first token after `$`, or a
/// parenthesized operator like `(+)`. `None` if neither is present.
fn def_name(after: &str) -> Option<String> {
    let after = after.trim_start();
    if let Some(rest) = after.strip_prefix('(') {
        let inner = rest.split(')').next()?;
        return Some(format!("({inner})"));
    }
    let name: String = after
        .chars()
        .take_while(|c| !c.is_whitespace() && *c != ':' && *c != '=')
        .collect();
    (!name.is_empty()).then_some(name)
}

const HELP: &str = "\
The shell reads Thrax `$` items, like a `.thx` file. Each prompt opens with a
fixed `$ ` and a deletable `_= ` prefill. Plain Enter starts a new line, so items
may span lines; type `$` then Enter to submit the item you are editing.

  $ _= <expr>       evaluate an expression and print its value
  $ name = <expr>   add (or redefine) a binding (silent, like a file)
  $ with MOD        import a standard-library module (e.g. VEC, STR, MATH)
  :type <expr>      show an expression's type without evaluating   (:t)
  :list             show the current imports and definitions       (:l)
  :reset            clear all imports and definitions
  :help             show this help                                 (:?)
  :quit             leave the shell (or Ctrl-D)                     (:q)
";

/// Raw-mode terminal support, built directly on libc `termios` (no crates).
mod term {
    use super::{c_int, Key, Read};

    const STDIN: c_int = 0;
    const TCSANOW: c_int = 0;
    // Linux `c_lflag` bits and `c_cc` indices.
    const ISIG: u32 = 0x0000_0001;
    const ICANON: u32 = 0x0000_0002;
    const ECHO: u32 = 0x0000_0008;
    const IEXTEN: u32 = 0x0000_8000;
    const VMIN: usize = 6;
    const VTIME: usize = 5;

    #[repr(C)]
    #[derive(Clone)]
    struct Termios {
        c_iflag: u32,
        c_oflag: u32,
        c_cflag: u32,
        c_lflag: u32,
        c_line: u8,
        c_cc: [u8; 32],
        c_ispeed: u32,
        c_ospeed: u32,
    }

    extern "C" {
        fn tcgetattr(fd: c_int, termios: *mut Termios) -> c_int;
        fn tcsetattr(fd: c_int, actions: c_int, termios: *const Termios) -> c_int;
    }

    /// Puts the terminal into a minimally-raw mode for the editor's lifetime:
    /// echo, canonical line buffering, and signal keys are off, so keys arrive
    /// one at a time and Ctrl-C/D come through as bytes. Output post-processing
    /// (`\n` -> CRLF) is left on, so ordinary `println!` still works. The
    /// original settings are restored on drop. `enable` returns `None` when
    /// stdin is not a terminal.
    pub struct RawMode {
        original: Termios,
    }

    impl RawMode {
        pub fn enable() -> Option<RawMode> {
            let mut original = zeroed();
            if unsafe { tcgetattr(STDIN, &mut original) } != 0 {
                return None;
            }
            let mut raw = original.clone();
            raw.c_lflag &= !(ISIG | ICANON | ECHO | IEXTEN);
            raw.c_cc[VMIN] = 1;
            raw.c_cc[VTIME] = 0;
            if unsafe { tcsetattr(STDIN, TCSANOW, &raw) } != 0 {
                return None;
            }
            Some(RawMode { original })
        }
    }

    impl Drop for RawMode {
        fn drop(&mut self) {
            unsafe { tcsetattr(STDIN, TCSANOW, &self.original) };
        }
    }

    fn zeroed() -> Termios {
        Termios {
            c_iflag: 0,
            c_oflag: 0,
            c_cflag: 0,
            c_lflag: 0,
            c_line: 0,
            c_cc: [0; 32],
            c_ispeed: 0,
            c_ospeed: 0,
        }
    }

    /// Read and decode the next keypress, skipping escape sequences (arrow keys
    /// and the like) and stray control bytes. `None` on end of input.
    pub fn read_key(input: &mut impl Read) -> Option<Key> {
        loop {
            let b = read_byte(input)?;
            match b {
                0x04 => return Some(Key::CtrlD),
                0x03 => return Some(Key::CtrlC),
                b'\r' | b'\n' => return Some(Key::Enter),
                0x7f | 0x08 => return Some(Key::Backspace),
                0x1b => {
                    // A CSI/SS3 escape: swallow through its final byte.
                    if matches!(read_byte(input), Some(b'[') | Some(b'O')) {
                        while let Some(x) = read_byte(input) {
                            if (0x40..=0x7e).contains(&x) {
                                break;
                            }
                        }
                    }
                }
                0x00..=0x1f => {} // other control keys: ignore
                0x20..=0x7e => return Some(Key::Char(b as char)),
                _ => {
                    if let Some(c) = decode_utf8(b, input) {
                        return Some(Key::Char(c));
                    }
                }
            }
        }
    }

    fn read_byte(input: &mut impl Read) -> Option<u8> {
        let mut b = [0u8; 1];
        match input.read(&mut b) {
            Ok(1) => Some(b[0]),
            _ => None,
        }
    }

    /// Assemble a full UTF-8 scalar from a lead byte and its continuation bytes.
    fn decode_utf8(lead: u8, input: &mut impl Read) -> Option<char> {
        let extra = match lead {
            0xc0..=0xdf => 1,
            0xe0..=0xef => 2,
            0xf0..=0xf7 => 3,
            _ => return None,
        };
        let mut bytes = vec![lead];
        for _ in 0..extra {
            bytes.push(read_byte(input)?);
        }
        std::str::from_utf8(&bytes).ok()?.chars().next()
    }
}
