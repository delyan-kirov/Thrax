//! The interactive shell (`thrax repl`): a read-eval-print loop that reads
//! ordinary Thrax `$` items, so working in it feels like writing a `.thx` file.
//!
//! Input is a stream of `$`-delimited items. The first `$` is printed as the
//! prompt (suggesting the shape of input); starting the next item with `$`
//! submits the previous one, so an item may span as many lines as it likes with
//! no continuation character. A blank line or Ctrl-D also submits the pending
//! item. `$ _ = <expr>` evaluates the expression and prints its value (the
//! discard name `_` is the "show me" idiom); a named `$ x = ...` is added
//! silently, like a definition in a file; `$ with MOD` imports a module. Lines
//! beginning with `:` are meta-commands.
//!
//! The whole session is recompiled against the standard library on each item,
//! so definitions accumulate and errors just print and leave the session intact.
//! No external crates are allowed, so there is no readline (history/arrow keys);
//! the terminal's own cooked-mode editing is all that is available.

use std::io::{self, BufRead, Write};
use std::path::PathBuf;
use std::process::ExitCode;

use crate::driver;

/// The reserved binding an evaluated expression is compiled under.
const IT: &str = "__it";

pub fn cmd_repl() -> ExitCode {
    let root_dir = std::env::current_dir().unwrap_or_default();
    let mut state = Repl::new(root_dir);

    println!(
        "thrax repl. Write Thrax items, like a `.thx` file. `$ _ = <expr>` prints a value; \n\
         `$ name = ...` defines; `$ with MOD` imports. Start the next `$` (or a blank line) to \n\
         submit the previous item. `:help` for commands, `:quit` (or Ctrl-D) to exit."
    );

    let mut input = io::stdin().lock();
    let mut buf = String::new();
    loop {
        print!("{}", if buf.is_empty() { "$ " } else { "  " });
        let _ = io::stdout().flush();

        let mut raw = String::new();
        if input.read_line(&mut raw).unwrap_or(0) == 0 {
            println!();
            state.submit(&buf);
            break;
        }
        let line = raw.trim_end_matches(['\n', '\r']);
        let trimmed = line.trim_start();

        // A meta-command: submit the pending item first, then run it.
        if let Some(cmd) = trimmed.strip_prefix(':') {
            state.submit(&buf);
            buf.clear();
            if state.command(cmd) {
                break;
            }
            continue;
        }
        // A new `$` item delimits (and so submits) the previous one.
        if let Some(rest) = trimmed.strip_prefix('$') {
            state.submit(&buf);
            buf = rest.trim_start().to_string();
            continue;
        }
        // A blank line submits the pending item.
        if trimmed.is_empty() {
            state.submit(&buf);
            buf.clear();
            continue;
        }
        // Otherwise this line continues the current item.
        if !buf.is_empty() {
            buf.push('\n');
        }
        buf.push_str(line);
    }
    ExitCode::SUCCESS
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
The shell reads Thrax `$` items, like a `.thx` file. Start the next `$` (or enter
a blank line, or Ctrl-D) to submit the item you are typing; items may span lines.

  $ _ = <expr>      evaluate an expression and print its value
  $ name = <expr>   add (or redefine) a binding (silent, like a file)
  $ with MOD        import a standard-library module (e.g. VEC, STR, MATH)
  :type <expr>      show an expression's type without evaluating   (:t)
  :list             show the current imports and definitions       (:l)
  :reset            clear all imports and definitions
  :help             show this help                                 (:?)
  :quit             leave the shell (or Ctrl-D)                     (:q)
";
