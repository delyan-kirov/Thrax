//! Hygiene: the names a quotation spells resolved where it was written, before
//! its code is spliced. See Note [Quotation scopes] in
//! `interpreter::machine::data`.
//!
//! The spliced code is parsed in the category its site splices, and each name in
//! a run of text a module's quotation wrote is rewritten so it means at the site
//! what it meant where it was written:
//!
//! - a binder is renamed apart, `x` to `x__7` (the scope's id), and so is every
//!   name of the same scope it binds, so it binds nothing else;
//! - a free name the quotation's module defines or imports is qualified with
//!   that module, so nothing at the site can capture it, and the site imports the
//!   module under its own name if it reaches it no other way.
//!
//! Text no module's quotation wrote (a hole the site filled, `@code` text) is
//! left as it is: it reads as written at the site. So is a name the fragment's
//! own items declare, which the items are spliced to define.

use std::collections::{HashMap, HashSet};
use std::rc::Rc;

use frontend::{Ast, Expr, FieldInit, FieldPat, Item, Pattern, SliceSlot, Ty, EffectRow, Homes};
use interpreter::machine::data::{Fragment, Quoted};
use utilities::{Aol, Span, StrId};

/// Code made hygienic: its text, and the modules the site must import (under
/// their own names) for the qualified names it now spells.
pub(super) struct Resolved {
    pub text: String,
    pub imports: Vec<StrId>,
}

/// Rewrite `code`, which parses as `kind`, so the runs of it in `quoted` resolve
/// as written. `site` is the module it is spliced into, and `homes` says where
/// each module's bare names are defined.
pub(super) fn resolve(code: &str, quoted: &[Quoted], kind: Fragment, site: StrId, homes: &HashMap<StrId, Rc<Homes>>) -> Resolved {
    if quoted.is_empty() {
        return Resolved { text: code.to_string(), imports: Vec::new() };
    }
    let Ok((parsed, base)) = interpreter::machine::data::parse_fragment(code, kind) else {
        return Resolved { text: code.to_string(), imports: Vec::new() };
    };
    let wrapped = interpreter::machine::data::wrap_fragment(code, kind);
    let mut walk = Walk {
        ast: &parsed.ast,
        src: &wrapped,
        base,
        len: code.len(),
        quoted,
        site,
        homes,
        env: Vec::new(),
        edits: HashMap::new(),
        imports: Vec::new(),
        globals: HashSet::new(),
        types: HashSet::new(),
    };
    let items = parsed.ast.slice(parsed.program.items).to_vec();
    walk.declare(&items);
    for item in &items {
        walk.item(item);
    }
    let mut edits: Vec<(usize, usize, String)> = walk.edits.into_iter().map(|(s, (e, t))| (s, e, t)).collect();
    edits.sort_by(|a, b| b.0.cmp(&a.0));
    let mut text = code.to_string();
    for (start, end, replacement) in edits {
        text.replace_range(start - base..end - base, &replacement);
    }
    Resolved { text, imports: walk.imports }
}

/// Which quotation wrote a binder, as name resolution compares them.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Origin {
    /// Text written at the site.
    Site,
    /// A module's quotation, by scope id.
    Scope(u64),
    /// A binder with no text to read its origin from, such as a parser-made
    /// one: it binds a name of any origin, and is never renamed.
    Any,
}

struct Binder {
    name: StrId,
    origin: Origin,
    /// What a name it binds is rewritten to, for a renamed binder.
    renamed: Option<String>,
}

struct Walk<'a> {
    ast: &'a Ast,
    /// The fragment wrapped as a module, which the spans index.
    src: &'a str,
    /// Where the fragment starts in `src`, and its length.
    base: usize,
    len: usize,
    quoted: &'a [Quoted],
    site: StrId,
    homes: &'a HashMap<StrId, Rc<Homes>>,
    /// The local binders in scope, innermost last.
    env: Vec<Binder>,
    /// Rewrites of `src`, keyed by start: `start -> (end, replacement)`.
    edits: HashMap<usize, (usize, String)>,
    imports: Vec<StrId>,
    /// The values and types the fragment's items declare.
    globals: HashSet<StrId>,
    types: HashSet<StrId>,
}

impl<'a> Walk<'a> {
    /// The quotation run that wrote the byte of `src` at `at`, if a module's did.
    fn quoted_at(&self, at: usize) -> Option<Quoted> {
        if at < self.base || at >= self.base + self.len {
            return None;
        }
        let at = at - self.base;
        self.quoted.iter().copied().find(|q| q.start <= at && at < q.end)
    }

    fn edit(&mut self, start: usize, end: usize, text: String) {
        if start >= self.base && end <= self.base + self.len {
            self.edits.insert(start, (end, text));
        }
    }

    /// The first whole-word occurrence of `name` in `span`.
    fn word_in(&self, span: Option<Span>, name: StrId) -> Option<usize> {
        let span = span?;
        let hay = self.src.get(span.start..span.end.min(self.src.len()))?;
        let name = name.as_str();
        let bytes = self.src.as_bytes();
        let ident = |b: u8| frontend::lexer::data::is_ident_cont(b);
        let mut from = 0;
        while let Some(i) = hay[from..].find(name) {
            let at = span.start + from + i;
            let end = at + name.len();
            let before = at == 0 || !ident(bytes[at - 1]);
            let after = end >= bytes.len() || !ident(bytes[end]);
            if before && after {
                return Some(at);
            }
            from += i + 1;
        }
        None
    }

    /// The binder `name` spelled just before the `=>` that `body` follows, as
    /// in an effect clause `| op x => body`.
    fn binder_before(&self, body: Aol<Expr>, name: StrId) -> Option<usize> {
        let start = self.ast.expr_span(body)?.start;
        let head = self.src.get(..start)?.trim_end();
        let head = head.strip_suffix("=>")?.trim_end();
        let at = head.len().checked_sub(name.as_str().len())?;
        (head.get(at..)? == name.as_str()).then_some(at)
    }

    /// The qualifier the site reaches `module` by, `None` for the site itself.
    fn qualifier(&mut self, module: StrId) -> Option<StrId> {
        if module == self.site {
            return None;
        }
        if let Some(q) = self.homes.get(&self.site).and_then(|h| h.qualifiers.get(&module)) {
            return Some(*q);
        }
        if !self.imports.contains(&module) {
            self.imports.push(module);
        }
        Some(module)
    }

    // -- names ----------------------------------------------------------------

    /// Bring binder `name`, spelled at `at`, into scope, renaming it apart if a
    /// module's quotation wrote it.
    fn bind(&mut self, name: StrId, at: Option<usize>) {
        let Some(at) = at else {
            self.env.push(Binder { name, origin: Origin::Any, renamed: None });
            return;
        };
        match self.quoted_at(at) {
            Some(q) => {
                let renamed = format!("{name}__{}", q.scope);
                self.edit(at, at + name.as_str().len(), renamed.clone());
                self.env.push(Binder { name, origin: Origin::Scope(q.scope), renamed: Some(renamed) });
            }
            None => self.env.push(Binder { name, origin: Origin::Site, renamed: None }),
        }
    }

    /// A field-pattern shorthand `.name` in `span` binds `name`. A renamed one
    /// is spelled out as `.name = renamed`.
    fn bind_shorthand(&mut self, span: Option<Span>, name: StrId, children: &[Span]) {
        let mut at = None;
        if let Some(span) = span {
            let mut from = span.start;
            while let Some(i) = self.word_in(Some(Span::new(from, span.end)), name) {
                let inside = children.iter().any(|c| c.start <= i && i < c.end);
                let dotted = self.src[..i].trim_end().ends_with('.');
                let named = self.src[i + name.as_str().len()..].trim_start().starts_with('=')
                    && !self.src[i + name.as_str().len()..].trim_start().starts_with("=>");
                if !inside && dotted && !named {
                    at = Some(i);
                    break;
                }
                from = i + 1;
            }
        }
        self.bind(name, at);
        if let (Some(at), Some(Binder { renamed: Some(renamed), .. })) = (at, self.env.last()) {
            let spelled = format!("{name} = {renamed}");
            self.edit(at, at + name.as_str().len(), spelled);
        }
    }

    /// A bare reference to `name` within `span`.
    fn reference(&mut self, name: StrId, span: Option<Span>) {
        if is_synthetic(name) {
            return;
        }
        let Some(at) = self.word_in(span, name) else { return };
        let quoted = self.quoted_at(at);
        let origin = quoted.map_or(Origin::Site, |q| Origin::Scope(q.scope));
        if let Some(b) = self.env.iter().rev().find(|b| b.name == name && (b.origin == origin || b.origin == Origin::Any)) {
            if let Some(renamed) = b.renamed.clone() {
                self.edit(at, at + name.as_str().len(), renamed);
            }
            return;
        }
        let Some(q) = quoted else { return };
        if self.globals.contains(&name) {
            return;
        }
        let Some(home) = self.homes.get(&q.module).and_then(|h| h.values.get(&name)).copied() else { return };
        if let Some(qual) = self.qualifier(home) {
            self.edit(at, at + name.as_str().len(), format!("{qual}.{name}"));
        }
    }

    /// A type name within `span`, bare.
    fn type_name(&mut self, name: StrId, span: Option<Span>) {
        if is_synthetic(name) || self.types.contains(&name) {
            return;
        }
        let Some(at) = self.word_in(span, name) else { return };
        let Some(q) = self.quoted_at(at) else { return };
        let Some(home) = self.homes.get(&q.module).and_then(|h| h.types.get(&name)).copied() else { return };
        // No local binds a type, so a type the site reaches bare as the same one
        // needs no qualifier.
        if self.homes.get(&self.site).and_then(|h| h.types.get(&name)) == Some(&home) {
            return;
        }
        if let Some(qual) = self.qualifier(home) {
            self.edit(at, at + name.as_str().len(), format!("{qual}.{name}"));
        }
    }

    /// The module qualifier `module` that a qualified name within `span` starts
    /// with: what it names where written, the site reaches by its own qualifier.
    fn qualified(&mut self, module: StrId, span: Option<Span>) {
        let Some(at) = self.word_in(span, module) else { return };
        let Some(q) = self.quoted_at(at) else { return };
        let Some(target) = self.homes.get(&q.module).and_then(|h| h.modules.get(&module)).copied() else { return };
        let qual = self.qualifier(target).unwrap_or(self.site);
        if qual != module {
            self.edit(at, at + module.as_str().len(), qual.to_string());
        }
    }

    // -- the tree ---------------------------------------------------------------

    /// Record what the fragment's items declare: the site names them as the
    /// fragment spells them.
    fn declare(&mut self, items: &[Item]) {
        for item in items {
            match item {
                Item::Def { name, .. } => {
                    self.globals.insert(*name);
                }
                Item::Struct { name, params, .. } | Item::Union { name, params, .. } | Item::Alias { name, params, .. } => {
                    self.types.insert(*name);
                    self.types.extend(self.ast.slice(*params).iter().copied());
                }
                Item::Effect { name, params, ops } => {
                    self.types.insert(*name);
                    self.types.extend(self.ast.slice(*params).iter().copied());
                    self.globals.extend(self.ast.slice(*ops).iter().map(|op| op.name));
                }
                Item::Import { .. } | Item::Private | Item::Run(..) => {}
            }
        }
    }

    fn item(&mut self, item: &Item) {
        match item {
            Item::Def { sig, ctx, body, .. } => {
                if let Some(t) = sig {
                    self.ty(*t);
                }
                if let Some(t) = ctx {
                    self.ty(*t);
                }
                self.expr(*body);
            }
            Item::Struct { fields, splices, .. } => {
                for s in self.ast.slice(*splices).to_vec() {
                    self.expr(s.expr);
                }
                for f in self.ast.slice(*fields).to_vec() {
                    self.ty(f.ty);
                }
            }
            Item::Union { variants, splices, .. } => {
                for s in self.ast.slice(*splices).to_vec() {
                    self.expr(s.expr);
                }
                for v in self.ast.slice(*variants).to_vec() {
                    match v.payload {
                        frontend::Payload::None => {}
                        frontend::Payload::Bare(t) => self.ty(t),
                        frontend::Payload::Fields(fs) => {
                            for f in self.ast.slice(fs).to_vec() {
                                self.ty(f.ty);
                            }
                        }
                    }
                }
            }
            Item::Alias { ty, .. } => self.ty(*ty),
            Item::Effect { ops, .. } => {
                for op in self.ast.slice(*ops).to_vec() {
                    self.ty(op.ty);
                }
            }
            Item::Run(e, _) => self.expr(*e),
            Item::Import { .. } | Item::Private => {}
        }
    }

    fn ty(&mut self, t: Aol<Ty>) {
        let span = self.ast.ty_span(t);
        match self.ast.ty(t).clone() {
            Ty::Con { module: None, name } => self.type_name(name, span),
            Ty::Con { module: Some(m), .. } => self.qualified(m, span),
            Ty::Var(_) | Ty::Nat(_) | Ty::Unit => {}
            Ty::SizeAdd(a, b) | Ty::SizeMul(a, b) | Ty::App(a, b) => {
                self.ty(a);
                self.ty(b);
            }
            Ty::Sized { size, elem, .. } => {
                self.ty(size);
                self.ty(elem);
            }
            Ty::MetaE(e) => self.expr(e),
            Ty::Arrow { from, effect, to } => {
                self.ty(from);
                if let Some(row) = effect {
                    self.row(&row);
                }
                self.ty(to);
            }
            Ty::Tuple(items) => {
                for t in self.ast.slice(items).to_vec() {
                    self.ty(t);
                }
            }
            Ty::Record { fields, .. } => {
                for f in self.ast.slice(fields).to_vec() {
                    self.ty(f.ty);
                }
            }
        }
    }

    fn row(&mut self, row: &EffectRow) {
        for entry in self.ast.slice(row.entries).to_vec() {
            if let Some(e) = entry.meta {
                self.expr(e);
            }
            for t in self.ast.slice(entry.args).to_vec() {
                self.ty(t);
            }
        }
    }

    fn fields(&mut self, fields: &[FieldInit]) {
        for f in fields {
            match f {
                FieldInit::Named { value, .. } | FieldInit::Positional(value) => self.expr(*value),
            }
        }
    }

    /// A constructor named `[module.][Type.]Tag` within `span`.
    fn constructor(&mut self, module: Option<StrId>, ty: Option<StrId>, span: Option<Span>) {
        match (module, ty) {
            (Some(m), _) => self.qualified(m, span),
            (None, Some(t)) => self.type_name(t, span),
            (None, None) => {}
        }
    }

    fn expr(&mut self, e: Aol<Expr>) {
        let span = self.ast.expr_span(e);
        match self.ast.expr(e).clone() {
            Expr::Int(_) | Expr::Real(_) | Expr::Str(_) | Expr::Bool(_) | Expr::Unit | Expr::Extern { .. } | Expr::Builtin => {}
            Expr::Var { module: None, name } => self.reference(name, span),
            Expr::Var { module: Some(m), .. } => self.qualified(m, span),
            Expr::App(f, x) => {
                self.expr(f);
                self.expr(x);
            }
            Expr::BinOp { lhs, rhs, .. } => {
                self.expr(lhs);
                self.expr(rhs);
            }
            Expr::UnOp { operand, .. } => self.expr(operand),
            // The default reading spells every hole the target does.
            Expr::Interp { default, .. } => self.expr(default),
            Expr::Splice(arg) => self.expr(arg),
            Expr::Tuple(items) | Expr::List(items) => {
                for it in self.ast.slice(items).to_vec() {
                    self.expr(it);
                }
            }
            Expr::Range { lo, hi } => {
                self.expr(lo);
                if let Some(hi) = hi {
                    self.expr(hi);
                }
            }
            Expr::Slice { recv, slots } => {
                self.expr(recv);
                for slot in self.ast.slice(slots).to_vec() {
                    match slot {
                        SliceSlot::Index(i) => self.expr(i),
                        SliceSlot::Range(lo, hi) => {
                            self.expr(lo);
                            self.expr(hi);
                        }
                        SliceSlot::Full => {}
                    }
                }
            }
            Expr::Array { size } => self.expr(size),
            Expr::Field { record, .. } => self.expr(record),
            Expr::StructLit { ty, fields, spread } => {
                if let Some(t) = ty {
                    self.type_name(t, span);
                }
                self.fields(&self.ast.slice(fields).to_vec());
                if let Some(s) = spread {
                    self.expr(s);
                }
            }
            Expr::Record { fields, extend, update } => {
                self.fields(&self.ast.slice(fields).to_vec());
                for e in [extend, update].into_iter().flatten() {
                    self.expr(e);
                }
            }
            Expr::Variant { module, ty, fields, .. } => {
                self.constructor(module, ty, span);
                self.fields(&self.ast.slice(fields).to_vec());
            }
            // A binding is recursive when its body names it, so its binders are
            // in scope in its value too.
            Expr::Let { binding, body } => {
                let mark = self.env.len();
                self.pattern(binding.pat);
                if let Some(t) = binding.sig {
                    self.ty(t);
                }
                self.expr(binding.value);
                self.expr(body);
                self.env.truncate(mark);
            }
            Expr::If { cond, then, alt } => {
                self.expr(cond);
                self.expr(then);
                self.expr(alt);
            }
            Expr::Match { scrut, arms, .. } => {
                self.expr(scrut);
                for arm in self.ast.slice(arms).to_vec() {
                    let mark = self.env.len();
                    for p in self.ast.slice(arm.patterns).to_vec() {
                        self.pattern(p);
                    }
                    if let Some(g) = arm.guard {
                        self.expr(g);
                    }
                    self.expr(arm.body);
                    self.env.truncate(mark);
                }
            }
            Expr::Lambda { params, body } => {
                let mark = self.env.len();
                for p in self.ast.slice(params).to_vec() {
                    if let Some(t) = p.sig {
                        self.ty(t);
                    }
                    self.pattern(p.pat);
                }
                self.expr(body);
                self.env.truncate(mark);
            }
            Expr::Handle { body, handler } => {
                self.expr(body);
                let Some(handler) = handler else { return };
                let mark = self.env.len();
                let after = self.ast.expr_span(body).map_or(0, |s| s.end);
                let ctl = span.and_then(|s| self.word_in(Some(Span::new(after, s.end)), StrId::intern("ctl")));
                let k = ctl.and_then(|c| {
                    let rest = &self.src[c + 3..];
                    let at = c + 3 + (rest.len() - rest.trim_start().len());
                    self.src[at..].starts_with(handler.continuation.as_str()).then_some(at)
                });
                self.bind(handler.continuation, k);
                for clause in self.ast.slice(handler.clauses).to_vec() {
                    let inner = self.env.len();
                    let at = self.binder_before(clause.body, clause.arg);
                    self.bind(clause.arg, at);
                    self.expr(clause.body);
                    self.env.truncate(inner);
                }
                self.env.truncate(mark);
                if let Some((binder, body)) = handler.value {
                    let at = self.binder_before(body, binder);
                    self.bind(binder, at);
                    self.expr(body);
                    self.env.truncate(mark);
                }
            }
            Expr::Ascribe { expr, ty } => {
                self.expr(expr);
                self.ty(ty);
            }
            Expr::UseArg(fields) => self.fields(&self.ast.slice(fields).to_vec()),
        }
    }

    /// Bring the binders of pattern `p` into scope.
    fn pattern(&mut self, p: Aol<Pattern>) {
        let span = self.ast.pat_span(p);
        match self.ast.pat(p).clone() {
            Pattern::Wild | Pattern::Int(_) | Pattern::Real(_) | Pattern::Str(_) | Pattern::Bool(_) => {}
            Pattern::Var(name) if is_synthetic(name) => self.bind(name, None),
            Pattern::Var(name) => {
                let at = self.word_in(span, name);
                self.bind(name, at);
            }
            Pattern::Range { lo, hi } => {
                self.pattern(lo);
                if let Some(hi) = hi {
                    self.pattern(hi);
                }
            }
            Pattern::Concat { pre, mid, suf } => {
                for q in self.ast.slice(pre).to_vec().into_iter().chain(mid).chain(self.ast.slice(suf).to_vec()) {
                    self.pattern(q);
                }
            }
            Pattern::List(items) | Pattern::Tuple(items) => {
                for q in self.ast.slice(items).to_vec() {
                    self.pattern(q);
                }
            }
            Pattern::Struct { ty, fields } => {
                self.type_name(ty, span);
                self.field_pats(span, &self.ast.slice(fields).to_vec());
            }
            Pattern::Record { fields, rest } => {
                self.field_pats(span, &self.ast.slice(fields).to_vec());
                if let Some(r) = rest {
                    self.pattern(r);
                }
            }
            Pattern::Variant { module, ty, fields, .. } => {
                self.constructor(module, ty, span);
                self.field_pats(span, &self.ast.slice(fields).to_vec());
            }
            Pattern::MetaE(e) => self.expr(e),
        }
    }

    fn field_pats(&mut self, span: Option<Span>, fields: &[FieldPat]) {
        let children: Vec<Span> = fields
            .iter()
            .filter_map(|f| match f {
                FieldPat::Named { pat, .. } | FieldPat::Positional(pat) => self.ast.pat_span(*pat),
                FieldPat::Shorthand(_) => None,
            })
            .collect();
        for f in fields {
            match f {
                FieldPat::Named { pat, .. } | FieldPat::Positional(pat) => self.pattern(*pat),
                FieldPat::Shorthand(name) => self.bind_shorthand(span, *name, &children),
            }
        }
    }
}

/// A name the compiler made (`#arg`, `%ret`) or an intrinsic (`@iadd`): no
/// quotation spells it.
fn is_synthetic(name: StrId) -> bool {
    name.as_str().starts_with(['@', '#', '%'])
}
