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
//! author wants it exploded). And the output is re-lexed: unless it yields the
//! very same tokens and comments, and parses whenever the input did, the
//! formatter refuses rather than change what the program means.

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
    let mut items = Vec::new();
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
        out.push_str(&text);
        let span = Span::new(slice[0].span.start, slice[slice.len() - 1].span.end);
        items.push((span, text));
    }
    if !out.is_empty() {
        out.push('\n');
    }
    verify(src, &out)?;
    Ok((out, items))
}

/// Refuse output that is not the input's token sequence, or that stops parsing.
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

// -- documents ---------------------------------------------------------------

type D = usize;

enum Node {
    Text(String),
    /// A space when flat, a newline when broken.
    Line,
    /// Nothing when flat, a newline when broken.
    Soft,
    /// Always a newline (after a line comment).
    Hard,
    /// A newline unless the line is still empty (before an own-line comment,
    /// which may follow a line break already).
    Fresh,
    Nest(D),
    /// Fits on a line or breaks all its lines; `true` always breaks.
    Group(D, bool),
    Cat(Vec<D>),
    /// Always on one line, however long: a declaration's head.
    Flat(D),
}

/// The document arena for one item: nodes refer to each other by index, and
/// the whole thing is dropped when the item is printed.
#[derive(Default)]
struct Docs {
    nodes: Vec<Node>,
}

impl Docs {
    fn add(&mut self, n: Node) -> D {
        self.nodes.push(n);
        self.nodes.len() - 1
    }
    fn text(&mut self, s: &str) -> D {
        self.add(Node::Text(s.to_string()))
    }
    fn cat(&mut self, ds: Vec<D>) -> D {
        self.add(Node::Cat(ds))
    }
    fn nest(&mut self, d: D) -> D {
        self.add(Node::Nest(d))
    }
    fn group(&mut self, d: D, force: bool) -> D {
        self.add(Node::Group(d, force))
    }
}

#[derive(Clone, Copy, PartialEq)]
enum Mode {
    Flat,
    Break,
}

fn print(docs: &Docs, root: D, style: &Style) -> String {
    let mut out = String::new();
    let mut col = 0usize;
    let mut stack: Vec<(usize, Mode, D)> = vec![(0, Mode::Break, root)];
    while let Some((ind, mode, d)) = stack.pop() {
        match &docs.nodes[d] {
            Node::Text(s) => {
                out.push_str(s);
                col = match s.rfind('\n') {
                    Some(i) => s[i + 1..].chars().count(),
                    None => col + s.chars().count(),
                };
            }
            Node::Fresh => {
                let line_start = out.rfind('\n').map_or(0, |i| i + 1);
                if !out[line_start..].trim().is_empty() {
                    while out.ends_with(' ') {
                        out.pop();
                    }
                    out.push('\n');
                    out.extend(std::iter::repeat_n(' ', ind));
                    col = ind;
                }
            }
            Node::Line | Node::Soft | Node::Hard => {
                let flat = mode == Mode::Flat && !matches!(docs.nodes[d], Node::Hard);
                if flat {
                    if matches!(docs.nodes[d], Node::Line) {
                        out.push(' ');
                        col += 1;
                    }
                } else {
                    while out.ends_with(' ') {
                        out.pop();
                    }
                    // A break right after a line comment's newline only sets the
                    // indentation: the comment already ended the line.
                    let fresh = out.ends_with('\n') && !matches!(docs.nodes[d], Node::Hard);
                    if !fresh {
                        out.push('\n');
                    }
                    out.extend(std::iter::repeat_n(' ', ind));
                    col = ind;
                }
            }
            Node::Nest(x) => stack.push((ind + style.indent, mode, *x)),
            Node::Flat(x) => stack.push((ind, Mode::Flat, *x)),
            Node::Cat(xs) => {
                for x in xs.iter().rev() {
                    stack.push((ind, mode, *x));
                }
            }
            Node::Group(x, force) => {
                let m = if !*force && mode == Mode::Flat {
                    Mode::Flat
                } else if !*force && fits(docs, style.width.saturating_sub(col), (ind, Mode::Flat, *x), &stack) {
                    Mode::Flat
                } else {
                    Mode::Break
                };
                stack.push((ind, m, *x));
            }
        }
    }
    while out.ends_with(' ') {
        out.pop();
    }
    out
}

/// Whether `next` printed flat, followed by the rest of the line, fits in `room`.
fn fits(docs: &Docs, room: usize, next: (usize, Mode, D), rest: &[(usize, Mode, D)]) -> bool {
    let mut room = room as isize;
    let mut work: Vec<(usize, Mode, D)> = vec![next];
    let mut rest = rest.iter().rev();
    loop {
        let Some((ind, mode, d)) = work.pop().or_else(|| rest.next().copied()) else {
            return true;
        };
        if room < 0 {
            return false;
        }
        match &docs.nodes[d] {
            Node::Text(s) => {
                if s.contains('\n') {
                    return mode == Mode::Break;
                }
                room -= s.chars().count() as isize;
            }
            // What precedes a line comment's newline fit; an own-line comment
            // never fits a flat group.
            Node::Hard => return true,
            Node::Fresh => return mode == Mode::Break,
            Node::Line | Node::Soft => match mode {
                Mode::Break => return true,
                Mode::Flat => room -= isize::from(matches!(docs.nodes[d], Node::Line)),
            },
            Node::Nest(x) => work.push((ind, mode, *x)),
            Node::Flat(x) => work.push((ind, Mode::Flat, *x)),
            Node::Cat(xs) => {
                for x in xs.iter().rev() {
                    work.push((ind, mode, *x));
                }
            }
            Node::Group(x, force) => {
                if *force {
                    return false;
                }
                work.push((ind, Mode::Flat, *x));
            }
        }
        if room < 0 {
            return false;
        }
    }
}

// -- token trees -------------------------------------------------------------

enum Tree {
    Tok(usize),
    Group { open: usize, kids: Vec<Tree>, close: Option<usize> },
}

fn trees(toks: &[Tok]) -> Vec<Tree> {
    let mut stack: Vec<(usize, Vec<Tree>)> = vec![(usize::MAX, Vec::new())];
    for (i, t) in toks.iter().enumerate() {
        match t.kind {
            Kind::LParen | Kind::LBrack | Kind::LBrace => stack.push((i, Vec::new())),
            Kind::RParen | Kind::RBrack | Kind::RBrace if stack.len() > 1 => {
                let (open, kids) = stack.pop().expect("checked");
                stack.last_mut().expect("root").1.push(Tree::Group { open, kids, close: Some(i) });
            }
            _ => stack.last_mut().expect("root").1.push(Tree::Tok(i)),
        }
    }
    while stack.len() > 1 {
        let (open, kids) = stack.pop().expect("checked");
        stack.last_mut().expect("root").1.push(Tree::Group { open, kids, close: None });
    }
    stack.pop().expect("root").1
}

// -- layout ------------------------------------------------------------------

/// A laid-out piece and its first and last tokens, which decide the spacing
/// when it is joined to a neighbour.
#[derive(Clone, Copy)]
struct P {
    d: D,
    first: usize,
    last: usize,
}

struct Layout<'t, 's> {
    toks: &'t [Tok<'s>],
    docs: Docs,
}

fn format_item(toks: &[Tok], style: &Style) -> String {
    let ts = trees(toks);
    let mut l = Layout { toks, docs: Docs::default() };
    let root = l.item(&ts);
    print(&l.docs, root, style).trim_end_matches('\n').to_string()
}

/// Operators a long expression breaks before, the continuation indented:
/// `"a"\n    ++ "b"`, `xs\n    |> f`.
const BREAK_BEFORE: &[&str] = &["|>", "++", "&&", "||"];

/// A match with more arms than this always puts each arm on its own line.
const MAX_FLAT_ARMS: usize = 2;

/// The keywords and marks that start a layout form.
fn starts_form(t: &Tok) -> bool {
    matches!(t.kind, Kind::Let | Kind::If | Kind::Is | Kind::Lambda | Kind::Defer | Kind::Do)
}

fn is_bar(t: &Tok) -> bool {
    matches!(t.kind, Kind::Op) && t.text == "|"
}

impl Layout<'_, '_> {
    fn tok_of(&self, tree: &Tree) -> Option<&Tok<'_>> {
        match tree {
            Tree::Tok(i) => Some(&self.toks[*i]),
            Tree::Group { .. } => None,
        }
    }

    fn first_tok(tree: &Tree) -> usize {
        match tree {
            Tree::Tok(i) => *i,
            Tree::Group { open, .. } => *open,
        }
    }

    /// The tree at `pos`, skipping comments, and its index.
    fn peek<'a>(&self, ts: &'a [Tree], pos: usize) -> Option<(usize, &'a Tree)> {
        (pos..ts.len()).map(|i| (i, &ts[i])).find(|(_, t)| !matches!(self.tok_of(t).map(|t| t.kind), Some(Kind::Comment)))
    }

    fn peek_tok(&self, ts: &[Tree], pos: usize) -> Option<Tok<'_>> {
        self.peek(ts, pos).and_then(|(_, t)| self.tok_of(t).copied())
    }

    fn unary(&self, i: usize) -> bool {
        let t = &self.toks[i];
        let next_glued = self.toks.get(i + 1).is_some_and(|n| !n.gap);
        matches!(t.kind, Kind::Op) && matches!(t.text, "-" | "!") && next_glued
            && (i == 0
                || t.gap
                || matches!(
                    self.toks[i - 1].kind,
                    Kind::Op | Kind::LParen | Kind::LBrack | Kind::LBrace | Kind::Comma | Kind::Eq | Kind::FatArrow | Kind::Arrow
                ))
    }

    /// Whether a space goes between tokens `a` and `b` (indices) on one line.
    fn space(&self, a: usize, b: usize) -> bool {
        let (x, y) = (&self.toks[a], &self.toks[b]);
        use Kind::*;
        match (x.kind, y.kind) {
            (LParen | LBrack, _) => false,
            (_, RParen | RBrack | Comma) => false,
            (_, Op) if y.text == ";" => false,
            (LBrace, RBrace) => false,
            (LBrace, _) | (_, RBrace) => true,
            (Dot, _) => false,
            (_, Dot) => y.gap,
            (Lambda, _) => false,
            (_, Colon) => y.gap,
            (Op, _) if self.unary(a) => false,
            (Op, _) | (_, Op) if matches!(x.text, "<" | ">" | "<>") || matches!(y.text, "<" | ">" | "<>") => y.gap,
            (At, LParen | LBrack) => y.gap,
            _ => true,
        }
    }

    /// Whether the source broke a line anywhere between tokens `a` and `b`.
    fn multiline(&self, a: usize, b: usize) -> bool {
        (a + 1..=b.min(self.toks.len().saturating_sub(1))).any(|i| self.toks[i].newline)
    }

    /// Join pieces left to right, a space or nothing between each pair.
    fn join(&mut self, ps: &[P]) -> Option<P> {
        let first = ps.first()?;
        let mut ds = vec![first.d];
        for w in ps.windows(2) {
            if self.space(w[0].last, w[1].first) {
                ds.push(self.docs.text(" "));
            }
            ds.push(w[1].d);
        }
        let d = self.docs.cat(ds);
        Some(P { d, first: first.first, last: ps.last().expect("non-empty").last })
    }

    /// `a`, then a line point, then `b` (a space when the line is not broken).
    fn line_join(&mut self, a: P, b: P, kind: Node) -> P {
        let line = self.docs.add(kind);
        let d = self.docs.cat(vec![a.d, line, b.d]);
        P { d, first: a.first, last: b.last }
    }

    fn tok(&mut self, i: usize) -> P {
        let t = self.toks[i];
        let text = if matches!(t.kind, Kind::Comment) { t.text.trim_end() } else { t.text };
        let d = self.docs.text(text);
        P { d, first: i, last: i }
    }

    /// A comment, on its own line if it was written on its own line; a line
    /// comment always ends its line.
    fn comment(&mut self, i: usize) -> P {
        let t = self.toks[i];
        let body = self.tok(i);
        let mut ds = Vec::new();
        if t.newline {
            ds.push(self.docs.add(Node::Fresh));
        }
        ds.push(body.d);
        if !t.text.starts_with("#-") || t.text.contains('\n') {
            ds.push(self.docs.add(Node::Hard));
        }
        let d = self.docs.cat(ds);
        P { d, first: i, last: i }
    }

    fn is_comment(&self, tree: &Tree) -> bool {
        matches!(self.tok_of(tree).map(|t| t.kind), Some(Kind::Comment))
    }

    /// Comments at `pos`, consumed and laid out.
    fn comments(&mut self, ts: &[Tree], pos: &mut usize) -> Vec<P> {
        let mut out = Vec::new();
        while *pos < ts.len() && self.is_comment(&ts[*pos]) {
            out.push(self.comment(Self::first_tok(&ts[*pos])));
            *pos += 1;
        }
        out
    }

    /// Prepend laid-out comments to a piece.
    fn with_comments(&mut self, cs: Vec<P>, p: P) -> P {
        if cs.is_empty() {
            return p;
        }
        let mut ds: Vec<D> = cs.iter().map(|c| c.d).collect();
        ds.push(p.d);
        let d = self.docs.cat(ds);
        P { d, first: cs[0].first, last: p.last }
    }

    fn tree(&mut self, tree: &Tree) -> P {
        match tree {
            Tree::Tok(i) => {
                if matches!(self.toks[*i].kind, Kind::Comment) {
                    self.comment(*i)
                } else {
                    self.tok(*i)
                }
            }
            Tree::Group { open, kids, close } => self.bracket(*open, kids, *close),
        }
    }

    fn bracket(&mut self, open: usize, kids: &[Tree], close: Option<usize>) -> P {
        let o = self.tok(open);
        let Some(c) = close else {
            let mut ps = vec![o];
            ps.extend(kids.iter().map(|k| self.tree(k)));
            return self.join(&ps).expect("non-empty");
        };
        let c_p = self.tok(c);
        if kids.is_empty() {
            return self.join(&[o, c_p]).expect("non-empty");
        }
        let (elems, trailing, tail) = self.comma_list(kids);
        let braces = matches!(self.toks[open].kind, Kind::LBrace);
        let mut inner = elems[0];
        for &e in &elems[1..] {
            let comma = self.docs.text(",");
            let d = self.docs.cat(vec![inner.d, comma]);
            inner = self.line_join(P { d, ..inner }, e, Node::Line);
        }
        let force = (trailing && self.multiline(open, c)) || !tail.is_empty();
        inner = self.close_list(inner, trailing, tail);
        let edge = if braces { Node::Line } else { Node::Soft };
        let lead = self.docs.add(if braces { Node::Line } else { Node::Soft });
        let body = self.docs.cat(vec![lead, inner.d]);
        let nested = self.docs.nest(body);
        let tail = self.docs.add(edge);
        let d = self.docs.cat(vec![o.d, nested, tail, c_p.d]);
        let g = self.docs.group(d, force);
        P { d: g, first: open, last: c }
    }

    /// Split a run at its top-level commas, each element laid out as an
    /// expression. Reports a trailing comma.
    fn comma_list(&mut self, ts: &[Tree]) -> (Vec<P>, bool, Vec<P>) {
        let mut elems = Vec::new();
        let mut tail = Vec::new();
        let mut pos = 0;
        let mut trailing = false;
        loop {
            let cs = self.comments(ts, &mut pos);
            if pos >= ts.len() {
                if elems.is_empty() && !cs.is_empty() {
                    elems.push(self.join(&cs).expect("non-empty"));
                } else {
                    tail = cs;
                }
                break;
            }
            let before = pos;
            let mut e = self.expr(ts, &mut pos, &|_| false, true);
            if pos == before && pos < ts.len() && !matches!(self.tok_of(&ts[pos]).map(|t| t.kind), Some(Kind::Comma)) {
                // A token no form accepts here (malformed input): keep it verbatim.
                let stray = self.tree(&ts[pos]);
                pos += 1;
                e = stray;
            }
            let e = self.with_comments(cs, e);
            elems.push(e);
            trailing = false;
            if let Some((i, t)) = self.peek(ts, pos) {
                if matches!(self.tok_of(t).map(|t| t.kind), Some(Kind::Comma)) {
                    let between = self.comments(ts, &mut pos);
                    let last = elems.pop().expect("just pushed");
                    let last = if between.is_empty() {
                        last
                    } else {
                        let mut ds = vec![last.d];
                        ds.extend(between.iter().map(|p| p.d));
                        P { d: self.docs.cat(ds), ..last }
                    };
                    elems.push(last);
                    pos = i + 1;
                    trailing = true;
                    continue;
                }
            }
            if pos >= ts.len() {
                break;
            }
        }
        if elems.is_empty() {
            let d = self.docs.text("");
            elems.push(P { d, first: 0, last: 0 });
        }
        (elems, trailing, tail)
    }

    /// The last element with the separators and comments that close a list:
    /// its trailing comma first, then any comments after it.
    fn close_list(&mut self, last: P, trailing: bool, tail: Vec<P>) -> P {
        let mut ps = vec![last];
        if trailing {
            let comma = self.docs.text(",");
            ps[0] = P { d: self.docs.cat(vec![last.d, comma]), ..last };
        }
        if tail.is_empty() {
            return ps[0];
        }
        ps.extend(tail);
        self.join(&ps).expect("non-empty")
    }

    /// An expression starting at `pos`, up to a token `stop` accepts (at this
    /// bracket level) or the end. `commas` stops at a comma too.
    fn expr(&mut self, ts: &[Tree], pos: &mut usize, stop: &dyn Fn(&Tok) -> bool, commas: bool) -> P {
        let halt = |t: &Tok| stop(t) || (commas && matches!(t.kind, Kind::Comma));
        let mut run: Vec<P> = Vec::new();
        // Break points in the run: after `;` (same level) and before `|>` (nested).
        let mut breaks: Vec<(usize, bool)> = Vec::new();
        while *pos < ts.len() {
            let tree = &ts[*pos];
            if self.is_comment(tree) {
                let own_line = self.toks[Self::first_tok(tree)].newline;
                match self.peek_tok(ts, *pos) {
                    Some(t) if halt(&t) && own_line => break,
                    None if own_line => break,
                    _ => {}
                }
                run.push(self.tree(tree));
                *pos += 1;
                continue;
            }
            if let Some(t) = self.tok_of(tree).copied() {
                if halt(&t) {
                    break;
                }
                if starts_form(&t) || (matches!(t.kind, Kind::With) && self.has_in(ts, *pos, &halt)) {
                    let form = self.form(ts, pos, &halt);
                    run.push(form);
                    break;
                }
                if matches!(t.kind, Kind::Op) && BREAK_BEFORE.contains(&t.text) && !run.is_empty() && !self.unary(Self::first_tok(tree)) {
                    breaks.push((run.len(), true));
                }
            }
            run.push(self.tree(tree));
            *pos += 1;
            if let Some(t) = self.tok_of(tree) {
                if matches!(t.kind, Kind::Op) && t.text == ";" {
                    breaks.push((run.len(), false));
                }
            }
        }
        if run.is_empty() {
            let d = self.docs.text("");
            let at = ts.get(*pos).map_or(0, Self::first_tok);
            return P { d, first: at, last: at };
        }
        // A lone operator, as in `$ (;) : ...`, has nothing on one side to break from.
        breaks.retain(|&(at, _)| 0 < at && at < run.len());
        if breaks.is_empty() {
            return self.join(&run).expect("non-empty");
        }
        let mut segments = Vec::new();
        let mut from = 0;
        for &(at, _) in &breaks {
            segments.push(self.join(&run[from..at]).expect("non-empty"));
            from = at;
        }
        segments.push(self.join(&run[from..]).expect("non-empty"));
        let mut acc = segments[0];
        let mut nested_tail: Vec<D> = Vec::new();
        for (k, seg) in segments[1..].iter().enumerate() {
            let (_, pipe) = breaks[k];
            if pipe {
                let line = self.docs.add(Node::Line);
                nested_tail.push(line);
                nested_tail.push(seg.d);
            } else {
                if !nested_tail.is_empty() {
                    let tail = self.docs.cat(std::mem::take(&mut nested_tail));
                    let n = self.docs.nest(tail);
                    acc.d = self.docs.cat(vec![acc.d, n]);
                }
                acc = self.line_join(acc, *seg, Node::Line);
            }
            acc.last = seg.last;
        }
        if !nested_tail.is_empty() {
            let tail = self.docs.cat(nested_tail);
            let n = self.docs.nest(tail);
            acc.d = self.docs.cat(vec![acc.d, n]);
        }
        let g = self.docs.group(acc.d, false);
        P { d: g, ..acc }
    }

    /// Whether an `in` follows at this level before the expression ends.
    fn has_in(&self, ts: &[Tree], pos: usize, halt: &dyn Fn(&Tok) -> bool) -> bool {
        ts[pos + 1..]
            .iter()
            .filter_map(|t| self.tok_of(t))
            .take_while(|t| !halt(t) || matches!(t.kind, Kind::Comma))
            .any(|t| matches!(t.kind, Kind::In))
    }

    fn expect(&mut self, ts: &[Tree], pos: &mut usize, want: impl Fn(&Tok) -> bool) -> Option<P> {
        let (i, t) = self.peek(ts, *pos)?;
        let tok = self.tok_of(t).copied()?;
        if !want(&tok) {
            return None;
        }
        let cs = self.comments(ts, pos);
        *pos = i + 1;
        let p = self.tok(Self::first_tok(t));
        Some(self.with_comments(cs, p))
    }

    /// A `let`/`with`/`defer`/`if`/`is`/`\`/`do` form, which runs to the end of
    /// the enclosing expression.
    fn form(&mut self, ts: &[Tree], pos: &mut usize, halt: &dyn Fn(&Tok) -> bool) -> P {
        let kw = self.tok_of(&ts[*pos]).copied().expect("a form starts with a token");
        match kw.kind {
            Kind::Lambda => {
                let head = self.lambda_head(ts, pos, halt);
                let body = self.expr(ts, pos, halt, false);
                self.hang(head, body)
            }
            Kind::Let | Kind::With | Kind::Defer => self.binding_form(ts, pos, halt),
            Kind::If => self.if_form(ts, pos, halt),
            Kind::Is => {
                let k = self.tok(Self::first_tok(&ts[*pos]));
                *pos += 1;
                let bar_or = |t: &Tok| halt(t) || is_bar(t);
                let scrut = self.expr(ts, pos, &bar_or, false);
                let head = self.join(&[k, scrut]).expect("non-empty");
                self.arms(head, ts, pos, halt)
            }
            Kind::Do => {
                let k = self.tok(Self::first_tok(&ts[*pos]));
                *pos += 1;
                let ctl_or = |t: &Tok| halt(t) || matches!(t.kind, Kind::Ctl);
                let body = self.expr(ts, pos, &ctl_or, false);
                let head = self.join(&[k, body]).expect("non-empty");
                let Some(ctl) = self.expect(ts, pos, |t| matches!(t.kind, Kind::Ctl)) else {
                    return head;
                };
                let bar_or = |t: &Tok| halt(t) || is_bar(t);
                let name = self.expr(ts, pos, &bar_or, false);
                let ctl_head = self.join(&[ctl, name]).expect("non-empty");
                let handled = self.arms(ctl_head, ts, pos, halt);
                let joined = self.line_join(head, handled, Node::Line);
                P { d: self.docs.group(joined.d, true), ..joined }
            }
            _ => unreachable!("not a form keyword"),
        }
    }

    /// `head` then `body`, the body moved to its own indented line when the
    /// two do not fit on one.
    fn hang(&mut self, head: P, body: P) -> P {
        let line = self.docs.add(Node::Line);
        let inner = self.docs.cat(vec![line, body.d]);
        let nested = self.docs.nest(inner);
        let d = self.docs.cat(vec![head.d, nested]);
        let g = self.docs.group(d, false);
        P { d: g, first: head.first, last: body.last }
    }

    /// `\p q =`, as one piece.
    fn lambda_head(&mut self, ts: &[Tree], pos: &mut usize, halt: &dyn Fn(&Tok) -> bool) -> P {
        let mut ps = Vec::new();
        while *pos < ts.len() {
            let tree = &ts[*pos];
            let done = matches!(self.tok_of(tree).map(|t| t.kind), Some(Kind::Eq));
            if !done && self.tok_of(tree).is_some_and(|t| halt(t)) {
                break;
            }
            ps.push(self.tree(tree));
            *pos += 1;
            if done {
                break;
            }
        }
        self.join(&ps).expect("a lambda has its backslash")
    }

    /// `let a = x, b = y in body` (and `with`, `defer`): bindings on one line,
    /// or each on its own, then `in` and the body.
    fn binding_form(&mut self, ts: &[Tree], pos: &mut usize, halt: &dyn Fn(&Tok) -> bool) -> P {
        let kw = self.tok(Self::first_tok(&ts[*pos]));
        *pos += 1;
        let sep = |t: &Tok| halt(t) || matches!(t.kind, Kind::Comma | Kind::In);
        let mut binds: Vec<P> = Vec::new();
        let mut trailing = false;
        loop {
            let cs = self.comments(ts, pos);
            let b = self.expr(ts, pos, &sep, false);
            let b = self.with_comments(cs, b);
            binds.push(b);
            match self.expect(ts, pos, |t| matches!(t.kind, Kind::Comma)) {
                Some(_) => {
                    trailing = true;
                    if matches!(self.peek_tok(ts, *pos).map(|t| t.kind), Some(Kind::In)) {
                        break;
                    }
                    trailing = false;
                }
                None => break,
            }
        }
        let space = self.docs.text(" ");
        let mut rest: Vec<D> = vec![binds[0].d];
        for k in 0..binds.len() {
            if k + 1 < binds.len() || trailing {
                rest.push(self.docs.text(","));
            }
            if k + 1 < binds.len() {
                rest.push(self.docs.add(Node::Line));
                rest.push(binds[k + 1].d);
            }
        }
        let tail = self.docs.cat(rest);
        let nested = self.docs.nest(tail);
        let d = self.docs.cat(vec![kw.d, space, nested]);
        let inner = P { d, first: kw.first, last: binds[binds.len() - 1].last };
        let Some(in_kw) = self.expect(ts, pos, |t| matches!(t.kind, Kind::In)) else {
            let g = self.docs.group(inner.d, trailing);
            return P { d: g, ..inner };
        };
        let space = self.docs.text(" ");
        let head = self.docs.cat(vec![inner.d, space, in_kw.d]);
        let head = P { d: self.docs.group(head, trailing), first: inner.first, last: in_kw.last };
        let body = self.expr(ts, pos, halt, false);
        let joined = self.line_join(head, body, Node::Line);
        let g = self.docs.group(joined.d, false);
        P { d: g, ..joined }
    }

    /// `if c => t, c2 => t2 else e`, with an `else if` chain kept flat: each
    /// `else if` starts a line of its own, at the level of the first `if`.
    fn if_form(&mut self, ts: &[Tree], pos: &mut usize, halt: &dyn Fn(&Tok) -> bool) -> P {
        let arrow_or = |t: &Tok| halt(t) || matches!(t.kind, Kind::FatArrow);
        let branch_end = |t: &Tok| halt(t) || matches!(t.kind, Kind::Comma | Kind::Else);
        let mut kw = self.tok(Self::first_tok(&ts[*pos]));
        *pos += 1;
        let mut lines: Vec<P> = Vec::new();
        loop {
            let mut first = true;
            loop {
                let cond = self.expr(ts, pos, &arrow_or, false);
                let mut parts = if first { vec![kw, cond] } else { vec![cond] };
                first = false;
                let arm = match self.expect(ts, pos, |t| matches!(t.kind, Kind::FatArrow)) {
                    Some(arrow) => {
                        parts.push(arrow);
                        let then = self.expr(ts, pos, &branch_end, false);
                        let head = self.join(&parts).expect("non-empty");
                        self.hang(head, then)
                    }
                    None => self.join(&parts).expect("non-empty"),
                };
                if self.expect(ts, pos, |t| matches!(t.kind, Kind::Comma)).is_none() {
                    lines.push(arm);
                    break;
                }
                let comma = self.docs.text(",");
                lines.push(P { d: self.docs.cat(vec![arm.d, comma]), ..arm });
            }
            let Some(else_kw) = self.expect(ts, pos, |t| matches!(t.kind, Kind::Else)) else { break };
            if matches!(self.peek_tok(ts, *pos).map(|t| t.kind), Some(Kind::If)) {
                let if_kw = self.expect(ts, pos, |t| matches!(t.kind, Kind::If)).expect("peeked");
                kw = self.join(&[else_kw, if_kw]).expect("non-empty");
                continue;
            }
            let e = self.expr(ts, pos, halt, false);
            lines.push(self.hang(else_kw, e));
            break;
        }
        let mut acc = lines[0];
        for &l in &lines[1..] {
            acc = self.line_join(acc, l, Node::Line);
        }
        let g = self.docs.group(acc.d, false);
        P { d: g, ..acc }
    }

    /// `head | p => e | q if g => f`: on one line, or the arms one per line
    /// under the head.
    fn arms(&mut self, head: P, ts: &[Tree], pos: &mut usize, halt: &dyn Fn(&Tok) -> bool) -> P {
        let mut acc = head;
        let mut count = 0;
        loop {
            let Some(bar) = self.expect(ts, pos, is_bar) else { break };
            let pat_end = |t: &Tok| halt(t) || is_bar(t) || matches!(t.kind, Kind::FatArrow | Kind::If);
            let mut parts = vec![bar];
            loop {
                parts.push(self.expr(ts, pos, &pat_end, false));
                match self.expect(ts, pos, is_bar) {
                    Some(alt) => parts.push(alt),
                    None => break,
                }
            }
            if let Some(kw) = self.expect(ts, pos, |t| matches!(t.kind, Kind::If)) {
                parts.push(kw);
                let arrow_or = |t: &Tok| halt(t) || matches!(t.kind, Kind::FatArrow);
                parts.push(self.expr(ts, pos, &arrow_or, false));
            }
            let arm = match self.expect(ts, pos, |t| matches!(t.kind, Kind::FatArrow)) {
                Some(arrow) => {
                    parts.push(arrow);
                    let head = self.join(&parts).expect("non-empty");
                    let bar_or = |t: &Tok| halt(t) || is_bar(t);
                    let body = self.expr(ts, pos, &bar_or, false);
                    self.hang(head, body)
                }
                None => self.join(&parts).expect("non-empty"),
            };
            acc = self.line_join(acc, arm, Node::Line);
            count += 1;
        }
        let g = self.docs.group(acc.d, count > MAX_FLAT_ARMS);
        P { d: g, ..acc }
    }

    /// One top-level item: its leading comments, then `$ head = body`.
    fn item(&mut self, ts: &[Tree]) -> D {
        let mut pos = 0;
        let mut lead: Vec<D> = Vec::new();
        while pos < ts.len() && self.is_comment(&ts[pos]) {
            let i = Self::first_tok(&ts[pos]);
            if !lead.is_empty() && self.toks[i].blank {
                lead.push(self.docs.add(Node::Hard));
            }
            lead.push(self.tok(i).d);
            lead.push(self.docs.add(Node::Hard));
            pos += 1;
        }
        if !lead.is_empty() && pos < ts.len() && self.toks[Self::first_tok(&ts[pos])].blank {
            lead.push(self.docs.add(Node::Hard));
        }
        let body = self.declaration(&ts[pos..]);
        lead.push(body);
        self.docs.cat(lead)
    }

    fn declaration(&mut self, ts: &[Tree]) -> D {
        if ts.is_empty() {
            return self.docs.text("");
        }
        let eq = ts.iter().position(|t| matches!(self.tok_of(t).map(|t| t.kind), Some(Kind::Eq)));
        let second = ts.get(1).and_then(|t| self.tok_of(t)).copied();
        let is_meta_e = second.is_some_and(|t| matches!(t.kind, Kind::At) && t.text == "@e");
        let Some(eq) = eq.filter(|_| !is_meta_e) else {
            // `$ @e expr`, `$ with M`, `$ @private`, `$ @mod M`.
            let split = if is_meta_e { 2 } else { ts.len() };
            let head_ps: Vec<P> = ts[..split].iter().map(|t| self.tree(t)).collect();
            let head = self.join(&head_ps).expect("non-empty");
            if split == ts.len() {
                return head.d;
            }
            let mut pos = split;
            let body = self.expr(ts, &mut pos, &|_| false, false);
            let rest = self.leftovers(ts, pos, body);
            return self.hang(head, rest).d;
        };
        let eq_tok = Self::first_tok(&ts[eq]);
        let mut head_ps: Vec<P> = ts[..=eq].iter().map(|t| self.tree(t)).collect();
        let mut pos = eq + 1;
        while matches!(self.peek_tok(ts, pos).map(|t| t.kind), Some(Kind::Lambda)) {
            let cs = self.comments(ts, &mut pos);
            head_ps.extend(cs);
            head_ps.push(self.lambda_head(ts, &mut pos, &|_| false));
        }
        let mut head_comment = false;
        while pos < ts.len() && self.is_comment(&ts[pos]) && !self.toks[Self::first_tok(&ts[pos])].newline {
            let i = Self::first_tok(&ts[pos]);
            head_ps.push(self.tok(i));
            head_comment = true;
            pos += 1;
        }
        let head = self.join(&head_ps).expect("non-empty");
        // A signature reads best whole; only the body moves to the next line.
        let head = P { d: self.docs.add(Node::Flat(head.d)), ..head };
        let (elems, trailing, tail) = self.comma_list(&ts[pos..]);
        let mut body = elems[0];
        for &e in &elems[1..] {
            let comma = self.docs.text(",");
            let d = self.docs.cat(vec![body.d, comma]);
            body = self.line_join(P { d, ..body }, e, Node::Line);
        }
        let exploded = trailing && self.multiline(eq_tok, self.toks.len() - 1);
        let body = self.close_list(body, trailing, tail);
        let line = self.docs.add(Node::Line);
        let inner = self.docs.cat(vec![line, body.d]);
        let nested = self.docs.nest(inner);
        let d = self.docs.cat(vec![head.d, nested]);
        self.docs.group(d, exploded || head_comment)
    }

    /// Anything an expression did not consume (malformed input), appended
    /// verbatim so no token is lost.
    fn leftovers(&mut self, ts: &[Tree], pos: usize, body: P) -> P {
        if pos >= ts.len() {
            return body;
        }
        let mut ps = vec![body];
        ps.extend(ts[pos..].iter().map(|t| self.tree(t)));
        self.join(&ps).expect("non-empty")
    }
}

#[cfg(test)]
#[path = "format_tests.rs"]
mod tests;
