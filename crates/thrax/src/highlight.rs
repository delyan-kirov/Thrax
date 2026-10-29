//! Lexical highlighting, shared by the language server (semantic tokens) and the
//! shell (ANSI colors on the line being edited). It is the lexer alone: no
//! parsing or typing, so it is cheap enough to run on every keystroke and works
//! on text that does not parse yet.

use frontend::{Kind, Lexer};
use utilities::Span;

/// What a token is, for coloring. The order is the language server's legend.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Class {
    Keyword,
    Comment,
    String,
    Number,
    Operator,
    Type,
    Function,
    Macro,
    Variable,
}

impl Class {
    pub const ALL: [Class; 9] = [
        Class::Keyword,
        Class::Comment,
        Class::String,
        Class::Number,
        Class::Operator,
        Class::Type,
        Class::Function,
        Class::Macro,
        Class::Variable,
    ];

    /// The LSP semantic token type name.
    pub fn lsp_name(self) -> &'static str {
        match self {
            Class::Keyword => "keyword",
            Class::Comment => "comment",
            Class::String => "string",
            Class::Number => "number",
            Class::Operator => "operator",
            Class::Type => "type",
            Class::Function => "function",
            Class::Macro => "macro",
            Class::Variable => "variable",
        }
    }

    /// The SGR parameters the shell paints this class with, or `None` to leave
    /// it in the terminal's default color. Only the 16 base colors, so the
    /// user's terminal theme decides the actual shades.
    fn sgr(self) -> Option<&'static str> {
        match self {
            Class::Keyword => Some("35"),
            Class::Comment => Some("90"),
            Class::String => Some("32"),
            Class::Number => Some("33"),
            Class::Type => Some("36"),
            Class::Macro => Some("34"),
            Class::Operator | Class::Function | Class::Variable => None,
        }
    }
}

/// The class of a token with source text `text`, or `None` for punctuation.
pub fn class(kind: Kind, text: &str) -> Option<Class> {
    use frontend::Kind::*;
    Some(match kind {
        Let | In | If | Is | Else | With | Do | Ctl | Defer => Class::Keyword,
        Comment => Class::Comment,
        Str => Class::String,
        Int(_) | Real(_) | Imaginary(_) => Class::Number,
        Op | Eq | FatArrow | Arrow | Lambda | Colon | Dollar | Ellipsis => Class::Operator,
        Word if text.starts_with(|c: char| c.is_ascii_uppercase()) => Class::Type,
        Word => Class::Variable,
        At if matches!(text, "@true" | "@false") => Class::Number,
        At if frontend::typing::BASE_TYPES.contains(&text) => Class::Type,
        At => Class::Macro,
        _ => return None,
    })
}

/// The classified spans of `src`, in order. Text after a lexical error is
/// dropped, except an unclosed string, which runs to the end as one.
pub fn spans(src: &str) -> Vec<(Span, Class)> {
    let (tokens, stop) = Lexer::tokenize_prefix(src);
    let mut out: Vec<(Span, Class)> = tokens
        .iter()
        .filter_map(|t| class(t.kind, &src[t.span.start..t.span.end]).map(|c| (t.span, c)))
        .collect();
    if src[stop..].starts_with('"') {
        out.push((Span { start: stop, end: src.len() }, Class::String));
    }
    out
}

/// `src` with ANSI color escapes around its tokens. Printed width is unchanged.
pub fn ansi(src: &str) -> String {
    let mut out = String::with_capacity(src.len() * 2);
    let mut at = 0;
    for (span, class) in spans(src) {
        let Some(sgr) = class.sgr() else { continue };
        out.push_str(&src[at..span.start]);
        // Per line, so a multi-line token never leaves color on behind a
        // cleared line.
        for (i, piece) in src[span.start..span.end].split('\n').enumerate() {
            if i > 0 {
                out.push('\n');
            }
            if !piece.is_empty() {
                out.push_str(&format!("\x1b[{sgr}m{piece}\x1b[0m"));
            }
        }
        at = span.end;
    }
    out.push_str(&src[at..]);
    out
}

#[cfg(test)]
#[path = "highlight_tests.rs"]
mod tests;
