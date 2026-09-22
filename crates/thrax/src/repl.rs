//! The interactive shell (`thrax repl`): a read-eval-print loop that reads
//! ordinary Thrax `$` items, so working in it feels like writing a `.thx` file.
//!
//! The shell works by evaluating global symbols. Each symbol is prefixed with
//! `$` and is considered defined when input ends or a new `$` begins; once
//! defined it is evaluated. To run effects, use an underscore name like `_= `.
//! That prefill is inserted at the start of every prompt by default and can be
//! deleted, so the user can instead define a named global symbol they can reuse
//! (`$ name = ...` defines silently, `$ with MOD` imports). The auto-inserted
//! `$ ` opening each prompt cannot be deleted. Typing `$` then Enter defines the
//! current symbol: it is evaluated, and the `$` is "moved" to the next prompt.
//! `Ctrl-J` submits the whole buffer at once, wherever the cursor sits and with no
//! trailing `$` (it arrives as LF, distinct from the Return key's CR; some terminals
//! also send `Ctrl-Enter` as LF, so it works there too). A line beginning with `:`
//! is a meta-command.
//!
//! The whole session is recompiled against the standard library on each item,
//! so definitions accumulate and errors just print and leave the session intact.
//! No external crates are allowed, so the line editor is hand-rolled on raw-mode
//! `termios`, with a movable cursor and emacs-style editing keys (character,
//! word, and line motion, the usual kill keys, undo/redo, and `C-r` reverse
//! history search). When stdin is not a terminal the shell falls back to a
//! plain line reader with the same `$`-terminates-an-item convention.

use std::io::{self, BufRead, Write};
use std::os::raw::{c_int, c_ulong};
use std::path::PathBuf;
use std::process::ExitCode;

use crate::driver;

/// The reserved binding an evaluated expression is compiled under.
const IT: &str = "__it";

/// The fixed, undeletable prefix that opens every prompt.
const PROMPT: &str = "$ ";

/// The deletable text seeded into every fresh prompt after the `$ `.
const PREFILL: &str = "_= ";

/// Marks the line reporting an evaluated item's value, echoing the `-->` arrow
/// diagnostics use so a result reads as the success analogue of an error.
const RESULT: &str = "  => ";

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
    let mut input = term::FdKeys;
    let mut ed = Editor::new();
    ed.render();
    loop {
        match term::read_key(&mut input) {
            None => {
                print!("\r\n");
                let _ = io::stdout().flush();
                break;
            }
            // Ctrl-D exits on an empty line (the readline convention), otherwise
            // it forward-deletes the character at the cursor.
            Some(Key::CtrlD) => {
                if ed.is_empty() {
                    print!("\r\n");
                    let _ = io::stdout().flush();
                    break;
                }
                ed.delete_forward();
            }
            Some(Key::CtrlC) => {
                print!("\r\n");
                ed = Editor::new();
                ed.render();
            }
            Some(Key::Backspace) => ed.backspace(),
            Some(Key::Left) => ed.move_left(),
            Some(Key::Right) => ed.move_right(),
            Some(Key::Home) => ed.move_home(),
            Some(Key::End) => ed.move_end(),
            Some(Key::Up) => ed.move_up(),
            Some(Key::Down) => ed.move_down(),
            Some(Key::PageUp) => ed.move_lines(-page_lines()),
            Some(Key::PageDown) => ed.move_lines(page_lines()),
            Some(Key::BufferStart) => ed.move_buffer_start(),
            Some(Key::BufferEnd) => ed.move_buffer_end(),
            Some(Key::ClearScreen) => ed.clear_screen(),
            Some(Key::WordLeft) => ed.move_word_left(),
            Some(Key::WordRight) => ed.move_word_right(),
            Some(Key::KillToEol) => ed.kill_to_eol(),
            Some(Key::KillWordBack) => ed.kill_word_back(),
            Some(Key::KillWordForward) => ed.kill_word_forward(),
            Some(Key::Undo) => ed.undo(),
            Some(Key::Redo) => ed.redo(),
            Some(Key::HistorySearch) => {
                ed.begin_overlay();
                if let Some(chosen) = history_search(&mut input, &state.history) {
                    ed.set_buffer(chosen);
                }
                ed.render();
            }
            Some(Key::SubmitAll) => {
                let body = ed.submit_body();
                print!("\r\n");
                let _ = io::stdout().flush();
                state.submit(&body);
                ed = Editor::new();
                ed.render();
            }
            Some(Key::Escape) => {}
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

/// Lines a page key (`C-v`/`M-v`) moves by: a screenful less a little overlap,
/// emacs-style. Falls back to a sane default when the window size is unknown.
fn page_lines() -> isize {
    term::window_rows().saturating_sub(2).max(1) as isize
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
#[derive(Debug, PartialEq)]
enum Key {
    Char(char),
    Enter,
    Backspace,
    /// Delete the character at the cursor, or exit when the line is empty.
    CtrlD,
    /// Abandon the current item.
    CtrlC,
    /// Cursor movement (arrows, or the emacs `Ctrl-B`/`Ctrl-F`/`Ctrl-A`/`Ctrl-E`).
    Left,
    Right,
    Home,
    End,
    /// Move between lines, preserving the column (arrows, or `C-p`/`C-n`).
    Up,
    Down,
    /// Move a screenful up/down (`M-v`/`C-v`).
    PageUp,
    PageDown,
    /// Jump to the start/end of the whole item (`M-<`/`M->`).
    BufferStart,
    BufferEnd,
    /// Clear the screen and redraw the prompt at the top (`C-l`).
    ClearScreen,
    /// Move a word backward/forward (`M-b`/`M-f`).
    WordLeft,
    WordRight,
    /// Kill from the cursor to the end of the line (`C-k`).
    KillToEol,
    /// Kill the word before the cursor (`M-DEL`, also `C-w`).
    KillWordBack,
    /// Kill the word after the cursor (`M-d`).
    KillWordForward,
    /// Undo (`C-/`, also `C-_`) and redo (`M-/`).
    Undo,
    Redo,
    /// Start an incremental reverse history search (`C-r`).
    HistorySearch,
    /// Evaluate the whole current buffer now, wherever the cursor sits and with no
    /// trailing `$` needed (`Ctrl-J`; also `Ctrl-Enter` in terminals that send it as
    /// LF).
    SubmitAll,
    /// Cancel / quit the current mode (`Esc`, also `C-g`).
    Escape,
}

/// The kind of the last buffer edit, so a run of same-kind edits coalesces into
/// one undo step instead of one per keystroke.
#[derive(Clone, Copy, PartialEq)]
enum Edit {
    Insert,
    Delete,
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

/// The current item's editable text, excluding the fixed `$ ` prompt, plus the
/// cursor as a byte offset into it (always on a `char` boundary). Rendering
/// reprints the whole block in place and then seats the terminal cursor at
/// `cursor`, so editing in the middle of a multi-line item needs no per-key
/// cursor bookkeeping. Terminal line wrapping is not accounted for, and columns
/// are counted in `char`s, so double-width glyphs may misplace the cursor.
struct Editor {
    buffer: String,
    cursor: usize,
    /// The block row (0 = prompt line) the terminal cursor was left on by the
    /// last render. The cursor can sit on any row after a vertical or mid-line
    /// move, so the next render rewinds by exactly this many rows to reach the
    /// block's top. Assuming the bottom row instead would walk the block up the
    /// screen on every such edit.
    cursor_row: usize,
    /// Snapshots of `(buffer, cursor)` taken before edits, and their inverse for
    /// redo. `last_edit` coalesces a run of same-kind edits into one step.
    undo: Vec<(String, usize)>,
    redo: Vec<(String, usize)>,
    last_edit: Option<Edit>,
}

impl Editor {
    fn new() -> Self {
        Editor {
            buffer: PREFILL.to_string(),
            cursor: PREFILL.len(),
            cursor_row: 0,
            undo: Vec::new(),
            redo: Vec::new(),
            last_edit: None,
        }
    }

    fn set_buffer(&mut self, buffer: String) {
        self.cursor = buffer.len();
        self.buffer = buffer;
        self.undo.clear();
        self.redo.clear();
        self.last_edit = None;
    }

    fn is_empty(&self) -> bool {
        self.buffer.is_empty()
    }

    /// Snapshot the current state before a same-kind run of edits, coalescing so
    /// typing or deleting in a row is one undo step. Any edit drops the redo
    /// stack, since the timeline has branched.
    fn record(&mut self, kind: Edit) {
        if self.last_edit != Some(kind) {
            self.undo.push((self.buffer.clone(), self.cursor));
            self.last_edit = Some(kind);
        }
        self.redo.clear();
    }

    /// Snapshot unconditionally, for a discrete edit (a kill) that should be its
    /// own undo step rather than coalescing with neighbors.
    fn checkpoint(&mut self) {
        self.undo.push((self.buffer.clone(), self.cursor));
        self.redo.clear();
        self.last_edit = None;
    }

    /// End the current coalescing run, so the next edit starts a fresh step.
    fn break_run(&mut self) {
        self.last_edit = None;
    }

    fn undo(&mut self) {
        if let Some(prev) = self.undo.pop() {
            self.redo.push((self.buffer.clone(), self.cursor));
            (self.buffer, self.cursor) = prev;
            self.last_edit = None;
            self.render();
        }
    }

    fn redo(&mut self) {
        if let Some(next) = self.redo.pop() {
            self.undo.push((self.buffer.clone(), self.cursor));
            (self.buffer, self.cursor) = next;
            self.last_edit = None;
            self.render();
        }
    }

    fn insert(&mut self, c: char) {
        self.record(Edit::Insert);
        self.buffer.insert(self.cursor, c);
        self.cursor += c.len_utf8();
        self.render();
    }

    fn newline(&mut self) {
        self.break_run();
        self.insert('\n');
        self.break_run();
    }

    /// Delete the character before the cursor.
    fn backspace(&mut self) {
        if let Some(prev) = self.buffer[..self.cursor].chars().next_back() {
            self.record(Edit::Delete);
            self.cursor -= prev.len_utf8();
            self.buffer.remove(self.cursor);
            self.render();
        }
    }

    /// Delete the character at the cursor.
    fn delete_forward(&mut self) {
        if self.cursor < self.buffer.len() {
            self.record(Edit::Delete);
            self.buffer.remove(self.cursor);
            self.render();
        }
    }

    fn move_left(&mut self) {
        self.break_run();
        if let Some(prev) = self.buffer[..self.cursor].chars().next_back() {
            self.cursor -= prev.len_utf8();
            self.render();
        }
    }

    fn move_right(&mut self) {
        self.break_run();
        if let Some(next) = self.buffer[self.cursor..].chars().next() {
            self.cursor += next.len_utf8();
            self.render();
        }
    }

    fn move_home(&mut self) {
        self.break_run();
        self.cursor = self.line_bounds().0;
        self.render();
    }

    fn move_end(&mut self) {
        self.break_run();
        self.cursor = self.line_bounds().1;
        self.render();
    }

    fn move_word_left(&mut self) {
        self.break_run();
        self.cursor = self.prev_word();
        self.render();
    }

    fn move_word_right(&mut self) {
        self.break_run();
        self.cursor = self.next_word();
        self.render();
    }

    /// The start of the word at or before the cursor: skip any non-word
    /// characters going left, then the word's own characters.
    fn prev_word(&self) -> usize {
        let mut i = self.cursor;
        while let Some(c) = self.buffer[..i].chars().next_back() {
            if c.is_alphanumeric() {
                break;
            }
            i -= c.len_utf8();
        }
        while let Some(c) = self.buffer[..i].chars().next_back() {
            if !c.is_alphanumeric() {
                break;
            }
            i -= c.len_utf8();
        }
        i
    }

    /// The end of the word at or after the cursor.
    fn next_word(&self) -> usize {
        let mut i = self.cursor;
        while let Some(c) = self.buffer[i..].chars().next() {
            if c.is_alphanumeric() {
                break;
            }
            i += c.len_utf8();
        }
        while let Some(c) = self.buffer[i..].chars().next() {
            if !c.is_alphanumeric() {
                break;
            }
            i += c.len_utf8();
        }
        i
    }

    fn move_up(&mut self) {
        self.move_lines(-1);
    }

    fn move_down(&mut self) {
        self.move_lines(1);
    }

    /// Move `delta` lines (negative = up), keeping the same column, clamped to
    /// the first/last line and to the target line's length.
    fn move_lines(&mut self, delta: isize) {
        self.break_run();
        let starts = self.line_starts();
        let here = self.buffer[..self.cursor].matches('\n').count();
        let col = self.buffer[starts[here]..self.cursor].chars().count();
        let target = (here as isize + delta).clamp(0, starts.len() as isize - 1) as usize;
        let start = starts[target];
        let end = starts
            .get(target + 1)
            .map_or(self.buffer.len(), |next| next - 1);
        self.cursor = self.seek_col(start, end, col);
        self.render();
    }

    fn move_buffer_start(&mut self) {
        self.break_run();
        self.cursor = 0;
        self.render();
    }

    fn move_buffer_end(&mut self) {
        self.break_run();
        self.cursor = self.buffer.len();
        self.render();
    }

    /// Clear the terminal and redraw the prompt at the top.
    fn clear_screen(&mut self) {
        print!("\x1b[2J\x1b[H");
        self.cursor_row = 0;
        self.render();
    }

    /// Rewind to the block's top-left and clear it, so a transient display (the
    /// history-search prompt) can take over. A following `render` redraws the
    /// block from there.
    fn begin_overlay(&mut self) {
        let mut out = String::new();
        if self.cursor_row > 0 {
            out.push_str(&format!("\x1b[{}A", self.cursor_row));
        }
        out.push_str("\r\x1b[J");
        print!("{out}");
        let _ = io::stdout().flush();
        self.cursor_row = 0;
    }

    /// The byte offset of each line's first character.
    fn line_starts(&self) -> Vec<usize> {
        let mut starts = vec![0];
        starts.extend(
            self.buffer
                .match_indices('\n')
                .map(|(i, _)| i + 1),
        );
        starts
    }

    /// The byte offset `col` characters into the line `[start, end)`, clamped to
    /// `end` when the line is shorter than `col`.
    fn seek_col(&self, start: usize, end: usize, col: usize) -> usize {
        let mut i = start;
        for _ in 0..col {
            match self.buffer[i..end].chars().next() {
                Some(c) => i += c.len_utf8(),
                None => break,
            }
        }
        i
    }

    /// Kill from the cursor to the end of the current line; at end of a line,
    /// swallow the newline so the next line joins this one.
    fn kill_to_eol(&mut self) {
        let (_, end) = self.line_bounds();
        if end > self.cursor {
            self.checkpoint();
            self.buffer.replace_range(self.cursor..end, "");
        } else if self.cursor < self.buffer.len() {
            self.checkpoint();
            self.buffer.remove(self.cursor);
        }
        self.render();
    }

    /// Kill the word before the cursor.
    fn kill_word_back(&mut self) {
        let start = self.prev_word();
        if start < self.cursor {
            self.checkpoint();
            self.buffer.replace_range(start..self.cursor, "");
            self.cursor = start;
            self.render();
        }
    }

    /// Kill the word after the cursor.
    fn kill_word_forward(&mut self) {
        let end = self.next_word();
        if end > self.cursor {
            self.checkpoint();
            self.buffer.replace_range(self.cursor..end, "");
            self.render();
        }
    }

    /// The byte range `[start, end)` of the buffer line the cursor sits on.
    fn line_bounds(&self) -> (usize, usize) {
        let start = self.buffer[..self.cursor].rfind('\n').map_or(0, |i| i + 1);
        let end = self.buffer[self.cursor..]
            .find('\n')
            .map_or(self.buffer.len(), |i| self.cursor + i);
        (start, end)
    }

    /// The whole buffer as a submittable item: trimmed, with a single optional
    /// trailing `$` terminator removed. Used by `Ctrl-Enter`, which submits the
    /// buffer outright rather than on the trailing-`$` convention.
    fn submit_body(&self) -> String {
        let trimmed = self.buffer.trim_end();
        trimmed.strip_suffix('$').unwrap_or(trimmed).trim().to_string()
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

    /// Reprint the block (`$ ` + buffer) in place, then seat the terminal cursor
    /// at `cursor`.
    fn render(&mut self) {
        let mut out = String::new();
        // Rewind from the cursor's current row to the block's top, then clear
        // everything below and reprint.
        if self.cursor_row > 0 {
            out.push_str(&format!("\x1b[{}A", self.cursor_row));
        }
        out.push_str("\r\x1b[J");
        out.push_str(PROMPT);
        out.push_str(&self.buffer);

        let total = 1 + self.buffer.matches('\n').count();
        let before = &self.buffer[..self.cursor];
        let row = before.matches('\n').count();
        let line = before.rsplit('\n').next().unwrap_or("");
        let col = line.chars().count() + if row == 0 { PROMPT.len() } else { 0 };
        // Printing left the cursor on the block's last row; move back up to the
        // cursor's row and across to its column.
        if total - 1 > row {
            out.push_str(&format!("\x1b[{}A", total - 1 - row));
        }
        out.push('\r');
        if col > 0 {
            out.push_str(&format!("\x1b[{col}C"));
        }

        print!("{out}");
        let _ = io::stdout().flush();
        self.cursor_row = row;
    }
}

/// An incremental reverse history search (`C-r`). Refine with typing/Backspace;
/// Up (`C-p`) or another `C-r` steps to an older match, Down (`C-n`) to a newer
/// one; Enter returns the highlighted item, Escape or `C-g` cancels. Renders a
/// single `(reverse-i-search)` line in the overlay the caller opened.
fn history_search<K: term::Keys>(input: &mut K, history: &[String]) -> Option<String> {
    let mut query = String::new();
    let mut idx = 0usize;
    loop {
        let matches = search_matches(history, &query);
        idx = idx.min(matches.len().saturating_sub(1));
        let current = matches.get(idx).copied();
        render_search(&query, current);
        match term::read_key(input) {
            Some(Key::Enter) => {
                clear_overlay_line();
                return current.cloned();
            }
            Some(Key::Escape) | Some(Key::CtrlC) | None => {
                clear_overlay_line();
                return None;
            }
            Some(Key::Backspace) => {
                query.pop();
                idx = 0;
            }
            Some(Key::Char(c)) => {
                query.push(c);
                idx = 0;
            }
            Some(Key::Up) | Some(Key::HistorySearch) if idx + 1 < matches.len() => idx += 1,
            Some(Key::Down) => idx = idx.saturating_sub(1),
            _ => {}
        }
    }
}

/// History entries containing `query`, newest first.
fn search_matches<'a>(history: &'a [String], query: &str) -> Vec<&'a String> {
    history.iter().rev().filter(|e| e.contains(query)).collect()
}

/// Draw the search prompt in place. A multi-line match is previewed on one line.
fn render_search(query: &str, current: Option<&String>) {
    let preview = current.map(|s| s.replace('\n', " ")).unwrap_or_default();
    print!("\r\x1b[J(reverse-i-search)'{query}': {preview}");
    let _ = io::stdout().flush();
}

fn clear_overlay_line() {
    print!("\r\x1b[J");
    let _ = io::stdout().flush();
}

struct Repl {
    root_dir: PathBuf,
    /// Full `$ with MOD` lines, in entry order, deduplicated.
    imports: Vec<String>,
    /// Accumulated definitions as `(binding name, full source item)`. Redefining
    /// a name replaces its entry, so the module never has a duplicate `$ name`.
    defs: Vec<(String, String)>,
    /// Bodies of submitted items in entry order, for reverse history search.
    history: Vec<String>,
}

impl Repl {
    fn new(root_dir: PathBuf) -> Self {
        Repl {
            root_dir,
            imports: Vec::new(),
            defs: Vec::new(),
            history: Vec::new(),
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
        if self.history.last().map(String::as_str) != Some(body) {
            self.history.push(body.to_string());
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
        let ty = session
            .decls
            .iter()
            .find(|(n, _)| n == IT)
            .map(|(_, t)| t.to_string());
        // Any output from effects the expression runs has already printed to
        // stdout by this point; the `=>` line reports the value it evaluated to
        // (and its type), keeping that boundary visible.
        match interpreter::machine::eval(&ir, IT) {
            Ok(shown) => match ty {
                Some(ty) => println!("{RESULT}{shown} : {ty}"),
                None => println!("{RESULT}{shown}"),
            },
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

    /// Print `expr : <type>` without evaluating it.
    fn show_type(&self, expr: &str) {
        let src = self.source(&format!("$ {IT} = {expr}\n"));
        match driver::compile_session(&src, &self.root_dir) {
            Ok(session) => match session.decls.iter().find(|(n, _)| n == IT) {
                Some((_, ty)) => println!("{expr} : {ty}"),
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
may span lines; type `$` then Enter to submit the item you are editing, or press
Ctrl-J to submit the whole buffer at once. An evaluated item's value is reported on
a `=>` line, with its type.

  $ _= <expr>       evaluate an expression and print its value and type
  $ name = <expr>   add (or redefine) a binding (silent, like a file)
  $ with MOD        import a standard-library module (e.g. VEC, STR, MATH)
  :type <expr>      show an expression's type without evaluating   (:t)
  :list             show the current imports and definitions       (:l)
  :reset            clear all imports and definitions
  :help             show this help                                 (:?)
  :quit             leave the shell (or Ctrl-D on an empty line)    (:q)

Emacs editing keys:
  C-b/C-f (or arrows) character    M-b/M-f word      C-p/C-n (or Up/Down) line
  C-a/C-e line start/end           M-</M-> item start/end
  C-v/M-v page down/up             C-l clear screen
  C-k kill to end of line          M-d/M-DEL kill next/previous word (C-w = M-DEL)
  C-d delete char (or exit on an empty line)
  C-/ undo                         M-/ redo
  C-r reverse history search: type to match, C-r/Up older, C-n/Down newer,
      Enter to accept, Esc (or C-g) to cancel
  C-j submit (evaluate) the whole buffer now, no trailing `$` needed
";

#[cfg(test)]
#[path = "repl_tests.rs"]
mod tests;

/// Raw-mode terminal support, built directly on libc `termios` (no crates).
mod term {
    use super::{c_int, c_ulong, Key};

    const STDIN: c_int = 0;
    const TCSANOW: c_int = 0;
    const TIOCGWINSZ: c_ulong = 0x5413; // Linux "get window size" ioctl
    const POLLIN: i16 = 0x001;
    /// How long to wait for the rest of an escape sequence before deciding a
    /// lone `ESC` was a keypress, not the start of one. Terminals emit a whole
    /// sequence in one burst, so this is imperceptible yet unambiguous.
    const ESC_WAIT_MS: c_int = 20;
    // Linux `c_iflag`, `c_lflag` bits and `c_cc` indices.
    const ICRNL: u32 = 0x0000_0100; // input: translate a received CR to NL
    const ISIG: u32 = 0x0000_0001;
    const ICANON: u32 = 0x0000_0002;
    const ECHO: u32 = 0x0000_0008;
    const IEXTEN: u32 = 0x0000_8000;
    const VMIN: usize = 6;
    const VTIME: usize = 5;

    // Control bytes the editor acts on. The emacs movement/kill keys arrive as
    // their Ctrl- code points; `Ctrl-<letter>` is the letter's position in the
    // alphabet (Ctrl-A = 1, ...). The rest are the usual ASCII names.
    const CTRL_A: u8 = 0x01; // start of line
    const CTRL_B: u8 = 0x02; // back one char
    const CTRL_C: u8 = 0x03; // abandon item
    const CTRL_D: u8 = 0x04; // delete char, or exit on an empty line
    const CTRL_E: u8 = 0x05; // end of line
    const CTRL_F: u8 = 0x06; // forward one char
    const CTRL_G: u8 = 0x07; // cancel (keyboard-quit)
    const CTRL_H: u8 = 0x08; // backspace
    const CTRL_K: u8 = 0x0b; // kill to end of line
    const CTRL_L: u8 = 0x0c; // clear screen
    const CTRL_N: u8 = 0x0e; // next line
    const CTRL_P: u8 = 0x10; // previous line
    const CTRL_R: u8 = 0x12; // reverse history search
    const CTRL_V: u8 = 0x16; // page down
    const CTRL_W: u8 = 0x17; // kill previous word
    const ESC: u8 = 0x1b; // introduces an arrow/cursor escape sequence
    const CR: u8 = b'\r';
    const LF: u8 = b'\n';
    const UNDO: u8 = 0x1f; // C-/ and C-_ both arrive as this byte
    const DEL: u8 = 0x7f; // what most terminals send for Backspace
    const LAST_CONTROL: u8 = 0x1f; // end of the C0 control range

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

    #[repr(C)]
    struct Winsize {
        rows: u16,
        cols: u16,
        xpixel: u16,
        ypixel: u16,
    }

    #[repr(C)]
    struct Pollfd {
        fd: c_int,
        events: i16,
        revents: i16,
    }

    extern "C" {
        fn tcgetattr(fd: c_int, termios: *mut Termios) -> c_int;
        fn tcsetattr(fd: c_int, actions: c_int, termios: *const Termios) -> c_int;
        fn ioctl(fd: c_int, request: c_ulong, argp: *mut Winsize) -> c_int;
        fn read(fd: c_int, buf: *mut u8, count: usize) -> isize;
        fn poll(fds: *mut Pollfd, nfds: c_ulong, timeout: c_int) -> c_int;
    }

    /// A source of keypresses. Abstracting over the real terminal lets the
    /// decoder be driven by a byte slice in tests. `pending` reports whether a
    /// byte is available within `timeout_ms`, used to tell a lone `ESC` from the
    /// start of an escape sequence.
    pub trait Keys {
        fn byte(&mut self) -> Option<u8>;
        fn pending(&mut self, timeout_ms: c_int) -> bool;
    }

    /// The real terminal: unbuffered `read`s straight from the stdin fd, so a
    /// `poll` on that fd sees exactly what is pending (a buffered reader could
    /// hold bytes the poll would miss).
    pub struct FdKeys;

    impl Keys for FdKeys {
        fn byte(&mut self) -> Option<u8> {
            let mut b = 0u8;
            match unsafe { read(STDIN, &mut b, 1) } {
                1 => Some(b),
                _ => None,
            }
        }

        fn pending(&mut self, timeout_ms: c_int) -> bool {
            let mut pfd = Pollfd {
                fd: STDIN,
                events: POLLIN,
                revents: 0,
            };
            unsafe { poll(&mut pfd, 1, timeout_ms) > 0 }
        }
    }

    impl Keys for &[u8] {
        fn byte(&mut self) -> Option<u8> {
            let (&b, rest) = self.split_first()?;
            *self = rest;
            Some(b)
        }

        fn pending(&mut self, _timeout_ms: c_int) -> bool {
            !self.is_empty()
        }
    }

    /// The terminal's height in rows, or 24 when it cannot be determined.
    pub fn window_rows() -> usize {
        let mut ws = Winsize {
            rows: 0,
            cols: 0,
            xpixel: 0,
            ypixel: 0,
        };
        if unsafe { ioctl(STDIN, TIOCGWINSZ, &mut ws) } == 0 && ws.rows > 0 {
            ws.rows as usize
        } else {
            24
        }
    }

    /// Puts the terminal into a minimally-raw mode for the editor's lifetime:
    /// echo, canonical line buffering, and signal keys are off, so keys arrive
    /// one at a time and Ctrl-C/D come through as bytes. `ICRNL` is cleared too, so
    /// a received CR is not folded into NL: the Return key arrives as CR (`\r`) and
    /// `Ctrl-J` as LF (`\n`), letting the editor tell them apart. Output
    /// post-processing (`\n` -> CRLF) is left on, so ordinary `println!` still
    /// works. The original settings are restored on drop. `enable` returns `None`
    /// when stdin is not a terminal.
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
            raw.c_iflag &= !ICRNL;
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

    /// Read and decode the next keypress, skipping unrecognized escape sequences
    /// and stray control bytes. `None` on end of input.
    pub fn read_key(input: &mut impl Keys) -> Option<Key> {
        loop {
            let b = input.byte()?;
            match b {
                CTRL_A => return Some(Key::Home),
                CTRL_E => return Some(Key::End),
                CTRL_B => return Some(Key::Left),
                CTRL_F => return Some(Key::Right),
                CTRL_P => return Some(Key::Up),
                CTRL_N => return Some(Key::Down),
                CTRL_V => return Some(Key::PageDown),
                CTRL_L => return Some(Key::ClearScreen),
                CTRL_K => return Some(Key::KillToEol),
                CTRL_W => return Some(Key::KillWordBack),
                CTRL_R => return Some(Key::HistorySearch),
                CTRL_G => return Some(Key::Escape),
                UNDO => return Some(Key::Undo),
                CTRL_C => return Some(Key::CtrlC),
                CTRL_D => return Some(Key::CtrlD),
                // With `ICRNL` off, the Return key is CR and `Ctrl-J` is LF, so the
                // two split: Return edits/submits an item, `Ctrl-J` evaluates the
                // whole buffer at once (like `Ctrl-Enter` in editors that can send it).
                CR => return Some(Key::Enter),
                LF => return Some(Key::SubmitAll),
                DEL | CTRL_H => return Some(Key::Backspace),
                ESC => {
                    if let Some(key) = read_escape(input) {
                        return Some(key);
                    }
                }
                0..=LAST_CONTROL => {} // other control bytes: ignore
                _ => {
                    if let Some(c) = decode_char(b, input) {
                        return Some(Key::Char(c));
                    }
                }
            }
        }
    }

    /// Decode the tail of an escape sequence (the `ESC` has been read). With no
    /// byte waiting, the `ESC` was a keypress of its own. Otherwise a CSI
    /// (`ESC [`) or SS3 (`ESC O`) sequence is a cursor key, selected by its final
    /// byte, and any other `ESC`-prefixed key is the matching `Meta` (`Alt`)
    /// binding: `M-b`/`M-f` word motion, `M-v` page up, `M-<`/`M->` item ends,
    /// `M-/` redo, `M-d`/`M-DEL` word kills. Unknown sequences are discarded.
    fn read_escape(input: &mut impl Keys) -> Option<Key> {
        if !input.pending(ESC_WAIT_MS) {
            return Some(Key::Escape);
        }
        match input.byte()? {
            b'[' | b'O' => {
                let mut last = 0;
                while let Some(x) = input.byte() {
                    last = x;
                    if is_csi_final(x) {
                        break;
                    }
                }
                match last {
                    b'C' => Some(Key::Right),
                    b'D' => Some(Key::Left),
                    b'A' => Some(Key::Up),
                    b'B' => Some(Key::Down),
                    b'H' => Some(Key::Home),
                    b'F' => Some(Key::End),
                    _ => None,
                }
            }
            b'b' | b'B' => Some(Key::WordLeft),
            b'f' | b'F' => Some(Key::WordRight),
            b'v' | b'V' => Some(Key::PageUp),
            b'<' => Some(Key::BufferStart),
            b'>' => Some(Key::BufferEnd),
            b'/' => Some(Key::Redo),
            b'd' | b'D' => Some(Key::KillWordForward),
            DEL | CTRL_H => Some(Key::KillWordBack),
            _ => None,
        }
    }

    /// A byte that ends a CSI escape sequence (`@`..`~`).
    fn is_csi_final(b: u8) -> bool {
        (0x40..=0x7e).contains(&b)
    }

    /// Decode one character from `lead` and, for a multibyte scalar, its
    /// continuation bytes. A UTF-8 lead byte's count of high 1-bits is the
    /// scalar's total byte length, so `std` does the actual validation and
    /// decoding in one place.
    fn decode_char(lead: u8, input: &mut impl Keys) -> Option<char> {
        if lead.is_ascii() {
            return Some(lead as char);
        }
        let len = lead.leading_ones() as usize;
        if !(2..=4).contains(&len) {
            return None;
        }
        let mut bytes = vec![lead];
        for _ in 1..len {
            bytes.push(input.byte()?);
        }
        std::str::from_utf8(&bytes).ok()?.chars().next()
    }
}
