use super::data::builtin_arity;

/// Every `@extern "thrax"` declaration in CORE names a builtin the machine
/// runs, so a misspelled declaration fails here rather than when it is called.
/// The operators the parser rewrites away are declared only for their types.
#[test]
fn core_builtins_are_known_to_the_machine() {
    let src = include_str!("../../../../library/CORE.thx");
    let parsed = frontend::parse(src).expect("parse CORE");
    let ast = &parsed.ast;
    let mut seen = 0;
    for item in ast.slice(parsed.program.items).iter() {
        if let frontend::parser::data::Item::Def { name, body, .. } = item {
            if matches!(ast.expr(*body), frontend::parser::data::Expr::Builtin) {
                let name = ast.text(*name);
                if matches!(name, ";" | "|>" | "<|") {
                    continue;
                }
                assert!(builtin_arity(name).is_some(), "CORE declares unknown builtin `{name}`");
                seen += 1;
            }
        }
    }
    assert!(seen > 0, "CORE declares no builtins");
}
