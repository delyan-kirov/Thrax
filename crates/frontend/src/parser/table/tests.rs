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
