//! `thrax fmt`: the canonical layout of a Thrax source.
//!
//! The formatter works on tokens, not on the AST, because the AST already
//! desugars surface forms (string interpolation, `&&`, `<|>`) that a formatter
//! must print as written. Tokens are printed verbatim and in order; only the
//! whitespace between them is chosen. The structure that decides where lines
//! break (items, bracket groups, `let`/`if`/`is`/`\`/`do` forms, arms) is read
//! off the token stream with a small recursive descent that mirrors the
//! grammar's layout-relevant shape, and laid out with a Wadler-style printer:
//! a group is printed on one line when it fits in the width, and broken at its
//! line points otherwise.
//!
//! Two rules keep it honest. A bracket group or declaration body that ends in a
//! trailing comma is always broken one entry per line (the comma says the
//! author wants it exploded). And the output is re-lexed before it is returned.
//! See Note [Formatter re-lex check].
//!
//! The printer is `wadler`; the rules that map syntax onto it are `layout`.
//!
//! Input: source text. Output: the formatted text, or the reason it was left
//! alone.
//!
//! Entry points: [`format_source`], [`format_edits`].

mod layout;
mod wadler;

use layout::format_item;

use crate::lexer::data::Kind;
use crate::lexer::Lexer;
use utilities::{Edit, Span};

/// How the output is laid out.
#[derive(Clone, Copy, Debug)]
pub struct Style {
    /// The line width groups are fitted into.
    pub width: usize,
    /// Spaces per indentation level.
    pub indent: usize,
}

impl Default for Style {
    fn default() -> Style {
        Style { width: 100, indent: 4 }
    }
}

/// The formatted source, or why it could not be formatted.
pub fn format_source(src: &str, style: &Style) -> Result<String, String> {
    let (out, _) = format_items(src, style)?;
    Ok(out)
}

/// The edits that format `src`. With a `window` (byte range), only the items it
/// touches are formatted. Empty when the source is already formatted.
pub fn format_edits(src: &str, style: &Style, window: Option<(usize, usize)>) -> Result<Vec<Edit>, String> {
    let (out, items) = format_items(src, style)?;
    match window {
        None => Ok(minimal_edit(src, &out).into_iter().collect()),
        Some((lo, hi)) => Ok(items
            .into_iter()
            .filter(|(span, _)| span.start <= hi && lo <= span.end)
            .filter(|(span, text)| span.slice(src) != text)
            .map(|(span, text)| Edit::new(span, text))
            .collect()),
    }
}

/// One edit replacing only the stretch where `old` and `new` differ.
fn minimal_edit(old: &str, new: &str) -> Option<Edit> {
    if old == new {
        return None;
    }
    let prefix = old.bytes().zip(new.bytes()).take_while(|(a, b)| a == b).count();
    let mut prefix = prefix;
    while !old.is_char_boundary(prefix) || !new.is_char_boundary(prefix) {
        prefix -= 1;
    }
    let max_suffix = (old.len() - prefix).min(new.len() - prefix);
    let mut suffix = old.bytes().rev().zip(new.bytes().rev()).take(max_suffix).take_while(|(a, b)| a == b).count();
    while !old.is_char_boundary(old.len() - suffix) || !new.is_char_boundary(new.len() - suffix) {
        suffix -= 1;
    }
    Some(Edit::new(Span::new(prefix, old.len() - suffix), &new[prefix..new.len() - suffix]))
}

#[derive(Clone, Copy)]
struct Tok<'s> {
    kind: Kind,
    span: Span,
    text: &'s str,
    /// Whitespace separated this token from the previous one.
    gap: bool,
    /// A newline separated this token from the previous one.
    newline: bool,
    /// More than one newline (a blank line) separated it.
    blank: bool,
}

fn lex(src: &str) -> Result<Vec<Tok<'_>>, String> {
    let raw = Lexer::tokenize_with_comments(src).map_err(|d| d.root().msg.clone())?;
    let mut out = Vec::with_capacity(raw.len());
    let mut prev_end = 0;
    for t in raw {
        if matches!(t.kind, Kind::Eof) {
            break;
        }
        let between = &src[prev_end..t.span.start];
        let newlines = between.matches('\n').count();
        out.push(Tok {
            kind: t.kind,
            span: t.span,
            text: &src[t.span.start..t.span.end],
            gap: !between.is_empty(),
            newline: newlines > 0 || prev_end == 0,
            blank: newlines > 1,
        });
        prev_end = t.span.end;
    }
    Ok(out)
}

/// Format every item, returning the whole output and, per item, its source
/// extent with its formatted text.
fn format_items(src: &str, style: &Style) -> Result<(String, Vec<(Span, String)>), String> {
    let toks = lex(src)?;
    // An item runs from its `$` (with the comments on their own lines before it)
    // to the token before the next item's.
    let mut starts: Vec<usize> = Vec::new();
    for (i, t) in toks.iter().enumerate() {
        if matches!(t.kind, Kind::Dollar) && t.newline {
            let mut s = i;
            while s > 0
                && matches!(toks[s - 1].kind, Kind::Comment)
                && toks[s - 1].newline
                && starts.last().is_none_or(|&l| s - 1 > l)
            {
                s -= 1;
            }
            starts.push(s);
        }
    }
    if starts.first() != Some(&0) {
        starts.insert(0, 0);
    }
    let mut out = String::new();
    // Per item: its source extent and the output lines it covers.
    let mut items: Vec<(Span, usize, usize)> = Vec::new();
    for (n, &s) in starts.iter().enumerate() {
        let e = starts.get(n + 1).copied().unwrap_or(toks.len());
        if s == e {
            continue;
        }
        let slice = &toks[s..e];
        let text = format_item(slice, style);
        if n > 0 {
            out.push_str(if slice[0].blank { "\n\n" } else { "\n" });
        }
        let first_line = out.matches('\n').count();
        out.push_str(&text);
        let span = Span::new(slice[0].span.start, slice[slice.len() - 1].span.end);
        items.push((span, first_line, out.matches('\n').count()));
    }
    if !out.is_empty() {
        out.push('\n');
    }
    let out = align_comments(&out, style.width);
    verify(src, &out)?;
    let lines: Vec<&str> = out.split('\n').collect();
    let items = items.into_iter().map(|(span, a, b)| (span, lines[a..=b].join("\n"))).collect();
    Ok((out, items))
}

/// Line up the trailing comments of consecutive lines at one indentation one
/// space past the longest code among them, unless that pushes a line past the
/// width.
fn align_comments(out: &str, width: usize) -> String {
    let Ok(toks) = lex(out) else { return out.to_string() };
    let mut lines: Vec<String> = out.split('\n').map(str::to_string).collect();
    let mut starts = vec![0];
    starts.extend(out.match_indices('\n').map(|(i, _)| i + 1));
    // Per line, where its trailing line comment starts (a column in bytes).
    let mut trailing: Vec<Option<usize>> = vec![None; lines.len()];
    for t in &toks {
        if matches!(t.kind, Kind::Comment) && !t.newline && !t.text.starts_with("#-") {
            let line = starts.partition_point(|&s| s <= t.span.start) - 1;
            trailing[line] = Some(t.span.start - starts[line]);
        }
    }
    let mut i = 0;
    while i < lines.len() {
        let indent = |l: &str| l.len() - l.trim_start().len();
        let mut j = i;
        while j < lines.len() && trailing[j].is_some() && indent(&lines[j]) == indent(&lines[i]) {
            j += 1;
        }
        if j - i > 1 {
            let code_width = |l: &str, at: usize| l[..at].trim_end().chars().count();
            let column = (i..j).map(|k| code_width(&lines[k], trailing[k].expect("in run"))).max().expect("non-empty") + 1;
            let fits = (i..j).all(|k| column + lines[k][trailing[k].expect("in run")..].chars().count() <= width);
            if fits {
                for k in i..j {
                    let at = trailing[k].expect("in run");
                    let code = lines[k][..at].trim_end().to_string();
                    let pad = column - code.chars().count();
                    lines[k] = format!("{code}{}{}", " ".repeat(pad), &lines[k][at..]);
                }
            }
        }
        i = j.max(i + 1);
    }
    lines.join("\n")
}

// Note [Formatter re-lex check]
//
// The formatter only chooses whitespace, but whitespace is not always
// meaningless: two tokens printed together can lex as a third, a comment
// moved onto a code line can swallow it, and a layout rule can be simply
// wrong. A formatter that changes what a program means is worse than none,
// and its bugs would surface far from their cause.
//
// So every result is checked before it is returned. The output is lexed
// again and must give the very same tokens (comments compared without
// trailing space), and when the input parsed, the output must parse to the
// same number of items. If either fails, the file is left as it was, with an
// error naming where. A layout bug can make the formatter decline a file; it
// cannot make it change one.

/// Refuse output that is not the input's token sequence, or that stops parsing.
/// See Note [Formatter re-lex check].
fn verify(src: &str, out: &str) -> Result<(), String> {
    let a = lex(src)?;
    let b = lex(out).map_err(|m| format!("the formatted text does not lex: {m}"))?;
    let same = a.len() == b.len()
        && a.iter().zip(&b).all(|(x, y)| {
            x.kind == y.kind
                && if matches!(x.kind, Kind::Comment) {
                    x.text.trim_end() == y.text.trim_end()
                } else {
                    x.text == y.text
                }
        });
    if !same {
        let at = a.iter().zip(&b).position(|(x, y)| x.kind != y.kind || x.text != y.text).unwrap_or(a.len().min(b.len()));
        let near = a.get(at).map_or("the end", |t| t.text);
        let line = a.get(at).map_or(0, |t| src[..t.span.start].matches('\n').count() + 1);
        return Err(format!("formatting would change the program's tokens (near `{near}`, line {line}); left as is"));
    }
    if let Ok(before) = crate::parse(src) {
        match crate::parse(out) {
            Ok(after) if after.program.items.len() == before.program.items.len() => {}
            _ => return Err("formatting would change how the program parses; left as is".to_string()),
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests;
