use super::*;

#[test]
fn defaults_come_from_the_table() {
    let c = Config::default();
    assert_eq!(c.int("format", "width"), 100);
    assert_eq!(c.word("lsp", "focus"), "roots");
    assert!(c.bool("lsp", "grace"));
}

#[test]
fn a_file_overrides_defaults() {
    let mut c = Config::default();
    let problems = c.apply_toml("# project\n[format]\nwidth = 80 # narrow\n\n[lsp]\ncheck = \"save\"\n");
    assert!(problems.is_empty(), "{problems:?}");
    assert_eq!(c.int("format", "width"), 80);
    assert_eq!(c.word("lsp", "check"), "save");
}

#[test]
fn an_unknown_key_suggests_the_near_one_and_the_rest_still_apply() {
    let mut c = Config::default();
    let text = "[format]\nwidht = 80\nindent = 2\n";
    let problems = c.apply_toml(text);
    assert_eq!(problems.len(), 1);
    let fix = &problems[0].helps()[0];
    assert_eq!(fix.msg, "did you mean `width`?");
    assert_eq!(apply_edits_for_test(text, fix), "[format]\nwidth = 80\nindent = 2\n");
    assert_eq!(c.int("format", "indent"), 2);
    assert_eq!(c.int("format", "width"), 100);
}

#[test]
fn a_key_in_the_wrong_section_names_its_section() {
    let mut c = Config::default();
    let problems = c.apply_toml("[format]\ndebounce_ms = 10\n");
    assert!(problems[0].helps()[0].msg.contains("`[lsp]`"), "{:?}", problems[0]);
}

#[test]
fn values_are_checked_against_their_kind() {
    let mut c = Config::default();
    let problems = c.apply_toml("[lsp]\nfocus = \"loud\"\ndebounce_ms = true\n[format]\nwidth = 5\n");
    let msgs: Vec<&str> = problems.iter().map(|d| d.root().msg.as_str()).collect();
    assert_eq!(msgs.len(), 3, "{msgs:?}");
    assert!(msgs[0].contains("one of \"all\", \"roots\", \"one\""), "{msgs:?}");
    assert!(msgs[1].contains("takes an integer"), "{msgs:?}");
    assert!(msgs[2].contains("outside 40 to 400"), "{msgs:?}");
}

#[test]
fn set_validates_too() {
    let mut c = Config::default();
    assert!(c.set("lsp", "grace", Value::Bool(false)).is_ok());
    assert!(!c.bool("lsp", "grace"));
    assert!(c.set("lsp", "nope", Value::Bool(false)).is_err());
}

#[test]
fn every_default_is_valid_for_its_kind() {
    for k in KEYS {
        check_kind(k, &parse_value(k.default).unwrap()).unwrap();
    }
}

fn apply_edits_for_test(text: &str, fix: &crate::error::Help) -> String {
    crate::error::apply_edits(text, &fix.edits)
}
