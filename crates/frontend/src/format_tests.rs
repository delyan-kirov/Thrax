use super::*;

fn fmt(src: &str) -> String {
    format_source(src, &Style::default()).unwrap_or_else(|e| panic!("{e}\n{src}"))
}

fn narrow(src: &str, width: usize) -> String {
    format_source(src, &Style { width, indent: 4 }).unwrap_or_else(|e| panic!("{e}\n{src}"))
}

#[test]
fn spacing_is_normalized_within_a_line() {
    assert_eq!(fmt("$ @mod M\n$ x=1+2\n"), "$ @mod M\n$ x = 1 + 2\n");
    assert_eq!(
        fmt("$ @mod M\n$ h = \\l = is l | List.Cons.{h, t} => h | _ => 0\n"),
        "$ @mod M\n$ h = \\l = is l | List.Cons.{ h, t } => h | _ => 0\n"
    );
}

#[test]
fn a_long_match_breaks_its_arms_under_the_head() {
    let src = "$ @mod M\n$ go : Light -> @Int = \\l =\n\tis l | Light.Red => 0\n\t       | Light.Yellow => 1\n";
    let three = "$ @mod M\n$ go = \\l = is l | A => 0 | B => 1 | C => 2\n";
    assert_eq!(fmt(three), "$ @mod M\n$ go = \\l =\n    is l\n    | A => 0\n    | B => 1\n    | C => 2\n");
    assert_eq!(
        narrow(src, 40),
        "$ @mod M\n$ go : Light -> @Int = \\l =\n    is l\n    | Light.Red    => 0\n    | Light.Yellow => 1\n"
    );
    assert_eq!(fmt(src), "$ @mod M\n$ go : Light -> @Int = \\l = is l | Light.Red => 0 | Light.Yellow => 1\n");
}

#[test]
fn a_trailing_comma_keeps_a_declaration_exploded() {
    let src = "$ @mod M\n$ P : @struct =\n  x: @Int, y: @Int,\n$ Q : @struct = x: @Int, y: @Int,\n";
    assert_eq!(
        fmt(src),
        "$ @mod M\n$ P : @struct =\n    x: @Int,\n    y: @Int,\n$ Q : @struct = x: @Int, y: @Int,\n"
    );
}

#[test]
fn comments_survive_and_line_comments_end_their_line() {
    let src = "$ @mod M\n\n# lead\n$ f = \\x = # why\n  x + 1  # tail\n";
    assert_eq!(fmt(src), "$ @mod M\n\n# lead\n$ f = \\x = # why\n    x + 1 # tail\n");
}

#[test]
fn blank_lines_between_items_collapse_to_one() {
    assert_eq!(fmt("$ @mod M\n\n\n\n$ a = 1\n$ b = 2\n"), "$ @mod M\n\n$ a = 1\n$ b = 2\n");
}

#[test]
fn dots_and_unary_minus_keep_their_adjacency() {
    assert_eq!(fmt("$ @mod M\n$ x = f (-1) p.x t.0\n"), "$ @mod M\n$ x = f (-1) p.x t.0\n");
}

#[test]
fn several_bindings_make_a_let_block() {
    let src = "$ @mod M\n$ f = \\x = let a = x + 1, bb = a * 2 in a + bb\n";
    assert_eq!(
        fmt(src),
        "$ @mod M\n$ f = \\x =\n    let\n        a  = x + 1,\n        bb = a * 2\n    in\n        a + bb\n"
    );
    let defer = "$ @mod M\n$ f = \\x = defer g 1, g 2 in x\n";
    assert_eq!(fmt(defer), "$ @mod M\n$ f = \\x =\n    defer\n        g 1,\n        g 2\n    in\n        x\n");
    assert_eq!(fmt("$ @mod M\n$ f = \\x = let a = x in a\n"), "$ @mod M\n$ f = \\x = let a = x in a\n");
}

#[test]
fn a_binding_that_cannot_share_the_line_makes_a_let_block() {
    let src = "$ @mod M\n$ f = \\g = let h = do g {} ctl k | Y.y v => v | X.x u => 1 | _ => 0 in h\n";
    assert_eq!(
        fmt(src),
        "$ @mod M\n$ f = \\g =\n    let\n        h =\n            do g {}\n            ctl k\n            | Y.y v => v\n            | X.x u => 1\n            | _     => 0\n    in\n        h\n"
    );
}

#[test]
fn a_form_after_a_broken_in_or_else_goes_one_level_in() {
    let src = "$ @mod M\n$ f = \\x = let a = x, b = x in let c = a + b in c\n";
    assert_eq!(
        fmt(src),
        "$ @mod M\n$ f = \\x =\n    let\n        a = x,\n        b = x\n    in\n        let c = a + b in\n        c\n"
    );
    let els = "$ @mod M\n$ f = \\x = if x == 0 => 1 else let y = x * 2 in y + 1\n";
    assert_eq!(narrow(els, 30), "$ @mod M\n$ f = \\x =\n    if x == 0 =>\n        1\n    else\n        let y = x * 2 in y + 1\n");
}

#[test]
fn a_broken_let_chain_ends_every_line_with_in() {
    let src = "$ @mod M\n$ f = \\x = let a = x in let b = a in let c = b in a + b + c\n";
    assert_eq!(
        narrow(src, 40),
        "$ @mod M\n$ f = \\x =\n    let a = x in\n    let b = a in\n    let c = b in\n    a + b + c\n"
    );
}

#[test]
fn sequences_break_after_semicolons() {
    let src = "$ @mod M\n$ f = \\x = println \"a\"; println \"b\"; x\n";
    assert_eq!(narrow(src, 20), "$ @mod M\n$ f = \\x =\n    println \"a\";\n    println \"b\";\n    x\n");
}

#[test]
fn formatting_is_idempotent() {
    let src = "$ @mod M\n$ go : Light -> @Int = \\l =\n\tis l | Light.Red => 0\n\t       | Light.Yellow => if l == l => 1 else 2\n$ P : @struct =\n\tx: @Int,\n";
    let once = narrow(src, 30);
    assert_eq!(narrow(&once, 30), once);
}

#[test]
fn range_formatting_touches_only_the_window() {
    let src = "$ @mod M\n$ a=1\n$ b=2\n";
    let at = src.find("b=2").unwrap();
    let edits = format_edits(src, &Style::default(), Some((at, at))).unwrap();
    assert_eq!(edits.len(), 1);
    assert_eq!(edits[0].text, "$ b = 2");
}

#[test]
fn unlexable_input_is_refused() {
    assert!(format_source("$ @mod M\n$ s = \"open\n", &Style::default()).is_err());
}

#[test]
fn let_chains_end_each_line_with_in() {
    let src = "$ @mod M\n$ f = \\p =\n\tlet q = g p in\n\tlet i = h q in\n\tq + i\n";
    assert_eq!(narrow(src, 20), "$ @mod M\n$ f = \\p =\n    let q = g p in\n    let i = h q in\n    q + i\n");
}

#[test]
fn a_broken_if_puts_every_body_under_its_arrow_or_else() {
    let src = "$ @mod M\n$ f = \\i = if i < 0 => 1 else if i == 0 => 2 # zero\n else 3\n";
    assert_eq!(
        narrow(src, 30),
        "$ @mod M\n$ f = \\i =\n    if i < 0 =>\n        1\n    else if i == 0 =>\n        2 # zero\n    else\n        3\n"
    );
    assert_eq!(fmt("$ @mod M\n$ f = \\i = if i < 0 => 1 else 2\n"), "$ @mod M\n$ f = \\i = if i < 0 => 1 else 2\n");
}

#[test]
fn a_comment_block_keeps_its_blank_line_before_the_item() {
    let src = "$ @mod M\n# about\n\n$ with OPT\n";
    assert_eq!(fmt(src), src);
}

#[test]
fn a_handler_puts_ctl_on_its_own_line() {
    let src = "$ @mod M\n$ s = \\g = do g {} ctl k | Y.y v => v + k {} | _ => 0\n";
    assert_eq!(fmt(src), "$ @mod M\n$ s = \\g =\n    do g {}\n    ctl k | Y.y v => v + k {} | _ => 0\n");
}

#[test]
fn a_long_concatenation_breaks_before_each_plus_plus() {
    let src = "$ @mod M\n$ s : @Str = \"aaaaaaaaaa\" ++ \"bbbbbbbbbb\" ++ \"cccccccccc\"\n";
    assert_eq!(
        narrow(src, 30),
        "$ @mod M\n$ s : @Str =\n    \"aaaaaaaaaa\"\n        ++ \"bbbbbbbbbb\"\n        ++ \"cccccccccc\"\n"
    );
}

#[test]
fn a_long_signature_wraps_before_its_arrows() {
    let src = "$ @mod M\n$ f : @Int -> {@Vec @Int, @Int} = \\x = x\n";
    assert_eq!(narrow(src, 36), "$ @mod M\n$ f : @Int\n    -> { @Vec @Int, @Int } = \\x =\n    x\n");
}

#[test]
fn a_long_sum_packs_its_operands() {
    let src = "$ @mod M\n$ t : @Int = aaaa + bbbb + cccc + dddd + eeee\n";
    assert_eq!(narrow(src, 30), "$ @mod M\n$ t : @Int =\n    aaaa + bbbb + cccc + dddd\n        + eeee\n");
}

#[test]
fn long_alternatives_wrap_one_level_in() {
    let src = "$ @mod M\n$ k = \\s = is s | \"aaaa\" | \"bbbb\" | \"cccc\" | \"dddd\" => 1 | _ => 0\n";
    assert_eq!(
        narrow(src, 30),
        "$ @mod M\n$ k = \\s =\n    is s\n    | \"aaaa\" | \"bbbb\" | \"cccc\"\n        | \"dddd\" =>\n        1\n    | _ => 0\n"
    );
}

#[test]
fn a_broken_paren_nests_its_contents() {
    let src = "$ @mod M\n$ f = \\x = is x | 0 => (let a = x in let b = a in { a, b, a, b }) | _ => (x)\n";
    assert_eq!(
        narrow(src, 30),
        "$ @mod M\n$ f = \\x =\n    is x\n    | 0 => (\n        let a = x in\n        let b = a in\n        { a, b, a, b }\n    )\n    | _ => (x)\n"
    );
}

#[test]
fn a_braced_body_opens_on_its_heads_line() {
    let src = "$ @mod M\n$ t = { aaaaaaaaaa, bbbbbbbbbb, cccccccccc }\n";
    assert_eq!(narrow(src, 30), "$ @mod M\n$ t = {\n    aaaaaaaaaa,\n    bbbbbbbbbb,\n    cccccccccc\n}\n");
}

#[test]
fn an_is_inside_a_handler_clause_ends_at_the_next_clause() {
    let src = "$ @mod M\n$ h = \\g = do g {} ctl k | Y.y v => (is v | 0 => 1 | _ => 2) | X.x u => is u | 0 => 1 | _ => 2 | Z.z w => 3\n";
    assert_eq!(
        fmt(src),
        "$ @mod M\n$ h = \\g =\n    do g {}\n    ctl k\n    | Y.y v => (is v | 0 => 1 | _ => 2)\n    | X.x u => is u | 0 => 1 | _ => 2\n    | Z.z w => 3\n"
    );
}

#[test]
fn trailing_comments_of_neighbouring_lines_line_up() {
    let src = "$ @mod M\n$ a = 1 # one\n$ bbb = 22 # two\n\n$ c = 3 # alone\n";
    assert_eq!(fmt(src), "$ @mod M\n$ a = 1    # one\n$ bbb = 22 # two\n\n$ c = 3 # alone\n");
}

#[test]
fn comma_arms_make_an_if_block() {
    let src = "$ @mod M\n$ f = \\i = if i < 0 => 1, i == 0 => 2 else 3\n";
    assert_eq!(fmt(src), "$ @mod M\n$ f = \\i =\n    if\n        i < 0  => 1,\n        i == 0 => 2\n        else 3\n");
    let chain = "$ @mod M\n$ f = \\i = if i < 0 => 1 else if i == 0 => 2, i == 1 => 3 else 4\n";
    assert_eq!(
        fmt(chain),
        "$ @mod M\n$ f = \\i =\n    if i < 0 =>\n        1\n    else if\n        i == 0 => 2,\n        i == 1 => 3\n        else 4\n"
    );
    assert_eq!(fmt(&fmt(chain)), fmt(chain));
}

#[test]
fn a_long_condition_never_pushes_else_or_its_body_right() {
    let src = "$ @mod M\n$ f = \\c = if a => b, at_p \"*\" && is_punct (peek_at 1) \"]\" => d else let e = rv (assign c) in expect \"]\"; g e\n";
    assert_eq!(
        fmt(src),
        "$ @mod M\n$ f = \\c =\n    if\n        a                                    => b,\n        at_p \"*\" && is_punct (peek_at 1) \"]\" => d\n        else let e = rv (assign c) in expect \"]\"; g e\n"
    );
}

#[test]
fn if_arms_that_cannot_line_up_all_break_under_their_arrows() {
    let src = "$ @mod M\n$ f = \\i = if i < 0 => 1, i == 0 => g iiiiiiiiii jjjjjjjjjj else 3\n";
    assert_eq!(
        narrow(src, 40),
        "$ @mod M\n$ f = \\i =\n    if\n        i < 0 =>\n            1,\n        i == 0 =>\n            g iiiiiiiiii jjjjjjjjjj\n        else\n            3\n"
    );
}

#[test]
fn a_trailing_lambda_opens_on_the_calls_line() {
    let src = "$ @mod M\n$ f = \\s = let r = with_a (\\u = with_b s (\\v = with_c (\\w = go s u v w))) in r\n";
    assert_eq!(
        narrow(src, 40),
        "$ @mod M\n$ f = \\s =\n    let\n        r = with_a (\\u =\n            with_b s (\\v =\n                with_c (\\w =\n                    go s u v w)))\n    in\n        r\n"
    );
}

#[test]
fn an_import_list_keeps_one_line_or_goes_one_entry_per_line() {
    assert_eq!(fmt("$ @mod M\n$ with V=VEC\n"), "$ @mod M\n$ with V = VEC\n");
    assert_eq!(fmt("$ @mod M\n$ with VEC ,M=MAP\n"), "$ @mod M\n$ with VEC, M = MAP\n");
    let exploded = "$ @mod M\n$ with\n    VEC,\n    M = MAP,\n    FOO,\n";
    assert_eq!(fmt("$ @mod M\n$ with VEC,\n  M = MAP, FOO,\n"), exploded);
    assert_eq!(fmt(exploded), exploded);
    assert_eq!(
        narrow("$ @mod M\n$ with ALPHA, BETA = GAMMA\n", 20),
        "$ @mod M\n$ with\n    ALPHA,\n    BETA = GAMMA\n"
    );
    let commented = "$ @mod M\n$ with\n    # the vectors\n    V = VEC, # qualified\n    LIST,\n";
    assert_eq!(fmt(commented), commented);
}
