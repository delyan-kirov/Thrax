use super::*;

#[test]
fn defaults_come_from_the_table() {
    let c = Config::default();
    assert_eq!(c.int("format", "width"), 100);
    assert_eq!(c.word("lsp", "focus"), "roots");
    assert!(c.bool("lsp", "grace"));
}

#[test]
fn a_setting_reads_in_its_kind() {
    assert_eq!(parse_setting("format.width", "80").unwrap().value, Value::Int(80));
    assert_eq!(parse_setting("lsp.check", "save").unwrap().value, Value::Str("save".into()));
    assert_eq!(parse_setting("lsp.grace", "false").unwrap().value, Value::Bool(false));
}

#[test]
fn an_unknown_key_suggests_the_near_one() {
    let r = parse_setting("format.widht", "80").unwrap_err();
    assert_eq!(r.help.as_deref(), Some("did you mean `format.width`?"));
}

#[test]
fn a_key_in_the_wrong_section_names_its_section() {
    let r = parse_setting("format.debounce_ms", "10").unwrap_err();
    assert!(r.help.as_deref().unwrap_or("").contains("`lsp.debounce_ms`"), "{r:?}");
    let r = parse_setting("x.max_diagnostics", "10").unwrap_err();
    assert!(r.help.as_deref().unwrap_or("").contains("`lsp.max_diagnostics`"), "{r:?}");
}

#[test]
fn values_are_checked_against_their_kind() {
    let msg = |k, v| parse_setting(k, v).unwrap_err().msg;
    assert!(msg("lsp.focus", "loud").contains("one of \"all\", \"roots\", \"one\""));
    assert!(msg("lsp.debounce_ms", "true").contains("takes an integer"));
    assert!(msg("format.width", "5").contains("outside 40 to 400"));
    assert!(msg("lsp.grace", "yes").contains("\"true\" or \"false\""));
}

#[test]
fn set_validates_too() {
    let mut c = Config::default();
    assert!(c.set("lsp", "grace", Value::Bool(false)).is_ok());
    assert!(!c.bool("lsp", "grace"));
    assert!(c.set("lsp", "nope", Value::Bool(false)).is_err());
}

#[test]
fn every_option_is_documented() {
    let reference = include_str!("../../../../documentation/language-reference.md");
    for k in KEYS {
        let full = format!("`{}.{}`", k.section, k.name);
        assert!(reference.contains(&full), "{full} is missing from the language reference");
    }
}
