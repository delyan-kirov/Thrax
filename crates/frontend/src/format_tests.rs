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
        "$ @mod M\n$ go : Light -> @Int = \\l =\n    is l\n    | Light.Red => 0\n    | Light.Yellow => 1\n"
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
fn a_long_let_puts_each_binding_on_its_line() {
    let src = "$ @mod M\n$ f = \\x = let a = x + 1, b = a * 2 in a + b\n";
    assert_eq!(
        narrow(src, 24),
        "$ @mod M\n$ f = \\x =\n    let a = x + 1,\n        b = a * 2 in\n    a + b\n"
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
fn else_if_chains_stay_flat() {
    let src = "$ @mod M\n$ f = \\i = if i < 0 => 1 else if i == 0 => 2 # zero\n else 3\n";
    assert_eq!(
        narrow(src, 30),
        "$ @mod M\n$ f = \\i =\n    if i < 0 => 1\n    else if i == 0 => 2 # zero\n    else 3\n"
    );
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
fn a_long_signature_stays_on_one_line() {
    let src = "$ @mod M\n$ f : @Int -> {@Vec @Int, @Int} = \\x = {[x], x}\n";
    assert_eq!(narrow(src, 30), "$ @mod M\n$ f : @Int -> { @Vec @Int, @Int } = \\x =\n    { [x], x }\n");
}
