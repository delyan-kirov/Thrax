//! Project options: set by `@option` in a project's `BUILD.thx`, layered over
//! built-in defaults and under an editor's settings.
//!
//! Every option is one row of [`KEYS`]: its section, name, kind, default and
//! documentation. The table is the single source of truth: it validates an
//! `@option` (an unknown key gets a did-you-mean, a wrong value a typed error),
//! and it is what the language server accepts as settings. Adding an option is
//! adding a row and reading it.

use std::collections::HashMap;

use crate::error::closest;

#[derive(Clone, Debug, PartialEq)]
pub enum Value {
    Str(String),
    Int(i64),
    Bool(bool),
}

/// What an option accepts.
#[derive(Clone, Copy, Debug)]
pub enum Kind {
    Int { min: i64, max: i64 },
    Bool,
    /// One of a fixed set of words.
    Choice(&'static [&'static str]),
}

pub struct Key {
    pub section: &'static str,
    pub name: &'static str,
    pub kind: Kind,
    pub default: &'static str,
    pub doc: &'static str,
}

const LINT_LEVELS: &[&str] = &["allow", "warn", "deny"];

/// Every option. Defaults are written as an `@option` value would be.
pub const KEYS: &[Key] = &[
    Key { section: "format", name: "width", kind: Kind::Int { min: 40, max: 400 }, default: "100",
          doc: "The line width the formatter fits code into." },
    Key { section: "format", name: "indent", kind: Kind::Int { min: 1, max: 16 }, default: "4",
          doc: "Spaces per indentation level." },
    Key { section: "check", name: "max_errors", kind: Kind::Int { min: 1, max: 10_000 }, default: "20",
          doc: "The most errors `thrax check` prints." },
    Key { section: "lsp", name: "check", kind: Kind::Choice(&["type", "save"]), default: "type",
          doc: "When the editor checks: as you type (`type`), or only when a file is saved (`save`). \
                Compile-time code (`$ @e`) always runs on save only." },
    Key { section: "lsp", name: "focus", kind: Kind::Choice(&["all", "roots", "one"]), default: "roots",
          doc: "How loudly errors are shown. `all`: every error at full strength. `roots`: errors \
                that follow from an earlier one are quieter. `one`: only the most important error \
                per file is an error, the rest are warnings." },
    Key { section: "lsp", name: "debounce_ms", kind: Kind::Int { min: 0, max: 5_000 }, default: "150",
          doc: "How long typing must pause before the file is checked." },
    Key { section: "lsp", name: "grace", kind: Kind::Bool, default: "true",
          doc: "Hold back errors inside the definition being typed until typing pauses for \
                `grace_ms`, so half-written code is not flagged mid-word." },
    Key { section: "lsp", name: "grace_ms", kind: Kind::Int { min: 0, max: 60_000 }, default: "1500",
          doc: "The pause after which held-back errors are shown." },
    Key { section: "lsp", name: "max_diagnostics", kind: Kind::Int { min: 1, max: 10_000 }, default: "50",
          doc: "The most diagnostics shown per file." },
    Key { section: "lints", name: "unreachable_arm", kind: Kind::Choice(LINT_LEVELS), default: "warn",
          doc: "A match arm no value can reach." },
];

/// The settings in force: defaults, then each layer's overrides on top.
#[derive(Clone, Debug)]
pub struct Config {
    values: HashMap<(String, String), Value>,
}

impl Default for Config {
    fn default() -> Config {
        let mut values = HashMap::new();
        for k in KEYS {
            let v = parse(k, k.default).expect("defaults are well-formed");
            values.insert((k.section.to_string(), k.name.to_string()), v);
        }
        Config { values }
    }
}

impl Config {
    pub fn int(&self, section: &str, name: &str) -> i64 {
        match self.get(section, name) {
            Value::Int(n) => *n,
            other => panic!("option {section}.{name} is not an integer: {other:?}"),
        }
    }

    pub fn bool(&self, section: &str, name: &str) -> bool {
        match self.get(section, name) {
            Value::Bool(b) => *b,
            other => panic!("option {section}.{name} is not a boolean: {other:?}"),
        }
    }

    pub fn word(&self, section: &str, name: &str) -> &str {
        match self.get(section, name) {
            Value::Str(s) => s,
            other => panic!("option {section}.{name} is not a word: {other:?}"),
        }
    }

    fn get(&self, section: &str, name: &str) -> &Value {
        self.values
            .get(&(section.to_string(), name.to_string()))
            .unwrap_or_else(|| panic!("no option {section}.{name} in the key table"))
    }

    /// Set one option after validating it against [`KEYS`].
    pub fn set(&mut self, section: &str, name: &str, value: Value) -> Result<(), String> {
        let key = find_key(section, name).ok_or_else(|| format!("unknown option `{section}.{name}`"))?;
        check_kind(key, &value)?;
        self.values.insert((section.to_string(), name.to_string()), value);
        Ok(())
    }
}

/// An option as `@option` names it, `section.name`, with its value as text.
#[derive(Clone, Debug, PartialEq)]
pub struct Setting {
    pub section: &'static str,
    pub name: &'static str,
    pub value: Value,
}

/// Why an `@option` was refused: the message, and a did-you-mean when one fits.
#[derive(Debug)]
pub struct Refusal {
    pub msg: String,
    pub help: Option<String>,
}

/// Read `@option "section.name" "value"`, validated against [`KEYS`].
pub fn parse_setting(full: &str, text: &str) -> Result<Setting, Refusal> {
    let all = || KEYS.iter().map(|k| format!("{}.{}", k.section, k.name));
    let Some(key) = KEYS.iter().find(|k| format!("{}.{}", k.section, k.name) == full) else {
        let names: Vec<String> = all().collect();
        let help = closest(full, names.iter().map(String::as_str))
            .map(|near| format!("did you mean `{near}`?"))
            .or_else(|| {
                let name = full.rsplit('.').next().unwrap_or(full);
                KEYS.iter().find(|k| k.name == name).map(|k| format!("the option is `{}.{}`", k.section, k.name))
            });
        return Err(Refusal { msg: format!("unknown option `{full}`"), help });
    };
    let value = parse(key, text).map_err(|msg| Refusal { msg, help: Some(key.doc.to_string()) })?;
    Ok(Setting { section: key.section, name: key.name, value })
}

fn find_key(section: &str, name: &str) -> Option<&'static Key> {
    KEYS.iter().find(|k| k.section == section && k.name == name)
}

/// The value `text` gives `key`, in the kind it takes.
fn parse(key: &Key, text: &str) -> Result<Value, String> {
    let full = format!("{}.{}", key.section, key.name);
    let value = match key.kind {
        Kind::Int { .. } => text
            .replace('_', "")
            .parse::<i64>()
            .map(Value::Int)
            .map_err(|_| format!("`{full}` takes an integer, not {text:?}"))?,
        Kind::Bool => match text {
            "true" => Value::Bool(true),
            "false" => Value::Bool(false),
            _ => return Err(format!("`{full}` takes \"true\" or \"false\", not {text:?}")),
        },
        Kind::Choice(_) => Value::Str(text.to_string()),
    };
    check_kind(key, &value)?;
    Ok(value)
}

fn check_kind(key: &Key, value: &Value) -> Result<(), String> {
    let full = format!("{}.{}", key.section, key.name);
    match (key.kind, value) {
        (Kind::Int { min, max }, Value::Int(n)) if (min..=max).contains(n) => Ok(()),
        (Kind::Int { min, max }, Value::Int(n)) => Err(format!("`{full}` is {n}, outside {min} to {max}")),
        (Kind::Bool, Value::Bool(_)) => Ok(()),
        (Kind::Choice(words), Value::Str(s)) if words.contains(&s.as_str()) => Ok(()),
        (Kind::Choice(words), Value::Str(s)) => Err(format!(
            "`{full}` cannot be {s:?}; it is one of {}",
            words.iter().map(|w| format!("\"{w}\"")).collect::<Vec<_>>().join(", ")
        )),
        (Kind::Int { .. }, _) => Err(format!("`{full}` takes an integer")),
        (Kind::Bool, _) => Err(format!("`{full}` takes true or false")),
        (Kind::Choice(_), _) => Err(format!("`{full}` takes a word")),
    }
}

#[cfg(test)]
mod tests;
