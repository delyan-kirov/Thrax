//! The document printer: Wadler's "prettier printer".
//!
//! The algorithm is from Philip Wadler, "A prettier printer" (1998). A document
//! is a tree of text, line points and groups ([`Node`]), stored in [`Docs`]
//! and addressed by index. [`print`] walks it with a stack of (indent, mode,
//! node) frames: a group is printed flat when [`fits`] says its flat form, and
//! everything up to the next possible break after it, fits in the remaining
//! width; otherwise its line points break.
//!
//! Input: a document built by [`super::layout`]. Output: the text.
//!
//! Entry point: [`print`].

use super::Style;

pub(super) type D = usize;

pub(super) enum Node {
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
    /// A space when flat; when broken, a newline only if what follows, up to
    /// the next line point, does not fit on this line.
    FillLine,
    /// `flat` or `broken`, as the group `group` was printed. The group is
    /// printed before this node; while it is being fitted, `flat` is assumed.
    Cond { group: D, flat: D, broken: D },
    /// A head and a bracketed body: on one line (`flat`) if it fits, else
    /// `head` then `tail`.
    Hang { flat: D, head: D, tail: D },
    /// The group `D`, always broken.
    Broken(D),
    /// Prints nothing. Decided flat when this many columns fit from where it
    /// is printed, so a [`Node::Cond`] after it can pick a layout by the width
    /// of a line that is not the first.
    Fits(usize),
    /// `D` indented from the indentation of the line it starts on, so a
    /// lambda opened at the end of a line nests one level under that line.
    LineIndent(D),
    /// The bracket of a [`Node::Hang`] that did not fit beside its head: on
    /// the next line, indented, if it fits there whole, else opened on the
    /// head's line and broken.
    HangTail(D),
}

/// The document arena for one item: nodes refer to each other by index, and
/// the whole thing is dropped when the item is printed.
#[derive(Default)]
pub(super) struct Docs {
    pub(super) nodes: Vec<Node>,
    /// Groups that open at the end of a line (a trailing `(\x = ...)`): when
    /// fitting what comes before one, only its text up to its first line
    /// point counts.
    pub(super) open: std::collections::HashSet<D>,
}

impl Docs {
    pub(super) fn add(&mut self, n: Node) -> D {
        self.nodes.push(n);
        self.nodes.len() - 1
    }
    pub(super) fn text(&mut self, s: &str) -> D {
        self.add(Node::Text(s.to_string()))
    }
    pub(super) fn cat(&mut self, ds: Vec<D>) -> D {
        self.add(Node::Cat(ds))
    }
    pub(super) fn nest(&mut self, d: D) -> D {
        self.add(Node::Nest(d))
    }
    pub(super) fn group(&mut self, d: D, force: bool) -> D {
        self.add(Node::Group(d, force))
    }
    /// A slot for a group whose contents refer to it (through [`Node::Cond`]);
    /// filled by [`Docs::fill_group`].
    pub(super) fn reserve(&mut self) -> D {
        self.add(Node::Text(String::new()))
    }
    pub(super) fn fill_group(&mut self, at: D, d: D, force: bool) {
        self.nodes[at] = Node::Group(d, force);
    }
}

#[derive(Clone, Copy, PartialEq)]
pub(super) enum Mode {
    Flat,
    Break,
}

pub(super) fn print(docs: &Docs, root: D, style: &Style) -> String {
    let mut out = String::new();
    let mut col = 0usize;
    let mut stack: Vec<(usize, Mode, D)> = vec![(0, Mode::Break, root)];
    let mut decided: Vec<Option<Mode>> = vec![None; docs.nodes.len()];
    let newline = |out: &mut String, ind: usize| {
        while out.ends_with(' ') {
            out.pop();
        }
        out.push('\n');
        out.extend(std::iter::repeat_n(' ', ind));
    };
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
                    newline(&mut out, ind);
                    col = ind;
                }
            }
            Node::FillLine => {
                let room = style.width.saturating_sub(col + 1);
                let fits_here = match stack.split_last() {
                    Some((&next, rest)) => fits_up_to(docs, &decided, room, next, rest, false),
                    None => true,
                };
                if mode == Mode::Flat || fits_here {
                    out.push(' ');
                    col += 1;
                } else {
                    newline(&mut out, ind);
                    col = ind;
                }
            }
            Node::Cond { group, flat, broken } => {
                let x = if decided[*group] == Some(Mode::Break) { *broken } else { *flat };
                stack.push((ind, mode, x));
            }
            Node::Fits(w) => decided[d] = Some(if col + w <= style.width { Mode::Flat } else { Mode::Break }),
            Node::Hang { flat, head, tail } => {
                if mode == Mode::Flat || fits(docs, &decided, style.width.saturating_sub(col), (ind, Mode::Flat, *flat), &stack) {
                    stack.push((ind, Mode::Flat, *flat));
                } else {
                    stack.push((ind, mode, *tail));
                    stack.push((ind, mode, *head));
                }
            }
            Node::LineIndent(x) => {
                let line = &out[out.rfind('\n').map_or(0, |i| i + 1)..];
                stack.push((line.len() - line.trim_start().len(), mode, *x));
            }
            Node::Broken(g) => {
                let Node::Group(x, _) = docs.nodes[*g] else { unreachable!("Broken wraps a group") };
                decided[*g] = Some(Mode::Break);
                stack.push((ind, Mode::Break, x));
            }
            Node::HangTail(body) => {
                let line = &out[out.rfind('\n').map_or(0, |i| i + 1)..];
                let here = line.len() - line.trim_start().len();
                let under = here + style.indent;
                if fits(docs, &decided, style.width.saturating_sub(under), (under, Mode::Flat, *body), &stack) {
                    newline(&mut out, under);
                    col = under;
                    stack.push((under, Mode::Flat, *body));
                } else {
                    out.push(' ');
                    col += 1;
                    stack.push((here, Mode::Break, *body));
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
            Node::Cat(xs) => {
                for x in xs.iter().rev() {
                    stack.push((ind, mode, *x));
                }
            }
            Node::Group(x, force) => {
                let m = if !*force && mode == Mode::Flat {
                    Mode::Flat
                } else if !*force && fits(docs, &decided, style.width.saturating_sub(col), (ind, Mode::Flat, *x), &stack) {
                    Mode::Flat
                } else {
                    Mode::Break
                };
                decided[d] = Some(m);
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
pub(super) fn fits(docs: &Docs, decided: &[Option<Mode>], room: usize, next: (usize, Mode, D), rest: &[(usize, Mode, D)]) -> bool {
    fits_up_to(docs, decided, room, next, rest, true)
}

/// [`fits`], where `lenient` measures the groups after `next` only up to their
/// first line point; otherwise they must fit whole, up to a line point outside
/// any group.
pub(super) fn fits_up_to(
    docs: &Docs,
    decided: &[Option<Mode>],
    room: usize,
    next: (usize, Mode, D),
    rest: &[(usize, Mode, D)],
    lenient: bool,
) -> bool {
    let mut room = room as isize;
    // `after` marks a piece whose groups may still break, so only their text
    // up to their first line point has to fit; `later` marks what follows `next`.
    let mut work: Vec<(usize, Mode, D, bool, bool)> = vec![(next.0, next.1, next.2, false, false)];
    let mut rest = rest.iter().rev();
    loop {
        let Some((ind, mode, d, after, later)) =
            work.pop().or_else(|| rest.next().map(|&(i, m, d)| (i, m, d, lenient, true)))
        else {
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
            Node::Nest(x) => work.push((ind, mode, *x, after, later)),
            Node::Fits(_) => {}
            Node::FillLine => match mode {
                Mode::Break => return true,
                Mode::Flat => room -= 1,
            },
            Node::Cond { group, flat, broken } => {
                work.push((ind, mode, if decided[*group] == Some(Mode::Break) { *broken } else { *flat }, after, later));
            }
            Node::Hang { flat, head, tail } => match mode {
                Mode::Flat => work.push((ind, Mode::Flat, *flat, after, later)),
                Mode::Break => {
                    work.push((ind, mode, *tail, after, later));
                    work.push((ind, mode, *head, after, later));
                }
            },
            // Counted as if it stays beside its head: what it opens with must fit.
            Node::HangTail(body) => {
                room -= 1;
                work.push((ind, Mode::Break, *body, true, later));
            }
            Node::Broken(g) => match (after, &docs.nodes[*g]) {
                (true, Node::Group(x, _)) => work.push((ind, Mode::Break, *x, true, later)),
                _ => return false,
            },
            Node::Cat(xs) => {
                for x in xs.iter().rev() {
                    work.push((ind, mode, *x, after, later));
                }
            }
            Node::LineIndent(x) => work.push((ind, mode, *x, after, later)),
            Node::Group(x, force) => {
                if after || (later && docs.open.contains(&d)) {
                    work.push((ind, Mode::Break, *x, true, later));
                } else if *force {
                    return false;
                } else {
                    work.push((ind, Mode::Flat, *x, false, later));
                }
            }
        }
        if room < 0 {
            return false;
        }
    }
}
