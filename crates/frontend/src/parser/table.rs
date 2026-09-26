//! Binding-power tables for the Pratt (precedence-climbing) parser.
//!
//! The per-operator binding powers live in [`crate::lexer::data::OPERATORS`],
//! the one table shared with the lexer, so an operator cannot lex yet be unknown
//! here. This module reads that table and adds only the fixed binding powers of
//! application, the `@ctx` override, and the unary prefixes. A left-associative
//! operator uses `left < right`, a right-associative one `left > right`.

use crate::lexer::data::{op_role, OpRole};

/// A left/right binding-power pair for an infix operator.
#[derive(Clone, Copy, Debug)]
pub struct Bp {
    pub left: u8,
    pub right: u8,
}

const fn bp(left: u8, right: u8) -> Bp {
    Bp { left, right }
}

/// Application by juxtaposition, the tightest binary form.
pub const APP: Bp = bp(50, 51);

/// Binding power of the postfix `@ctx` override: tighter than every binary
/// operator, looser than application, so `f a @ctx c` reads as `(f a) @ctx c`.
pub const CTX: Bp = bp(45, 46);

/// Binding power of the unary prefix operators (`-`, `!`).
pub const PREFIX: u8 = 40;

/// Binding power of `op` as an infix operator, or `None` if it is not infix.
pub fn infix(op: &str) -> Option<Bp> {
    match op_role(op)? {
        OpRole::Infix(l, r) | OpRole::InfixPrefix(l, r) => Some(bp(l, r)),
        _ => None,
    }
}

/// The canonical name of a prefix operator, or `None` if `op` is not prefix.
/// The name is distinct from the lexeme so unary `-` never aliases binary `-`.
pub fn prefix(op: &str) -> Option<&'static str> {
    match op_role(op)? {
        OpRole::Prefix | OpRole::InfixPrefix(..) => match op {
            "-" => Some("neg"),
            "!" => Some("not"),
            _ => None,
        },
        _ => None,
    }
}

/// True if `op` is an operator lexeme that legitimately ends an expression: a
/// grammatical delimiter (`|`, `<>`) that a surrounding construct consumes. Lets
/// the infix loop tell "the expression ended here" apart from "this operator has
/// no infix meaning here" (a real error).
pub fn ends_expr(op: &str) -> bool {
    matches!(op_role(op), Some(OpRole::Delimiter))
}

/// True if `a op b` is rewritten into some other form (a lazy `if`, a `let`, a
/// bare application, a lambda) instead of a call to a global named `op`. Such an
/// operator has no binding of its own, so `(op)` eta-expands to `\l r = l op r` and
/// reuses the rewrite, unless [`infix_global`] names a binding for it.
pub fn desugared(op: &str) -> bool {
    matches!(op, "&&" | "||" | ";" | "|>" | "<|" | "<|>")
}

/// The global an infix operator's FUNCTION form names, when no binding is spelled
/// like the operator: `(<|>)` is CORE's `compose`, since the infix form expands to
/// a lambda. The same indirection prefix `!` uses to name `not`.
pub fn infix_global(op: &str) -> Option<&'static str> {
    match op {
        "<|>" => Some("compose"),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lexer::data::{Kind, OPERATORS};

    /// Every operator that lexes as [`Kind::Op`] must have a parser role the
    /// infix loop understands (infix, prefix, or delimiter). If this fails, an
    /// operator was added to `lexer::data::OPERATORS` without a usable role, and
    /// it would silently drop out of expression parsing.
    #[test]
    fn every_op_token_has_a_parse_role() {
        for d in OPERATORS {
            if d.kind() != Kind::Op {
                continue;
            }
            let classified =
                infix(d.lexeme).is_some() || prefix(d.lexeme).is_some() || ends_expr(d.lexeme);
            assert!(
                classified,
                "operator `{}` lexes as Kind::Op but has no parser role",
                d.lexeme
            );
        }
    }

    /// Every lexeme [`desugared`] names must still be an infix operator. If this
    /// fails, the operator was renamed or dropped from
    /// `lexer::data::OPERATORS` and `(op)` would reference a global that the
    /// desugar never emits.
    #[test]
    fn every_desugared_op_is_infix() {
        for d in OPERATORS {
            assert!(
                !desugared(d.lexeme) || infix(d.lexeme).is_some(),
                "operator `{}` is listed as desugared but is not infix",
                d.lexeme
            );
        }
        for op in ["&&", "||", ";", "|>", "<|", "<|>"] {
            assert!(desugared(op), "`{op}` should be listed as desugared");
            assert!(infix(op).is_some(), "`{op}` is no longer an infix operator");
        }
    }
}
