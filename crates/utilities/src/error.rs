//! Shared diagnostic facility (a Rust port of the C++ `ER` module).
//!
//! A [`Diagnostic`] is a chain of [`Frame`]s, root cause *first*. Each frame is
//! anchored to a [`Span`] into the source so a caret can be drawn. Callers add
//! context on the way up ([`Diagnostic::context`]), so a frame exists only if
//! the error actually propagated through it; there is no shared error buffer.
//!
//! The public alias is [`Result`], a `std::result::Result<T, Diagnostic>`, so
//! the whole compiler threads errors with the `?` operator.

use std::fmt;

/// A 1-based source line number.
pub type Line = u32;

/// The single result type threaded through every compiler stage.
pub type Result<T> = std::result::Result<T, Diagnostic>;

/// A half-open byte range `[start, end)` into the source text.
///
/// Byte offsets (not `char` offsets) so slicing is O(1); the source is UTF-8 and
/// a span always falls on `char` boundaries because the lexer only cuts there.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Span {
    pub start: usize,
    pub end: usize,
}

impl Span {
    pub const fn new(start: usize, end: usize) -> Span {
        assert!(start <= end, "span start must not exceed end");
        Span { start, end }
    }

    /// The empty span at a single offset (used for point diagnostics like EOF).
    pub const fn at(offset: usize) -> Span {
        Span {
            start: offset,
            end: offset,
        }
    }

    pub const fn len(self) -> usize {
        self.end - self.start
    }

    pub const fn is_empty(self) -> bool {
        self.start == self.end
    }

    /// The source text this span covers.
    pub fn slice(self, source: &str) -> &str {
        &source[self.start..self.end]
    }
}

/// Stable error identities, ported one-for-one from `ER::Code`.
///
/// Kept as a closed enum so a new failure mode must be named here and every
/// `match` over codes lights up until it is handled.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Code {
    Ok,
    AsciiControlChar,
    NonAsciiChar,
    IllegalReservedChar,
    UnclosedQuote,
    InvalidEscape,
    InvalidUtf8,
    NumberParsingFailure,
    UnknownSymbol,
    ExpectedOperand,
    UnexpectedToken,
    UnbalancedParenthesis,
    ExpectedGlobal,
    Unsupported,
    TypeMismatch,
    TypeUnbound,
    TypeAnnotationRequired,
    TypeCycle,
    BadModuleName,
    FilenameMismatch,
    AmbiguousName,
    DuplicateSymbol,
    UnknownModule,
    PrivateSymbol,
    NoEntry,
    EntrySignature,
    AssertFailed,
    IntLiteralRange,
    NonExhaustiveMatch,
    UnreachableArm,
    RuntimeFault,
    ExpansionLimit,
}

impl Code {
    /// The SCREAMING_SNAKE spelling, matching the C++ `ER::pprint(Code)` output
    /// so diagnostics read the same across both implementations.
    pub fn name(self) -> &'static str {
        use Code::*;
        match self {
            Ok => "OK",
            AsciiControlChar => "ASCII_CTR_CHAR",
            NonAsciiChar => "NON_ASCII_CHAR",
            IllegalReservedChar => "ILLEGAL_RESERVED_CHAR",
            UnclosedQuote => "QUOTM_UNCLOSED",
            InvalidEscape => "INVALID_ESCAPE",
            InvalidUtf8 => "INVALID_UTF8",
            NumberParsingFailure => "NUMBER_PARSING_FAILURE",
            UnknownSymbol => "UNKNOWN_SYMBOL",
            ExpectedOperand => "EXPECTED_OPERAND",
            UnexpectedToken => "UNEXPECTED_TOKEN",
            UnbalancedParenthesis => "PARENTHESIS_UNBALANCED",
            ExpectedGlobal => "EXPECTED_GLOBAL",
            Unsupported => "UNSUPPORTED",
            TypeMismatch => "TYPE_MISMATCH",
            TypeUnbound => "TYPE_UNBOUND",
            TypeAnnotationRequired => "TYPE_ANNOTATION_REQUIRED",
            TypeCycle => "TYPE_CYCLE",
            BadModuleName => "BAD_MODULE_NAME",
            FilenameMismatch => "FILENAME_MISMATCH",
            AmbiguousName => "AMBIGUOUS_NAME",
            DuplicateSymbol => "DUPLICATE_SYMBOL",
            UnknownModule => "UNKNOWN_MODULE",
            PrivateSymbol => "PRIVATE_SYMBOL",
            NoEntry => "NO_ENTRY",
            EntrySignature => "ENTRY_SIGNATURE",
            AssertFailed => "ASSERT_FAILED",
            IntLiteralRange => "INT_LITERAL_RANGE",
            NonExhaustiveMatch => "NON_EXHAUSTIVE_MATCH",
            UnreachableArm => "UNREACHABLE_ARM",
            RuntimeFault => "RUNTIME_FAULT",
            ExpansionLimit => "EXPANSION_LIMIT",
        }
    }
}

impl fmt::Display for Code {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
}

/// Build a [`Diagnostic`] from a code, span, line, and a `format!`-style message,
/// followed by any number of `note:` (more information) and `help:` (how to fix
/// it) asides.
///
/// ```ignore
/// diag!(Code::TypeMismatch, span, line, "expected {}, found {}", a, b)
/// diag!(Code::TypeMismatch, span, line, "expected {}, found {}", a, b;
///       help: "add an `else {}` clause", binder)
/// ```
#[macro_export]
macro_rules! diag {
    ($code:expr, $span:expr, $line:expr, $fmt:literal $(, $arg:expr)*
     $(; $kind:ident : $nfmt:literal $(, $narg:expr)* )+ $(,)?) => {
        $crate::Diagnostic::error($code, $span, $line, ::std::format!($fmt $(, $arg)*))
            $(.aside($crate::Aside::keyword(::std::stringify!($kind)),
                     ::std::format!($nfmt $(, $narg)*)))+
    };
    ($code:expr, $span:expr, $line:expr, $fmt:literal $(, $arg:expr)* $(,)?) => {
        $crate::Diagnostic::error($code, $span, $line, ::std::format!($fmt $(, $arg)*))
    };
}

/// One link in a diagnostic's context chain.
#[derive(Clone, Debug)]
pub struct Frame {
    pub code: Code,
    pub span: Span,
    pub line: Line,
    pub msg: String,
}

/// A trailing line printed after the source excerpt. The message says what is
/// wrong; an aside carries everything else, so the two never run together.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Aside {
    /// Extra information: why the error arose, what the compiler saw.
    Note,
    /// A concrete way to fix it.
    Help,
}

impl Aside {
    /// The `diag!` keyword spelling (`note` / `help`).
    pub fn keyword(word: &str) -> Aside {
        match word {
            "note" => Aside::Note,
            "help" => Aside::Help,
            other => panic!("unknown diagnostic aside `{other}:` (expected `note:` or `help:`)"),
        }
    }

    fn lead(self) -> &'static str {
        match self {
            Aside::Note => "note",
            Aside::Help => "help",
        }
    }
}

/// A secondary source location, underlined with `-` and annotated with `msg`
/// (e.g. the annotation a mismatched expression was checked against).
#[derive(Clone, Debug)]
pub struct Label {
    pub span: Span,
    pub msg: String,
}

/// A diagnostic: a non-empty chain of frames, root cause first.
///
/// Context frames are appended as the error unwinds, so `frames[0]` is always
/// the original cause and `frames.last()` is the outermost context.
#[derive(Clone, Debug)]
pub struct Diagnostic {
    frames: Vec<Frame>,
    /// Secondary spans drawn in the root frame's excerpt.
    labels: Vec<Label>,
    /// Trailing `note:` / `help:` lines. Rendered notes first, then helps, each
    /// kind in the order attached.
    asides: Vec<(Aside, String)>,
}

impl Diagnostic {
    /// Start a diagnostic from its root cause.
    pub fn error(code: Code, span: Span, line: Line, msg: impl Into<String>) -> Diagnostic {
        Diagnostic {
            frames: vec![Frame {
                code,
                span,
                line,
                msg: msg.into(),
            }],
            labels: Vec::new(),
            asides: Vec::new(),
        }
    }

    /// Attach a trailing `note:` or `help:` line (builder style).
    pub fn aside(mut self, kind: Aside, text: impl Into<String>) -> Diagnostic {
        self.asides.push((kind, text.into()));
        self
    }

    /// Attach a `note:` line: more information about the error.
    pub fn with_note(self, note: impl Into<String>) -> Diagnostic {
        self.aside(Aside::Note, note)
    }

    /// Attach a `help:` line: how to fix the error.
    pub fn with_help(self, help: impl Into<String>) -> Diagnostic {
        self.aside(Aside::Help, help)
    }

    /// Underline a secondary span in the root excerpt. A span-less label
    /// (the [`Span::at`]`(0)` sentinel) is dropped: it has nothing to point at.
    pub fn with_label(mut self, span: Span, msg: impl Into<String>) -> Diagnostic {
        if span != Span::at(0) {
            self.labels.push(Label {
                span,
                msg: msg.into(),
            });
        }
        self
    }

    pub fn asides(&self) -> &[(Aside, String)] {
        &self.asides
    }

    pub fn labels(&self) -> &[Label] {
        &self.labels
    }

    /// Give the root frame a source location if it has none (the [`Span::at`]`(0)`
    /// sentinel that span-less passes like the type checker start with). A
    /// diagnostic that already carries a real span is left untouched, so the
    /// innermost pass to set a span wins.
    pub fn fill_span(mut self, span: Span) -> Diagnostic {
        if self.frames[0].span == Span::at(0) {
            self.frames[0].span = span;
        }
        self
    }

    /// Add an outer context frame (builder style, for use on the error path of
    /// `map_err`). Mirrors the C++ `CTX` macro.
    pub fn context(
        mut self,
        code: Code,
        span: Span,
        line: Line,
        msg: impl Into<String>,
    ) -> Diagnostic {
        self.frames.push(Frame {
            code,
            span,
            line,
            msg: msg.into(),
        });
        self
    }

    /// The root cause. Always present: a `Diagnostic` cannot be built empty.
    pub fn root(&self) -> &Frame {
        debug_assert!(
            !self.frames.is_empty(),
            "a Diagnostic always has a root frame"
        );
        &self.frames[0]
    }

    pub fn frames(&self) -> &[Frame] {
        &self.frames
    }

    /// Render the diagnostic with a source excerpt, root cause first. A frame
    /// that still carries the [`Span::at`]`(0)` sentinel has no location, so it
    /// gets the bare filename and no excerpt: pointing it at `1:1` would blame
    /// the file's first line for an error raised somewhere unknown.
    pub fn render(&self, source: &str, filename: &str) -> String {
        self.render_as("error", source, filename)
    }

    /// [`Self::render`] for a non-fatal diagnostic: the root frame leads with
    /// `warning` instead of `error`.
    pub fn render_warning(&self, source: &str, filename: &str) -> String {
        self.render_as("warning", source, filename)
    }

    fn render_as(&self, root_lead: &str, source: &str, filename: &str) -> String {
        let mut out = String::new();
        for (depth, frame) in self.frames.iter().enumerate() {
            let lead = if depth == 0 { root_lead } else { "note " };
            if frame.span == Span::at(0) {
                out.push_str(&format!(
                    "{lead}[{}]: {}\n  --> {filename}\n",
                    frame.code, frame.msg
                ));
                continue;
            }
            let span = anchor(source, frame.span);
            let (line, col, _, _) = locate(source, span.start);
            out.push_str(&format!(
                "{lead}[{}]: {}\n  --> {filename}:{}:{}\n",
                frame.code, frame.msg, line, col
            ));
            let mut marks = vec![Mark {
                span,
                glyph: '^',
                msg: None,
            }];
            if depth == 0 {
                marks.extend(self.labels.iter().map(|l| Mark {
                    span: anchor(source, l.span),
                    glyph: '-',
                    msg: Some(l.msg.as_str()),
                }));
            }
            out.push_str(&excerpt(source, &marks));
        }
        // Information first, then what to do about it.
        for want in [Aside::Note, Aside::Help] {
            for (kind, text) in self.asides.iter().filter(|(k, _)| *k == want) {
                out.push_str(&format!("{}: {text}\n", kind.lead()));
            }
        }
        out
    }
}

impl fmt::Display for Diagnostic {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let r = self.root();
        if r.line == 0 {
            return write!(f, "{}: {}", r.code, r.msg);
        }
        write!(f, "{}: {} (line {})", r.code, r.msg, r.line)
    }
}

impl std::error::Error for Diagnostic {}

/// The candidate most likely meant by a misspelled `name`, for a "did you
/// mean" hint: the nearest by edit distance (ignoring case), if it is close
/// enough to be a plausible typo. Ties go to the alphabetically first
/// candidate, so the hint does not depend on iteration order.
pub fn closest<'a>(name: &str, candidates: impl IntoIterator<Item = &'a str>) -> Option<&'a str> {
    let len = name.chars().count();
    // Replacing every character is not a typo, so a one-letter name only ever
    // matches another case of itself.
    let limit = (len / 3).max(1).min(len.saturating_sub(1));
    let lower = name.to_lowercase();
    let mut best: Option<(usize, &'a str)> = None;
    for c in candidates {
        if c == name {
            continue;
        }
        let d = edit_distance(&lower, &c.to_lowercase());
        if d <= limit && best.is_none_or(|(bd, bc)| (d, c) < (bd, bc)) {
            best = Some((d, c));
        }
    }
    best.map(|(_, c)| c)
}

/// Levenshtein distance over characters.
fn edit_distance(a: &str, b: &str) -> usize {
    let b: Vec<char> = b.chars().collect();
    let mut row: Vec<usize> = (0..=b.len()).collect();
    for (i, ca) in a.chars().enumerate() {
        let mut diag = row[0];
        row[0] = i + 1;
        for (j, &cb) in b.iter().enumerate() {
            let next = (diag + usize::from(ca != cb)).min(row[j] + 1).min(row[j + 1] + 1);
            diag = row[j + 1];
            row[j + 1] = next;
        }
    }
    row[b.len()]
}

/// Recover the 1-based line number and column, the full text of the line, and
/// the line's start offset, for the source position at `offset`.
fn locate(source: &str, offset: usize) -> (usize, usize, &str, usize) {
    let offset = offset.min(source.len());
    let line_start = source[..offset].rfind('\n').map_or(0, |i| i + 1);
    let line_end = source[offset..]
        .find('\n')
        .map_or(source.len(), |i| offset + i);
    let col = source[line_start..offset].chars().count() + 1;
    let line = source[..line_start].bytes().filter(|&b| b == b'\n').count() + 1;
    (line, col, &source[line_start..line_end], line_start)
}

/// Clamp `span` into `source`, and pull a point span that sits in trailing
/// whitespace (an "end of input" error) back to just after the last real
/// character, so the caret lands where the text stopped instead of on an empty
/// line past it.
fn anchor(source: &str, span: Span) -> Span {
    let end = span.end.min(source.len());
    let start = span.start.min(end);
    let text_end = source.trim_end().len();
    if start == end && start > text_end {
        return Span::at(text_end);
    }
    Span::new(start, end)
}

/// A span to underline in an excerpt: `^` for the primary, `-` for a label.
struct Mark<'a> {
    span: Span,
    glyph: char,
    msg: Option<&'a str>,
}

/// The most lines of one multi-line span to show before eliding its middle.
const MAX_SPAN_LINES: usize = 4;

/// The source lines the marks touch, each followed by one underline row per
/// mark segment on it. A mark's message sits after its last segment. Gaps
/// between shown lines collapse to a `...` row.
fn excerpt(source: &str, marks: &[Mark]) -> String {
    let starts = line_starts(source);
    // (line index, start byte, end byte, mark index, is the mark's last segment)
    let mut segments: Vec<(usize, usize, usize, usize, bool)> = Vec::new();
    for (m, mark) in marks.iter().enumerate() {
        let first = line_of(&starts, mark.span.start);
        // A span ending exactly at a line start covers nothing on that line.
        let last = if mark.span.end > mark.span.start {
            line_of(&starts, mark.span.end - 1)
        } else {
            first
        };
        let mut lines: Vec<usize> = (first..=last)
            .filter(|&l| {
                l == first || l == last || !line_text(source, &starts, l).trim().is_empty()
            })
            .collect();
        if lines.len() > MAX_SPAN_LINES {
            let tail = lines.split_off(lines.len() - 1);
            lines.truncate(MAX_SPAN_LINES - 1);
            lines.extend(tail);
        }
        let n = lines.len();
        for (k, &l) in lines.iter().enumerate() {
            let text = line_text(source, &starts, l);
            let (ls, le) = (starts[l], starts[l] + text.len());
            let indent = text.len() - text.trim_start().len();
            let s = if l == first { mark.span.start } else { ls + indent };
            let e = if l == last { mark.span.end.min(le) } else { le };
            segments.push((l, s, e.max(s), m, k + 1 == n));
        }
    }
    segments.sort_by_key(|&(l, _, _, m, _)| (l, m));

    let widest = segments.iter().map(|s| s.0 + 1).max().unwrap_or(1);
    let width = widest.to_string().len();
    let blank = " ".repeat(width);
    let mut out = format!("{blank} |\n");
    let mut shown: Option<usize> = None;
    for &(l, s, e, m, is_last) in &segments {
        let text = line_text(source, &starts, l);
        if shown != Some(l) {
            if shown.is_some_and(|p| l > p + 1) {
                out.push_str(&format!("{blank} ...\n"));
            }
            out.push_str(&format!("{:>width$} | {text}\n", l + 1));
            shown = Some(l);
        }
        // Pad with the line's own leading characters (tabs kept as tabs,
        // everything else blanked) so a tab-indented line still lines the
        // underline up: both rows hit identical tab stops.
        let col = source[starts[l]..s].chars().count();
        let pad: String = text
            .chars()
            .take(col)
            .map(|c| if c == '\t' { '\t' } else { ' ' })
            .collect();
        let len = source[s..e].chars().count().max(1);
        let mut row = format!("{blank} | {pad}{}", marks[m].glyph.to_string().repeat(len));
        if is_last {
            if let Some(msg) = marks[m].msg {
                row.push(' ');
                row.push_str(msg);
            }
        }
        out.push_str(row.trim_end());
        out.push('\n');
    }
    out
}

/// Byte offsets where each line of `source` begins.
fn line_starts(source: &str) -> Vec<usize> {
    std::iter::once(0)
        .chain(source.match_indices('\n').map(|(i, _)| i + 1))
        .collect()
}

/// The 0-based line containing byte `offset`.
fn line_of(starts: &[usize], offset: usize) -> usize {
    starts.partition_point(|&s| s <= offset).saturating_sub(1)
}

/// Line `l`'s text, without its newline.
fn line_text<'a>(source: &'a str, starts: &[usize], l: usize) -> &'a str {
    let s = starts[l];
    let e = source[s..].find('\n').map_or(source.len(), |i| s + i);
    &source[s..e]
}

#[cfg(test)]
#[path = "error_tests.rs"]
mod tests;
