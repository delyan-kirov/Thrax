//! Progress lines: what a command is doing, printed to stderr as it goes, so a
//! slow compile does not look frozen.
//!
//! A command [`begin`]s with the number of steps it takes, then opens a line
//! per [`step`], numbered. Each line opens with the time since the start and
//! closes with how long the step took, a clock that ticks in place while it runs
//! and stops when the next step begins:
//!
//! ```text
//!    0.0s  [1/3] PARSING  1.3s
//!    1.3s  [2/3] GENERATING C  0.3s
//!    1.6s  [3/3] COMPILING  51.7s
//! ```
//!
//! A last step whose own output follows (the program `run` starts) is announced
//! with [`end_with`], which has no clock.
//!
//! Only the newest line is ever rewritten (with `\r`, no escape sequences, so any
//! terminal works), and anything else printed meanwhile would land on it, so a
//! writer calls [`seal`] first: the clock stops there and the line is ended.
//!
//! Whether stderr is a terminal is the caller's decision: every call before
//! [`begin`] (or after [`end`]) is a no-op, so a pipe, a file, or an editor
//! session never sees a line.

use std::io::Write;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;
use std::time::{Duration, Instant};

const TICK: Duration = Duration::from_millis(100);

static ACTIVE: AtomicBool = AtomicBool::new(false);

static STATE: Mutex<State> = Mutex::new(State {
    started: None,
    steps: 0,
    done: 0,
    width: 0,
    open: None,
    ticking: false,
});

struct State {
    started: Option<Instant>,
    steps: usize,
    /// Steps started so far.
    done: usize,
    width: usize,
    /// The newest line, whose clock is still running.
    open: Option<Open>,
    ticking: bool,
}

struct Open {
    text: String,
    since: Instant,
    /// The clock as last drawn, so a tick that would not change it draws nothing.
    drawn: String,
}

fn state() -> std::sync::MutexGuard<'static, State> {
    STATE.lock().unwrap_or_else(|e| e.into_inner())
}

/// Start reporting a command of `steps` steps on a terminal `width` columns
/// wide (0 when unknown).
pub fn begin(steps: usize, width: usize) {
    let mut s = state();
    s.started = Some(Instant::now());
    s.steps = steps;
    s.done = 0;
    s.width = if width == 0 { 80 } else { width };
    ACTIVE.store(true, Ordering::SeqCst);
    if !s.ticking {
        s.ticking = true;
        std::thread::spawn(tick);
    }
}

/// Whether a command is reporting.
pub fn active() -> bool {
    ACTIVE.load(Ordering::Relaxed)
}

/// Start the next step, `name`, ending the one before.
pub fn step(name: &str) {
    if !active() {
        return;
    }
    let mut s = state();
    close(&mut s);
    let text = heading(&mut s, name);
    s.open = Some(Open { text, since: Instant::now(), drawn: String::new() });
    draw(&mut s, true);
}

/// Stop reporting with a last step, `name`, that has no clock.
pub fn end_with(name: &str) {
    if !active() {
        return;
    }
    let mut s = state();
    close(&mut s);
    let line = format!("{}\n", heading(&mut s, name));
    let _ = std::io::stderr().write_all(line.as_bytes());
    ACTIVE.store(false, Ordering::SeqCst);
}

/// `   1.3s  [2/3] GENERATING C`, counting the step.
fn heading(s: &mut State, name: &str) -> String {
    s.done += 1;
    assert!(s.done <= s.steps, "more steps than the command declared");
    let started = s.started.expect("reporting");
    format!("{:>7}  [{}/{}] {name}", clock(started.elapsed()), s.done, s.steps)
}

/// End the newest line, so other output starts on a line of its own.
pub fn seal() {
    if !active() {
        return;
    }
    close(&mut state());
}

/// Stop reporting, ending the newest line.
pub fn end() {
    if !active() {
        return;
    }
    let mut s = state();
    close(&mut s);
    ACTIVE.store(false, Ordering::SeqCst);
}

fn tick() {
    loop {
        std::thread::sleep(TICK);
        let mut s = state();
        // Checked under the lock `end` clears it under, so nothing draws after it.
        if active() {
            draw(&mut s, false);
        }
    }
}

fn close(s: &mut State) {
    if s.open.is_some() {
        draw(s, true);
        let _ = std::io::stderr().write_all(b"\n");
        s.open = None;
    }
}

/// Redraw the newest line, unless its clock reads as it did (`force` aside).
fn draw(s: &mut State, force: bool) {
    let width = s.width;
    let Some(open) = s.open.as_mut() else { return };
    let clock = clock(open.since.elapsed());
    if !force && clock == open.drawn {
        return;
    }
    // One column short of the width: a character in the last column wraps the
    // cursor on some terminals, and `\r` then returns to the wrong line.
    let room = width.saturating_sub(clock.len() + 3);
    let mut text: String = open.text.chars().take(room).collect();
    if text.chars().count() < open.text.chars().count() {
        text.pop();
        text.push('~');
    }
    let line = format!("\r{text}  {clock}");
    let _ = std::io::stderr().write_all(line.as_bytes());
    open.drawn = clock;
}

/// `0.4s`, `12.0s`, `2m08s`.
pub fn clock(d: Duration) -> String {
    let secs = d.as_secs();
    if secs < 60 {
        format!("{:.1}s", d.as_secs_f64())
    } else {
        format!("{}m{:02}s", secs / 60, secs % 60)
    }
}
