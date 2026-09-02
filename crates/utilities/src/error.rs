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
    RuntimeFault,
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
            RuntimeFault => "RUNTIME_FAULT",
        }
    }
}

impl fmt::Display for Code {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
}

/// Build a [`Diagnostic`] from a code, span, line, and a `format!`-style message,
/// with an optional trailing `note:` fix hint.
///
/// ```ignore
/// diag!(Code::TypeMismatch, span, line, "expected {}, found {}", a, b)
/// diag!(Code::TypeMismatch, span, line, "expected {}, found {}", a, b;
///       note: "add an `else {}` clause", binder)
/// ```
#[macro_export]
macro_rules! diag {
    ($code:expr, $span:expr, $line:expr, $fmt:literal $(, $arg:expr)*
     ; note: $nfmt:literal $(, $narg:expr)* $(,)?) => {
        $crate::Diagnostic::error($code, $span, $line, ::std::format!($fmt $(, $arg)*))
            .with_note(::std::format!($nfmt $(, $narg)*))
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

/// A diagnostic: a non-empty chain of frames, root cause first.
///
/// Context frames are appended as the error unwinds, so `frames[0]` is always
/// the original cause and `frames.last()` is the outermost context.
#[derive(Clone, Debug)]
pub struct Diagnostic {
    frames: Vec<Frame>,
    /// An optional closing hint on how to fix the error. Rendered as a trailing
    /// `note: ...` line, after the source caret, with no code or location.
    note: Option<String>,
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
            note: None,
        }
    }

    /// Attach a closing hint on how to fix the error (builder style).
    pub fn with_note(mut self, note: impl Into<String>) -> Diagnostic {
        self.note = Some(note.into());
        self
    }

    /// Give the root frame a source location if it has none (the [`Span::at(0)`]
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

    /// Render the diagnostic with a source caret, root cause first.
    pub fn render(&self, source: &str, filename: &str) -> String {
        let mut out = String::new();
        for (depth, frame) in self.frames.iter().enumerate() {
            let (line, col, line_text) = locate(source, frame.span.start);
            let lead = if depth == 0 { "error" } else { "note " };
            out.push_str(&format!(
                "{lead}[{}]: {}\n  --> {filename}:{}:{}\n",
                frame.code, frame.msg, line, col
            ));
            out.push_str(&format!("   | {}\n", line_text));
            // Pad the caret with the line's own leading characters (tabs kept as
            // tabs, everything else blanked to a space) so a tab-indented line
            // still lines the caret up under the span: both rows hit identical
            // tab stops.
            let caret_pad: String = line_text
                .chars()
                .take(col.saturating_sub(1))
                .map(|c| if c == '\t' { '\t' } else { ' ' })
                .collect();
            let caret_len = frame.span.len().max(1);
            out.push_str(&format!("   | {}{}\n", caret_pad, "^".repeat(caret_len)));
        }
        if let Some(note) = &self.note {
            out.push_str(&format!("note: {note}\n"));
        }
        out
    }
}

impl fmt::Display for Diagnostic {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let r = self.root();
        write!(f, "{}: {} (line {})", r.code, r.msg, r.line)
    }
}

impl std::error::Error for Diagnostic {}

/// Recover the 1-based line number and column, plus the full text of the line,
/// for the source position at `offset`.
fn locate(source: &str, offset: usize) -> (usize, usize, &str) {
    let offset = offset.min(source.len());
    let line_start = source[..offset].rfind('\n').map_or(0, |i| i + 1);
    let line_end = source[offset..]
        .find('\n')
        .map_or(source.len(), |i| offset + i);
    let col = source[line_start..offset].chars().count() + 1;
    let line = source[..line_start].bytes().filter(|&b| b == b'\n').count() + 1;
    (line, col, &source[line_start..line_end])
}

#[cfg(test)]
#[path = "error_tests.rs"]
mod tests;
