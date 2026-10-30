//! The layout rules: how Thrax syntax maps onto a document.
//!
//! Tokens are first grouped into bracket trees ([`trees`]). [`Layout`] then
//! reads the shapes the formatter cares about off the token stream with a small
//! recursive descent (items, `let`/`if`/`is`/`\`/`do` forms, match arms,
//! operator chains) and emits a [`super::wadler`] document for each item:
//! where lines may break, what indents, which runs align.
//!
//! Input: one item's tokens. Output: its printed text.
//!
//! Entry point: [`format_item`].

use super::wadler::*;
use super::*;

pub(super) enum Tree {
    Tok(usize),
    Group { open: usize, kids: Vec<Tree>, close: Option<usize> },
}

pub(super) fn trees(toks: &[Tok]) -> Vec<Tree> {
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

// ## Layout

/// A laid-out piece and its first and last tokens, which decide the spacing
/// when it is joined to a neighbour.
#[derive(Clone, Copy)]
pub(super) struct P {
    d: D,
    first: usize,
    last: usize,
}

pub(super) struct Layout<'t, 's> {
    toks: &'t [Tok<'s>],
    docs: Docs,
    /// Per bracket-opening token, the token that closes it.
    closes: Vec<Option<usize>>,
    /// Per closing token, whether its group opens at the end of a line: a
    /// non-empty `{ }` or `[ ]`, or a parenthesized lambda.
    opens: Vec<bool>,
    width: usize,
}

pub(super) fn format_item(toks: &[Tok], style: &Style) -> String {
    let ts = trees(toks);
    let mut l = Layout { toks, docs: Docs::default(), closes: vec![None; toks.len()], opens: vec![false; toks.len()], width: style.width };
    let root = l.item(&ts);
    print(&l.docs, root, style).trim_end_matches('\n').to_string()
}

/// Operators a long expression breaks before, the continuation indented:
/// `"a"\n    ++ "b"`, `xs\n    |> f`.
pub(super) const BREAK_BEFORE: &[&str] = &["|>", "++", "&&", "||"];

/// Operators a long expression wraps before, as many operands to a line as fit.
pub(super) const FILL_BEFORE: &[&str] = &["+", "-", "*"];

/// A match with more arms than this always puts each arm on its own line.
pub(super) const MAX_FLAT_ARMS: usize = 2;

/// Arms and bindings line up their `=>` or `=`, except one whose left side is
/// wider than this: a long pattern would push every other arm far right.
pub(super) const MAX_ALIGNED: usize = 40;

/// The keywords and marks that start a layout form.
pub(super) fn starts_form(t: &Tok) -> bool {
    matches!(t.kind, Kind::Let | Kind::If | Kind::Is | Kind::Lambda | Kind::Defer | Kind::Do)
}

pub(super) fn is_bar(t: &Tok) -> bool {
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
        self.closes[open] = close;
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
        let lambda = matches!(self.peek_tok(kids, 0).map(|t| t.kind), Some(Kind::Lambda)) && elems.len() == 1 && !trailing;
        if lambda {
            // `f (\x =\n    body)`: the body one level under the line, not the paren.
            self.opens[c] = true;
            let under = self.docs.add(Node::LineIndent(inner.d));
            let d = self.docs.cat(vec![o.d, under, c_p.d]);
            let g = self.docs.group(d, force);
            self.docs.open.insert(g);
            return P { d: g, first: open, last: c };
        }
        self.opens[c] = true;
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
        let mut fills: Vec<usize> = Vec::new();
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
                if matches!(t.kind, Kind::Op) && FILL_BEFORE.contains(&t.text) && !run.is_empty() && !self.unary(Self::first_tok(tree)) {
                    fills.push(run.len());
                }
            }
            if let Some(prev) = run.last() {
                if self.applies(prev.last, Self::first_tok(tree)) {
                    fills.push(run.len());
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
        let segment = |l: &mut Self, from: usize, to: usize| {
            let at: Vec<usize> = fills.iter().filter(|&&k| from < k && k < to).map(|&k| k - from).collect();
            l.join_fill(&run[from..to], &at)
        };
        if breaks.is_empty() {
            return segment(self, 0, run.len());
        }
        let mut segments = Vec::new();
        let mut from = 0;
        for &(at, _) in &breaks {
            segments.push(segment(self, from, at));
            from = at;
        }
        segments.push(segment(self, from, run.len()));
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
                self.hang_below(head, body, true)
            }
            Kind::Let | Kind::With | Kind::Defer => self.binding_form(ts, pos, halt),
            Kind::If => self.if_form(ts, pos, halt),
            Kind::Is => {
                let k = self.tok(Self::first_tok(&ts[*pos]));
                *pos += 1;
                let bar_or = |t: &Tok| halt(t) || is_bar(t);
                let scrut = self.expr(ts, pos, &bar_or, false);
                let head = self.join(&[k, scrut]).expect("non-empty");
                self.arms(head, ts, pos, halt, true)
            }
            Kind::Do => {
                let k = self.tok(Self::first_tok(&ts[*pos]));
                *pos += 1;
                let ctl_or = |t: &Tok| halt(t) || matches!(t.kind, Kind::Ctl);
                let body = self.expr(ts, pos, &ctl_or, false);
                let head = self.hang(k, body);
                let Some(ctl) = self.expect(ts, pos, |t| matches!(t.kind, Kind::Ctl)) else {
                    return head;
                };
                let bar_or = |t: &Tok| halt(t) || is_bar(t);
                let name = self.expr(ts, pos, &bar_or, false);
                let ctl_head = self.join(&[ctl, name]).expect("non-empty");
                let handled = self.arms(ctl_head, ts, pos, halt, false);
                let joined = self.line_join(head, handled, Node::Line);
                P { d: self.docs.group(joined.d, true), ..joined }
            }
            _ => unreachable!("not a form keyword"),
        }
    }

    /// `head` then `body`, the body moved to its own indented line when the
    /// two do not fit on one. A bracketed body never leaves its opening bracket
    /// alone on a line: it moves down only if it fits there whole, and is
    /// otherwise opened on the head's line.
    fn hang(&mut self, head: P, body: P) -> P {
        if self.opens_block(body) {
            let space = self.docs.text(" ");
            let flat = self.docs.cat(vec![head.d, space, body.d]);
            let tail = self.docs.add(Node::HangTail(body.d));
            let d = self.docs.add(Node::Hang { flat, head: head.d, tail });
            return P { d, first: head.first, last: body.last };
        }
        self.hang_below(head, body, false)
    }

    /// `head` then `body`, the body on its own indented line when the two do
    /// not fit on one: a lambda's body never starts beside `\x =` unless it
    /// fits there whole. A lambda's body (`by_line`) goes one level under the
    /// line its head ends, however deep the expression around it wrapped.
    fn hang_below(&mut self, head: P, body: P, by_line: bool) -> P {
        let line = self.docs.add(Node::Line);
        let inner = self.docs.cat(vec![line, body.d]);
        let mut nested = self.docs.nest(inner);
        if by_line {
            nested = self.docs.add(Node::LineIndent(nested));
        }
        let d = self.docs.cat(vec![head.d, nested]);
        let g = self.docs.group(d, false);
        P { d: g, first: head.first, last: body.last }
    }

    /// The width of `d` printed on one line, if it can be.
    fn flat_width(&self, d: D) -> Option<usize> {
        match &self.docs.nodes[d] {
            Node::Text(s) => (!s.contains('\n')).then(|| s.chars().count()),
            Node::Line | Node::FillLine => Some(1),
            Node::Soft | Node::Fits(_) => Some(0),
            Node::Hard | Node::Fresh | Node::HangTail(_) | Node::Broken(_) | Node::Group(_, true) => None,
            Node::Nest(x) | Node::LineIndent(x) | Node::Group(x, false) => self.flat_width(*x),
            Node::Cat(xs) => xs.iter().map(|x| self.flat_width(*x)).sum(),
            Node::Cond { flat, .. } | Node::Hang { flat, .. } => self.flat_width(*flat),
        }
    }

    /// The width of a left side that lines up with its neighbours, or `None`
    /// for one that does not: too wide (see [`MAX_ALIGNED`]), or with a right
    /// side that could not share its line even at the margin.
    fn left_width(&self, left: P, right: Option<P>) -> Option<usize> {
        let w = self.flat_width(left.d).filter(|&w| w <= MAX_ALIGNED)?;
        let right = right?;
        if self.opens_block(right) || self.closes[right.first] == Some(right.last) {
            return Some(w);
        }
        let r = self.flat_width(right.d)?;
        (w + 4 + r <= self.width).then_some(w)
    }

    /// The widest of the left sides that are lined up.
    fn align_width(&self, lefts: &[Option<usize>]) -> Option<usize> {
        lefts.iter().flatten().copied().max()
    }

    /// The padding that lines a left side of width `w` up to width `to`.
    fn pad(&mut self, w: Option<usize>, to: Option<usize>) -> D {
        match (w, to) {
            (Some(w), Some(to)) if w < to => self.docs.text(&" ".repeat(to - w)),
            _ => self.docs.text(""),
        }
    }

    /// Join pieces as [`Layout::join`] does, except that before each piece
    /// whose index is in `fill` the line may wrap, one level in, so a long run
    /// packs as much on a line as fits.
    fn join_fill(&mut self, ps: &[P], fill: &[usize]) -> P {
        if fill.is_empty() {
            return self.join(ps).expect("non-empty");
        }
        let mut head = vec![ps[0].d];
        let mut tail = Vec::new();
        for k in 1..ps.len() {
            let wraps = fill.contains(&k);
            let sep = if wraps {
                Some(self.docs.add(Node::FillLine))
            } else if self.space(ps[k - 1].last, ps[k].first) {
                Some(self.docs.text(" "))
            } else {
                None
            };
            let into = if wraps || !tail.is_empty() { &mut tail } else { &mut head };
            into.extend(sep);
            into.push(ps[k].d);
        }
        let tail = self.docs.cat(tail);
        let tail = self.docs.nest(tail);
        head.push(tail);
        let d = self.docs.cat(head);
        P { d, first: ps[0].first, last: ps[ps.len() - 1].last }
    }

    /// Whether tokens `a` and `b` are a function and its argument (or two
    /// arguments) side by side, where a long application may wrap. Never
    /// before a `{ }`, `[ ]` or lambda argument, which opens on the call's line.
    fn applies(&self, a: usize, b: usize) -> bool {
        use Kind::*;
        let (x, y) = (&self.toks[a], &self.toks[b]);
        let operand = |k: Kind| matches!(k, Int(_) | Real(_) | Imaginary(_) | Str | Word | At);
        self.space(a, b)
            && (operand(x.kind) || matches!(x.kind, RParen | RBrace | RBrack))
            && (operand(y.kind) || (matches!(y.kind, LParen) && !self.toks.get(b + 1).is_some_and(|t| matches!(t.kind, Lambda))))
    }

    /// Whether the bar at `pos` starts a handler's operation clause
    /// (`| op x`, `| Eff.op x`), which ends an `is` nested in the clause before.
    fn at_op_clause(&self, ts: &[Tree], pos: usize) -> bool {
        let Some((i, _)) = self.peek(ts, pos) else { return false };
        let kinds: Vec<Option<Tok>> = (i..i + 4).map(|k| ts.get(k).and_then(|t| self.tok_of(t)).copied()).collect();
        let word = |k: usize| kinds[k].filter(|t| matches!(t.kind, Kind::Word));
        let lower = |t: Tok| !t.text.starts_with(|c: char| c.is_ascii_uppercase());
        if !kinds[0].is_some_and(|t| is_bar(&t)) {
            return false;
        }
        match word(1) {
            Some(first) if lower(first) => word(2).is_some(),
            Some(_) => kinds[2].is_some_and(|t| matches!(t.kind, Kind::Dot)) && word(3).is_some_and(lower),
            None => false,
        }
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

    /// `let a = x, b = y in body` (and `with`, `defer`). One short binding
    /// stays on the keyword's line, `in` ending it. Several bindings, or one that
    /// does not fit there, make a block: the keyword alone, the bindings
    /// indented with their `=` lined up, then `in` alone and the body indented.
    fn binding_form(&mut self, ts: &[Tree], pos: &mut usize, halt: &dyn Fn(&Tok) -> bool) -> P {
        let is_let = matches!(self.tok_of(&ts[*pos]).map(|t| t.kind), Some(Kind::Let));
        let kw = self.tok(Self::first_tok(&ts[*pos]));
        *pos += 1;
        let sep = |t: &Tok| halt(t) || matches!(t.kind, Kind::Comma | Kind::In);
        let lhs_end = |t: &Tok| sep(t) || matches!(t.kind, Kind::Eq);
        // Per binding: its leading comments, its left side, and `=` with the value.
        let mut binds: Vec<(Vec<P>, P, Option<(P, P)>)> = Vec::new();
        let mut trailing = false;
        loop {
            let cs = self.comments(ts, pos);
            let lhs = self.expr(ts, pos, if is_let { &lhs_end } else { &sep }, false);
            let rhs = match self.expect(ts, pos, |t| is_let && matches!(t.kind, Kind::Eq)) {
                Some(eq) => Some((eq, self.expr(ts, pos, &sep, false))),
                None => None,
            };
            binds.push((cs, lhs, rhs));
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
        let several = binds.len() > 1;
        let widths: Vec<Option<usize>> = binds.iter().map(|b| self.left_width(b.1, b.2.map(|r| r.1))).collect();
        let to = if several { self.align_width(&widths) } else { None };
        let mut items: Vec<D> = Vec::new();
        for (k, (cs, lhs, rhs)) in binds.iter().enumerate() {
            let mut piece = match rhs {
                Some((eq, value)) => {
                    let pad = self.pad(widths[k], to);
                    let space = self.docs.text(" ");
                    let head = self.docs.cat(vec![lhs.d, pad, space, eq.d]);
                    self.hang(P { d: head, first: lhs.first, last: eq.last }, *value)
                }
                None => *lhs,
            };
            piece = self.with_comments(cs.clone(), piece);
            if k + 1 < binds.len() || trailing {
                let comma = self.docs.text(",");
                piece.d = self.docs.cat(vec![piece.d, comma]);
            }
            if k > 0 {
                items.push(self.docs.add(Node::Line));
            }
            items.push(piece.d);
        }
        let last = binds[binds.len() - 1].2.as_ref().map_or(binds[binds.len() - 1].1.last, |r| r.1.last);
        let lead = self.docs.add(Node::Line);
        let list = self.docs.cat(std::iter::once(lead).chain(items).collect());
        let list = self.docs.nest(list);
        let explode = trailing || several;
        let Some(in_kw) = self.expect(ts, pos, |t| matches!(t.kind, Kind::In)) else {
            let d = self.docs.cat(vec![kw.d, list]);
            let g = self.docs.group(d, explode);
            return P { d: g, first: kw.first, last };
        };
        let head = self.docs.reserve();
        let line = self.docs.add(Node::Line);
        let d = self.docs.cat(vec![kw.d, list, line, in_kw.d]);
        self.docs.fill_group(head, d, explode);
        let outer = self.docs.reserve();
        let mut body = self.expr(ts, pos, halt, false);
        // A chain of lets breaks as one: once this one ends its line with `in`,
        // so does every let after it.
        if matches!(self.toks[body.first].kind, Kind::Let) {
            while let Node::Cat(xs) = &self.docs.nodes[body.d] {
                let [x] = xs[..] else { break };
                body.d = x;
            }
            if let Node::Group(_, false) = self.docs.nodes[body.d] {
                let broken = self.docs.add(Node::Broken(body.d));
                body.d = self.docs.add(Node::Cond { group: outer, flat: body.d, broken });
            }
        }
        let line = self.docs.add(Node::Line);
        let flat = self.docs.cat(vec![line, body.d]);
        let line = self.docs.add(Node::Line);
        let broken = self.docs.cat(vec![line, body.d]);
        let broken = self.docs.nest(broken);
        let after = self.docs.add(Node::Cond { group: head, flat, broken });
        let d = self.docs.cat(vec![head, after]);
        self.docs.fill_group(outer, d, false);
        P { d: outer, first: kw.first, last: body.last }
    }

    /// `if c => t else e` on one line, or broken: every body on a line of its
    /// own one level under its `=>` or `else`, each `else` (or `else if`)
    /// starting a line at the level of the first `if`. A keyword with several
    /// comma-separated arms makes a block: the keyword alone, then the arms and
    /// the `else` indented, either each on one line with their `=>` lined up
    /// or, when one of them does not fit so, every body under its arm.
    fn if_form(&mut self, ts: &[Tree], pos: &mut usize, halt: &dyn Fn(&Tok) -> bool) -> P {
        let arrow_or = |t: &Tok| halt(t) || matches!(t.kind, Kind::FatArrow);
        let branch_end = |t: &Tok| halt(t) || matches!(t.kind, Kind::Comma | Kind::Else);
        let mut kw = self.tok(Self::first_tok(&ts[*pos]));
        *pos += 1;
        let mut lines: Vec<P> = Vec::new();
        let mut explode = false;
        loop {
            // Per arm: the condition, `=>` with the branch, and its comma.
            let mut arms: Vec<(P, Option<(P, P)>, bool)> = Vec::new();
            let mut trailing = false;
            loop {
                let cond = self.expr(ts, pos, &arrow_or, false);
                let then = match self.expect(ts, pos, |t| matches!(t.kind, Kind::FatArrow)) {
                    Some(arrow) => Some((arrow, self.expr(ts, pos, &branch_end, false))),
                    None => None,
                };
                let comma = self.expect(ts, pos, |t| matches!(t.kind, Kind::Comma)).is_some();
                arms.push((cond, then, comma));
                if !comma {
                    break;
                }
                if matches!(self.peek_tok(ts, *pos).map(|t| t.kind), Some(Kind::Else)) {
                    trailing = true;
                    break;
                }
            }
            let else_kw = self.expect(ts, pos, |t| matches!(t.kind, Kind::Else));
            let chained = else_kw.is_some() && matches!(self.peek_tok(ts, *pos).map(|t| t.kind), Some(Kind::If));
            if arms.len() > 1 || trailing {
                explode = true;
                let widths: Vec<Option<usize>> = arms.iter().map(|a| self.left_width(a.0, a.1.map(|t| t.1))).collect();
                let to = self.align_width(&widths).filter(|_| widths.iter().all(Option::is_some));
                // Lined up only if every arm's line fits, decided where the arms start.
                let probe = to.map(|to| {
                    let w = arms
                        .iter()
                        .filter_map(|(_, then, comma)| {
                            let (_, b) = then.as_ref()?;
                            let r = self.flat_width(b.d).unwrap_or_else(|| self.opening_width(*b));
                            Some(to + 4 + r + usize::from(*comma))
                        })
                        .max()
                        .unwrap_or(0);
                    self.docs.add(Node::Fits(w))
                });
                let mut inner = Vec::new();
                for (k, (cond, then, comma)) in arms.iter().enumerate() {
                    let mut entry = match then {
                        Some((arrow, branch)) => {
                            let space = self.docs.text(" ");
                            let head = self.docs.cat(vec![cond.d, space, arrow.d]);
                            let below = self.under(P { d: head, first: cond.first, last: arrow.last }, *branch);
                            match probe {
                                Some(group) => {
                                    let pad = self.pad(widths[k], to);
                                    let (s1, s2) = (self.docs.text(" "), self.docs.text(" "));
                                    let beside = self.docs.cat(vec![cond.d, pad, s1, arrow.d, s2, branch.d]);
                                    let beside = self.docs.group(beside, false);
                                    let d = self.docs.add(Node::Cond { group, flat: beside, broken: below.d });
                                    P { d, ..below }
                                }
                                None => below,
                            }
                        }
                        None => *cond,
                    };
                    if *comma {
                        let c = self.docs.text(",");
                        entry.d = self.docs.cat(vec![entry.d, c]);
                    }
                    inner.push(self.docs.add(Node::Line));
                    if k == 0 {
                        inner.extend(probe);
                    }
                    inner.push(entry.d);
                }
                let mut last = arms[arms.len() - 1].1.map_or(arms[arms.len() - 1].0.last, |t| t.1.last);
                if let Some(e) = else_kw.filter(|_| !chained) {
                    let body = self.expr(ts, pos, halt, false);
                    let below = self.under(e, body);
                    let d = match probe {
                        Some(group) => {
                            let beside = self.hang_below(e, body, false);
                            self.docs.add(Node::Cond { group, flat: beside.d, broken: below.d })
                        }
                        None => below.d,
                    };
                    inner.push(self.docs.add(Node::Line));
                    inner.push(d);
                    last = body.last;
                }
                let inner = self.docs.cat(inner);
                let inner = self.docs.nest(inner);
                let d = self.docs.cat(vec![kw.d, inner]);
                lines.push(P { d, first: kw.first, last });
            } else {
                let (cond, then, _) = arms[0];
                let head = self.join(&[kw, cond]).expect("non-empty");
                lines.push(match then {
                    Some((arrow, branch)) => {
                        let space = self.docs.text(" ");
                        let d = self.docs.cat(vec![head.d, space, arrow.d]);
                        self.under(P { d, first: head.first, last: arrow.last }, branch)
                    }
                    None => head,
                });
                if let Some(e) = else_kw.filter(|_| !chained) {
                    let body = self.expr(ts, pos, halt, false);
                    lines.push(self.under(e, body));
                }
            }
            if !chained {
                break;
            }
            let if_kw = self.expect(ts, pos, |t| matches!(t.kind, Kind::If)).expect("peeked");
            kw = self.join(&[else_kw.expect("chained"), if_kw]).expect("non-empty");
        }
        let mut acc = lines[0];
        for &l in &lines[1..] {
            acc = self.line_join(acc, l, Node::Line);
        }
        let commented = (acc.first..=acc.last).any(|i| matches!(self.toks[i].kind, Kind::Comment));
        let g = self.docs.group(acc.d, explode || commented);
        P { d: g, ..acc }
    }

    /// `head body`, or, when the group around it breaks, `body` on the next
    /// line one level in.
    fn under(&mut self, head: P, body: P) -> P {
        let line = self.docs.add(Node::Line);
        let inner = self.docs.cat(vec![line, body.d]);
        let nested = self.docs.nest(inner);
        let d = self.docs.cat(vec![head.d, nested]);
        P { d, first: head.first, last: body.last }
    }

    /// Whether `p` ends in a group that opens at the end of its line (a
    /// `{ }`, `[ ]` or `( )`), so it can start beside its head rather than on
    /// a line of its own. A sequence `a; b { }` does not: only its last part
    /// ends in the group.
    fn opens_block(&self, p: P) -> bool {
        let t = self.toks[p.first];
        if !self.opens[p.last] || starts_form(&t) || matches!(t.kind, Kind::With) {
            return false;
        }
        let mut i = p.first;
        while i < p.last {
            if let Some(c) = self.closes[i] {
                if c == p.last {
                    return true;
                }
                i = c + 1;
                continue;
            }
            if matches!(self.toks[i].kind, Kind::Op) && self.toks[i].text == ";" {
                return false;
            }
            i += 1;
        }
        true
    }

    /// The width of `p` up to where its last group opens: `f x {`, or through
    /// the `=` of `f (\x =`.
    fn opening_width(&self, p: P) -> usize {
        let open = (p.first..=p.last).find(|&i| self.closes[i] == Some(p.last)).unwrap_or(p.last);
        let lambda = self.toks.get(open + 1).is_some_and(|t| matches!(t.kind, Kind::Lambda));
        let end = if lambda {
            (open..p.last).find(|&i| matches!(self.toks[i].kind, Kind::Eq)).unwrap_or(open)
        } else {
            open
        };
        (p.first..=end).map(|i| self.toks[i].text.chars().count() + 1).sum::<usize>().saturating_sub(1)
    }

    /// `head | p => e | q if g => f`: on one line, or the arms one per line
    /// under the head with their `=>` lined up. An `is` (`clauses_end`) stops
    /// at a handler's operation clause, as the parser does.
    fn arms(&mut self, head: P, ts: &[Tree], pos: &mut usize, halt: &dyn Fn(&Tok) -> bool, clauses_end: bool) -> P {
        let group = self.docs.reserve();
        // Per arm: its patterns and guard, then `=>` with the body.
        let mut arms: Vec<(P, Option<(P, P)>)> = Vec::new();
        loop {
            if clauses_end && self.at_op_clause(ts, *pos) {
                break;
            }
            let Some(bar) = self.expect(ts, pos, is_bar) else { break };
            let pat_end = |t: &Tok| halt(t) || is_bar(t) || matches!(t.kind, Kind::FatArrow | Kind::If);
            let mut parts = vec![bar];
            let mut alts = Vec::new();
            loop {
                parts.push(self.expr(ts, pos, &pat_end, false));
                match self.expect(ts, pos, is_bar) {
                    Some(alt) => {
                        alts.push(parts.len());
                        parts.push(alt);
                    }
                    None => break,
                }
            }
            let mut lead = self.join_fill(&parts, &alts);
            if let Some(kw) = self.expect(ts, pos, |t| matches!(t.kind, Kind::If)) {
                let arrow_or = |t: &Tok| halt(t) || matches!(t.kind, Kind::FatArrow);
                let guard = self.expr(ts, pos, &arrow_or, false);
                lead = self.join(&[lead, kw, guard]).expect("non-empty");
            }
            let body = match self.expect(ts, pos, |t| matches!(t.kind, Kind::FatArrow)) {
                Some(arrow) => {
                    let bar_or = |t: &Tok| halt(t) || is_bar(t);
                    Some((arrow, self.expr(ts, pos, &bar_or, false)))
                }
                None => None,
            };
            arms.push((lead, body));
        }
        let widths: Vec<Option<usize>> = arms.iter().map(|a| self.left_width(a.0, a.1.map(|b| b.1))).collect();
        let to = self.align_width(&widths);
        let mut acc = head;
        for (k, (lead, body)) in arms.iter().enumerate() {
            let arm = match body {
                Some((arrow, body)) => {
                    let flat = self.docs.text("");
                    let broken = self.pad(widths[k], to);
                    let pad = self.docs.add(Node::Cond { group, flat, broken });
                    let space = self.docs.text(" ");
                    let d = self.docs.cat(vec![lead.d, pad, space, arrow.d]);
                    self.hang(P { d, first: lead.first, last: arrow.last }, *body)
                }
                None => *lead,
            };
            acc = self.line_join(acc, arm, Node::Line);
        }
        self.docs.fill_group(group, acc.d, arms.len() > MAX_FLAT_ARMS);
        P { d: group, ..acc }
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
        if matches!(ts.get(1).and_then(|t| self.tok_of(t)).map(|t| t.kind), Some(Kind::With)) {
            return self.import(ts);
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
        // A long signature wraps before its arrows.
        let arrows: Vec<usize> = (1..head_ps.len())
            .filter(|&k| head_ps[k].first == head_ps[k].last && matches!(self.toks[head_ps[k].first].kind, Kind::Arrow))
            .collect();
        let head = self.join_fill(&head_ps, &arrows);
        let (elems, trailing, tail) = self.comma_list(&ts[pos..]);
        let mut body = elems[0];
        for &e in &elems[1..] {
            let comma = self.docs.text(",");
            let d = self.docs.cat(vec![body.d, comma]);
            body = self.line_join(P { d, ..body }, e, Node::Line);
        }
        let exploded = trailing && self.multiline(eq_tok, self.toks.len() - 1);
        let body = self.close_list(body, trailing, tail);
        // A body opens beside the head only when the head and the opening fit
        // one line; a head that must wrap reads better with the body below it.
        let beside = self.flat_width(head.d).is_some_and(|w| w + 1 + self.opening_width(body) <= self.width);
        if elems.len() == 1 && !exploded && !head_comment && self.opens_block(body) && beside {
            return self.hang(head, body).d;
        }
        let line = self.docs.add(Node::Line);
        let inner = self.docs.cat(vec![line, body.d]);
        let nested = self.docs.nest(inner);
        let d = self.docs.cat(vec![head.d, nested]);
        self.docs.group(d, exploded || head_comment)
    }

    /// `$ with A, B = C`: one line when it fits, else one entry per line under
    /// the `$ with`. A trailing comma on a list written over several lines
    /// keeps it one entry per line.
    fn import(&mut self, ts: &[Tree]) -> D {
        let dollar = self.tree(&ts[0]);
        let with = self.tree(&ts[1]);
        let head = self.join(&[dollar, with]).expect("non-empty");
        let is_comma = |me: &Self, t: &Tree| matches!(me.tok_of(t).map(|t| t.kind), Some(Kind::Comma));
        // Each entry with its comma (if any) and the comments that end its line.
        let mut entries: Vec<(P, bool, Vec<P>)> = Vec::new();
        let mut tail = Vec::new();
        let mut pos = 2;
        while pos < ts.len() {
            let cs = self.comments(ts, &mut pos);
            let start = pos;
            while pos < ts.len() && !self.is_comment(&ts[pos]) && !is_comma(self, &ts[pos]) {
                pos += 1;
            }
            if start == pos && !(pos < ts.len() && is_comma(self, &ts[pos])) {
                tail = cs;
                continue;
            }
            let parts: Vec<P> = ts[start..pos].iter().map(|t| self.tree(t)).collect();
            let entry = match self.join(&parts) {
                Some(entry) => self.with_comments(cs, entry),
                None => {
                    let d = self.docs.text("");
                    self.with_comments(cs, P { d, first: start, last: start })
                }
            };
            let comma = pos < ts.len() && is_comma(self, &ts[pos]);
            if comma {
                pos += 1;
            }
            let mut after = Vec::new();
            while pos < ts.len() && self.is_comment(&ts[pos]) && !self.toks[Self::first_tok(&ts[pos])].newline {
                after.push(self.comment(Self::first_tok(&ts[pos])));
                pos += 1;
            }
            entries.push((entry, comma, after));
        }
        let Some(&(_, trailing, _)) = entries.last() else {
            return head.d;
        };
        let mut body: Option<P> = None;
        for (entry, comma, after) in entries {
            let mut ds = vec![entry.d];
            if comma {
                ds.push(self.docs.text(","));
            }
            for c in &after {
                ds.push(self.docs.text(" "));
                ds.push(c.d);
            }
            let last = after.last().map_or(entry.last, |c| c.last);
            let piece = P { d: self.docs.cat(ds), first: entry.first, last };
            body = Some(match body {
                Some(b) => self.line_join(b, piece, Node::Line),
                None => piece,
            });
        }
        let body = self.close_list(body.expect("non-empty"), false, tail);
        let exploded = trailing && self.multiline(head.last, self.toks.len() - 1);
        let line = self.docs.add(Node::Line);
        let inner = self.docs.cat(vec![line, body.d]);
        let nested = self.docs.nest(inner);
        let d = self.docs.cat(vec![head.d, nested]);
        self.docs.group(d, exploded)
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
