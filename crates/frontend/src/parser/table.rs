//! Binding-power tables for the Pratt (precedence-climbing) parser.
//!
//! Ported verbatim from the C++ `infix_db`. Each infix operator has a left and
//! right binding power `(l, r)`: a left-associative operator uses `l < r`, a
//! right-associative one uses `l > r`, so equal-precedence chains fold the right
//! way. Application (juxtaposition) binds tightest; unary `-`/`!` are prefix.

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

/// Infix operators keyed by lexeme; `None` means "not an infix operator".
/// Presence in this table is exactly the definition of "is infix".
pub fn infix(op: &str) -> Option<Bp> {
    let bp = match op {
        // Sequencing and pipes: loosest, so they bind looser than arithmetic,
        // comparison, and application. `;` and `<|` right-associative, `|>` left.
        ";" => bp(2, 1),
        "<|" => bp(5, 4),
        "|>" => bp(6, 7),
        // Short-circuit boolean, looser than comparison; `||` looser than `&&`.
        "||" => bp(8, 9),
        "&&" => bp(9, 10),
        // Comparison (all one precedence, left-associative).
        "?=" | "?>" | "?<" | "<=" | ">=" => bp(10, 11),
        // Cons: right-associative, looser than +/comparison.
        "::" => bp(15, 14),
        // Concatenation: left-associative, tighter than comparison.
        "++" => bp(16, 17),
        // Additive / multiplicative.
        "+" | "-" => bp(20, 21),
        "*" | "/" | "%" => bp(30, 31),
        _ => return None,
    };
    Some(bp)
}

/// The canonical name of a prefix operator, or `None` if `op` is not prefix.
/// The name is distinct from the lexeme so unary `-` never aliases binary `-`.
pub fn prefix(op: &str) -> Option<&'static str> {
    match op {
        "-" => Some("neg"),
        "!" => Some("not"),
        _ => None,
    }
}
