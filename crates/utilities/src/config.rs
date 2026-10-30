//! Project configuration: `thrax.toml`, layered over built-in defaults and
//! under an editor's settings.
//!
//! Every option is one row of [`KEYS`]: its section, name, kind, default and
//! documentation. The table is the single source of truth: it validates a file
//! (an unknown key gets a did-you-mean fix, a wrong kind a typed error), it
//! documents the options (`thrax config`), and it is what the language server
//! accepts as settings. Adding an option is adding a row and reading it.
//!
//! The file format is a TOML subset: `[section]` headers, `key = value` lines
//! with strings, integers, booleans or arrays of strings, and `#` comments.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use crate::error::{closest, Applicability, Code, Diagnostic, Edit, Span};

/// The name of the project file, looked for in the file's directory and then
/// each parent.
pub const FILE_NAME: &str = "thrax.toml";

#[derive(Clone, Debug, PartialEq)]
pub enum Value {
    Str(String),
    Int(i64),
    Bool(bool),
    List(Vec<String>),
}

impl Value {
    fn kind(&self) -> &'static str {
        match self {
            Value::Str(_) => "a string",
            Value::Int(_) => "an integer",
            Value::Bool(_) => "a boolean",
            Value::List(_) => "a list of strings",
        }
    }

    fn show(&self) -> String {
        match self {
            Value::Str(s) => format!("{s:?}"),
            Value::Int(n) => n.to_string(),
            Value::Bool(b) => b.to_string(),
            Value::List(xs) => format!("[{}]", xs.iter().map(|x| format!("{x:?}")).collect::<Vec<_>>().join(", ")),
        }
    }
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

/// Every option. Defaults are written as they would be in the file.
pub const KEYS: &[Key] = &[
    Key { section: "format", name: "width", kind: Kind::Int { min: 40, max: 400 }, default: "100",
          doc: "The line width the formatter fits code into." },
    Key { section: "format", name: "indent", kind: Kind::Int { min: 1, max: 16 }, default: "4",
          doc: "Spaces per indentation level." },
    Key { section: "check", name: "max_errors", kind: Kind::Int { min: 1, max: 10_000 }, default: "20",
          doc: "The most errors `thrax check` prints." },
    Key { section: "lsp", name: "check", kind: Kind::Choice(&["type", "save"]), default: "\"type\"",
          doc: "When the editor checks: as you type (`type`), or only when a file is saved (`save`). \
                Compile-time code (`$ @e`) always runs on save only." },
    Key { section: "lsp", name: "focus", kind: Kind::Choice(&["all", "roots", "one"]), default: "\"roots\"",
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
    Key { section: "lints", name: "unreachable_arm", kind: Kind::Choice(LINT_LEVELS), default: "\"warn\"",
          doc: "A match arm no value can reach." },
];

/// The settings in force: defaults, then each layer's overrides on top.
#[derive(Clone, Debug)]
pub struct Config {
    values: HashMap<(String, String), Value>,
    /// The files that contributed, nearest last.
    pub sources: Vec<PathBuf>,
}

impl Default for Config {
    fn default() -> Config {
        let mut values = HashMap::new();
        for k in KEYS {
            let v = parse_value(k.default).expect("defaults are well-formed");
            values.insert((k.section.to_string(), k.name.to_string()), v);
        }
        Config { values, sources: Vec::new() }
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

    /// Layer a `thrax.toml`'s text over this config. Returns the problems found,
    /// each a diagnostic against `text`; a bad line is skipped, the rest apply.
    pub fn apply_toml(&mut self, text: &str) -> Vec<Diagnostic> {
        let mut problems = Vec::new();
        let mut section = String::new();
        let mut offset = 0;
        for (n, raw) in text.split_inclusive('\n').enumerate() {
            let line_no = n as u32 + 1;
            let start = offset;
            offset += raw.len();
            let line = strip_comment(raw).trim();
            if line.is_empty() {
                continue;
            }
            let lead = raw.len() - raw.trim_start().len();
            let span = Span::new(start + lead, start + lead + line.len());
            if let Some(name) = line.strip_prefix('[').and_then(|l| l.strip_suffix(']')) {
                let name = name.trim();
                if !KEYS.iter().any(|k| k.section == name) {
                    let mut d = Diagnostic::error(Code::UnknownSymbol, span, line_no, format!("unknown section `[{name}]`"));
                    if let Some(near) = closest(name, sections()) {
                        d = d.with_fix(format!("did you mean `[{near}]`?"), vec![Edit::new(span, format!("[{near}]"))], Applicability::MaybeIncorrect);
                    }
                    problems.push(d);
                }
                section = name.to_string();
                continue;
            }
            let Some((key, value)) = line.split_once('=') else {
                problems.push(Diagnostic::error(Code::UnexpectedToken, span, line_no, "expected `key = value` or a `[section]`"));
                continue;
            };
            let key = key.trim();
            let key_start = start + lead;
            let key_span = Span::new(key_start, key_start + key.len());
            let Some(k) = find_key(&section, key) else {
                let mut d = Diagnostic::error(
                    Code::UnknownSymbol,
                    key_span,
                    line_no,
                    format!("unknown option `{key}` in `[{section}]`"),
                );
                let here = KEYS.iter().filter(|k| k.section == section).map(|k| k.name);
                if let Some(near) = closest(key, here) {
                    d = d.with_fix(format!("did you mean `{near}`?"), vec![Edit::new(key_span, near)], Applicability::MaybeIncorrect);
                } else if let Some(home) = KEYS.iter().find(|k| k.name == key) {
                    d = d.with_help(format!("`{key}` is an option of `[{}]`", home.section));
                }
                problems.push(d);
                continue;
            };
            let value = match parse_value(value.trim()) {
                Ok(v) => v,
                Err(msg) => {
                    problems.push(Diagnostic::error(Code::UnexpectedToken, span, line_no, msg));
                    continue;
                }
            };
            if let Err(msg) = check_kind(k, &value) {
                problems.push(Diagnostic::error(Code::TypeMismatch, span, line_no, msg).with_note(k.doc));
                continue;
            }
            self.values.insert((section.clone(), key.to_string()), value);
        }
        problems
    }

    /// The config for a file at `path`: defaults, then the nearest `thrax.toml`
    /// above it. Problems in that file come back rendered, for the caller to
    /// report; they never stop the load.
    pub fn for_path(path: &Path) -> (Config, Vec<String>) {
        let mut config = Config::default();
        let mut problems = Vec::new();
        if let Some(file) = find_file(path) {
            if let Ok(text) = std::fs::read_to_string(&file) {
                let shown = file.display().to_string();
                problems = config.apply_toml(&text).iter().map(|d| d.render(&text, &shown)).collect();
                config.sources.push(file);
            }
        }
        (config, problems)
    }
}

/// The nearest `thrax.toml` in `start`'s directory or above.
pub fn find_file(start: &Path) -> Option<PathBuf> {
    let dir = if start.is_dir() { start } else { start.parent()? };
    dir.ancestors().map(|d| d.join(FILE_NAME)).find(|f| f.is_file())
}

/// The option table as documentation, one block per section.
pub fn document() -> String {
    let mut out = String::new();
    let mut last = "";
    for k in KEYS {
        if k.section != last {
            out.push_str(&format!("[{}]\n", k.section));
            last = k.section;
        }
        let accepts = match k.kind {
            Kind::Int { min, max } => format!("an integer from {min} to {max}"),
            Kind::Bool => "true or false".to_string(),
            Kind::Choice(words) => words.iter().map(|w| format!("\"{w}\"")).collect::<Vec<_>>().join(", "),
        };
        out.push_str(&format!("{} = {}    # {accepts}\n    # {}\n", k.name, k.default, k.doc));
    }
    out
}

fn sections() -> impl Iterator<Item = &'static str> {
    let mut seen: Vec<&'static str> = Vec::new();
    for k in KEYS {
        if !seen.contains(&k.section) {
            seen.push(k.section);
        }
    }
    seen.into_iter()
}

fn find_key(section: &str, name: &str) -> Option<&'static Key> {
    KEYS.iter().find(|k| k.section == section && k.name == name)
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
        (Kind::Int { .. }, v) => Err(format!("`{full}` takes an integer, not {}", v.kind())),
        (Kind::Bool, v) => Err(format!("`{full}` takes true or false, not {}", v.kind())),
        (Kind::Choice(_), v) => Err(format!("`{full}` takes a quoted word, not {} ({})", v.kind(), v.show())),
    }
}

/// A `#` outside a string starts a comment.
fn strip_comment(line: &str) -> &str {
    let mut in_str = false;
    for (i, c) in line.char_indices() {
        match c {
            '"' => in_str = !in_str,
            '#' if !in_str => return &line[..i],
            _ => {}
        }
    }
    line
}

fn parse_value(text: &str) -> Result<Value, String> {
    match text {
        "true" => return Ok(Value::Bool(true)),
        "false" => return Ok(Value::Bool(false)),
        _ => {}
    }
    if let Some(s) = parse_string(text) {
        return Ok(Value::Str(s));
    }
    if let Some(inner) = text.strip_prefix('[').and_then(|t| t.strip_suffix(']')) {
        return inner
            .split(',')
            .map(str::trim)
            .filter(|p| !p.is_empty())
            .map(|p| parse_string(p).ok_or_else(|| format!("a list holds quoted strings, not `{p}`")))
            .collect::<Result<Vec<_>, _>>()
            .map(Value::List);
    }
    text.replace('_', "")
        .parse::<i64>()
        .map(Value::Int)
        .map_err(|_| format!("`{text}` is not a value: write a number, true/false, a \"string\", or a [list]"))
}

fn parse_string(text: &str) -> Option<String> {
    let inner = text.strip_prefix('"')?.strip_suffix('"')?;
    (!inner.contains('"')).then(|| inner.replace("\\\\", "\\"))
}

#[cfg(test)]
mod tests;
