//! The negative suite: programs the compiler must REJECT (or warn about), each
//! pinned to the diagnostics it has to produce.
//!
//! Every case under `tests/diagnostics/` is a program that `thrax check` is run
//! on. A case is either one `NAME.thx` file, or a directory `NAME/` whose root is
//! `NAME/MAIN.thx` (for diagnostics that need several modules). Its expected
//! diagnostics are written inline as `#~` comments:
//!
//! ```text
//! $ p : P = P.{ 1 }   #~ error[TYPE_MISMATCH]: literal is missing field `y`
//! $ s : @Str = "abc
//! #~^ error[QUOTM_UNCLOSED]
//! #~ error: must have the signature
//! ```
//!
//! * A trailing `#~` pins its own line. `#~^` pins the line above, `#~^^` the
//!   one above that, and so on (for a line that cannot carry a comment, like an
//!   unclosed string).
//! * A `#~` alone on its line (no carets) is an UNLOCATED diagnostic: one with
//!   no line in this case's files.
//! * `error[CODE]` / `warning[CODE]` name the severity and the code. A bare
//!   `error` (no code) matches the driver's uncoded `thrax: ...` errors.
//! * Text after `:` must occur in the diagnostic's message (a substring, so a
//!   reworded aside or a moved column does not break the case).
//!
//! The match is exact both ways: every annotation needs a diagnostic, and every
//! diagnostic needs an annotation. A case with no annotations is rejected (it
//! would pin nothing), and a crash (a panic, a signal) always fails the case.

use std::path::{Path, PathBuf};
use std::process::Command;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Severity {
    Error,
    Warning,
}

impl Severity {
    fn word(self) -> &'static str {
        match self {
            Severity::Error => "error",
            Severity::Warning => "warning",
        }
    }
}

/// Where a diagnostic points: a line of one of the case's files, or nowhere.
#[derive(Clone, PartialEq, Eq, Debug)]
enum Site {
    Line { file: String, line: usize },
    Unlocated,
}

impl std::fmt::Display for Site {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Site::Line { file, line } => write!(f, "{file}:{line}"),
            Site::Unlocated => f.write_str("<unlocated>"),
        }
    }
}

#[derive(Debug)]
struct Expected {
    severity: Severity,
    code: Option<String>,
    text: Option<String>,
    site: Site,
}

impl std::fmt::Display for Expected {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.site, self.severity.word())?;
        if let Some(code) = &self.code {
            write!(f, "[{code}]")?;
        }
        if let Some(text) = &self.text {
            write!(f, ": {text}")?;
        }
        Ok(())
    }
}

#[derive(Debug)]
struct Actual {
    severity: Severity,
    code: Option<String>,
    msg: String,
    site: Site,
}

impl std::fmt::Display for Actual {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.site, self.severity.word())?;
        if let Some(code) = &self.code {
            write!(f, "[{code}]")?;
        }
        write!(f, ": {}", self.msg)
    }
}

impl Expected {
    fn matches(&self, a: &Actual) -> bool {
        self.severity == a.severity
            && self.code == a.code
            && self.site == a.site
            && self.text.as_ref().is_none_or(|t| a.msg.contains(t.as_str()))
    }
}

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn file_name(path: &Path) -> String {
    path.file_name().unwrap().to_string_lossy().into_owned()
}

/// Parse the `#~` annotations of one source file.
fn annotations(file: &str, src: &str) -> Result<Vec<Expected>, String> {
    let mut out = Vec::new();
    for (i, line) in src.lines().enumerate() {
        let Some(at) = line.find("#~") else {
            continue;
        };
        let lineno = i + 1;
        let own_line = line[..at].trim().is_empty();
        let rest = &line[at + 2..];
        let carets = rest.len() - rest.trim_start_matches('^').len();
        let spec = rest[carets..].trim();
        let site = if carets > 0 {
            if carets >= lineno {
                return Err(format!("{file}:{lineno}: `#~` points above the first line"));
            }
            Site::Line { file: file.to_string(), line: lineno - carets }
        } else if own_line {
            Site::Unlocated
        } else {
            Site::Line { file: file.to_string(), line: lineno }
        };
        let bad = || format!("{file}:{lineno}: malformed annotation `#~{rest}`");
        let (head, text) = match spec.split_once(':') {
            Some((h, t)) => (h.trim(), Some(t.trim().to_string()).filter(|t| !t.is_empty())),
            None => (spec, None),
        };
        let (word, code) = match head.split_once('[') {
            Some((w, c)) => {
                let c = c.strip_suffix(']').ok_or_else(bad)?;
                (w, Some(c.to_string()))
            }
            None => (head, None),
        };
        let severity = match word {
            "error" => Severity::Error,
            "warning" => Severity::Warning,
            _ => return Err(bad()),
        };
        out.push(Expected { severity, code, text, site });
    }
    Ok(out)
}

/// Read the diagnostics back out of `thrax check`'s stderr: the rendered
/// `error[CODE]: msg` / `warning[CODE]: msg` blocks, plus the driver's uncoded
/// `thrax: msg` errors. A `thrax: ...:` line ending in a colon is a header for
/// the block that follows, not a diagnostic of its own.
fn diagnostics(stderr: &str, files: &[String]) -> Vec<Actual> {
    let lines: Vec<&str> = stderr.lines().collect();
    let mut out = Vec::new();
    for (i, line) in lines.iter().enumerate() {
        let coded = [("error[", Severity::Error), ("warning[", Severity::Warning)]
            .into_iter()
            .find_map(|(lead, sev)| line.strip_prefix(lead).map(|r| (sev, r)));
        if let Some((severity, rest)) = coded {
            let Some((code, msg)) = rest.split_once("]: ") else {
                continue;
            };
            let site = lines
                .get(i + 1)
                .and_then(|l| l.trim_start().strip_prefix("--> "))
                .and_then(|loc| {
                    let mut parts = loc.rsplitn(3, ':');
                    let (_col, line, file) = (parts.next()?, parts.next()?, parts.next()?);
                    let file = file_name(Path::new(file));
                    let line = line.parse().ok()?;
                    files.contains(&file).then_some(Site::Line { file, line })
                })
                .unwrap_or(Site::Unlocated);
            out.push(Actual {
                severity,
                code: Some(code.to_string()),
                msg: msg.to_string(),
                site,
            });
        } else if let Some(msg) = line.strip_prefix("thrax: ") {
            if !msg.ends_with(':') {
                out.push(Actual {
                    severity: Severity::Error,
                    code: None,
                    msg: msg.to_string(),
                    site: Site::Unlocated,
                });
            }
        }
    }
    out
}

/// Run one case. `Err` carries the report of what went wrong.
fn run_case(thrax: &Path, root: &Path, files: &[PathBuf]) -> Result<(), String> {
    let names: Vec<String> = files.iter().map(|f| file_name(f)).collect();
    let mut expected = Vec::new();
    for (path, name) in files.iter().zip(&names) {
        let src = std::fs::read_to_string(path).map_err(|e| format!("cannot read {name}: {e}"))?;
        expected.extend(annotations(name, &src)?);
    }
    if expected.is_empty() {
        return Err("no `#~` annotations: a negative case must expect a diagnostic".into());
    }

    let dir = root.parent().unwrap();
    let out = Command::new(thrax)
        .arg("check")
        .arg(file_name(root))
        .current_dir(dir)
        .output()
        .map_err(|e| format!("cannot run thrax: {e}"))?;
    let stderr = String::from_utf8_lossy(&out.stderr);
    let report = |what: String| format!("{what}\n--- stderr ---\n{stderr}--------------");

    // 0 is a clean check, 1 a reported failure; anything else is a crash.
    let code = out.status.code();
    if !matches!(code, Some(0) | Some(1)) || stderr.contains("panicked at") {
        return Err(report(format!("the compiler crashed ({})", out.status)));
    }

    let mut actual = diagnostics(&stderr, &names);
    let mut problems = Vec::new();
    for e in &expected {
        match actual.iter().position(|a| e.matches(a)) {
            Some(i) => {
                actual.remove(i);
            }
            None => problems.push(format!("expected, not produced:  {e}")),
        }
    }
    for a in &actual {
        problems.push(format!("produced, not expected:  {a}"));
    }
    let wants_error = expected.iter().any(|e| e.severity == Severity::Error);
    if wants_error && code == Some(0) {
        problems.push("the program was ACCEPTED (exit 0), but an error is expected".into());
    }
    if !wants_error && code == Some(1) {
        problems.push("the program was REJECTED (exit 1), but only warnings are expected".into());
    }
    if problems.is_empty() {
        Ok(())
    } else {
        Err(report(problems.join("\n")))
    }
}

/// Every case under `tests/diagnostics/`: (name, root file, all its files).
fn cases() -> Vec<(String, PathBuf, Vec<PathBuf>)> {
    let dir = repo_root().join("tests/diagnostics");
    let mut out = Vec::new();
    for entry in std::fs::read_dir(&dir).expect("tests/diagnostics exists") {
        let path = entry.unwrap().path();
        let name = file_name(&path);
        if path.is_dir() {
            let mut files: Vec<PathBuf> = std::fs::read_dir(&path)
                .unwrap()
                .map(|e| e.unwrap().path())
                .filter(|p| p.extension().is_some_and(|x| x == "thx"))
                .collect();
            files.sort();
            out.push((name, path.join("MAIN.thx"), files));
        } else if path.extension().is_some_and(|x| x == "thx") {
            out.push((name, path.clone(), vec![path]));
        }
    }
    out.sort();
    out
}

#[test]
fn negative_suite() {
    let thrax = PathBuf::from(env!("CARGO_BIN_EXE_thrax"));
    let cases = cases();
    assert!(!cases.is_empty(), "no cases under tests/diagnostics");
    let results: Vec<(String, Result<(), String>)> = std::thread::scope(|s| {
        let handles: Vec<_> = cases
            .iter()
            .map(|(name, root, files)| {
                let thrax = &thrax;
                s.spawn(move || (name.clone(), run_case(thrax, root, files)))
            })
            .collect();
        handles.into_iter().map(|h| h.join().unwrap()).collect()
    });
    let failures: Vec<String> = results
        .into_iter()
        .filter_map(|(name, r)| r.err().map(|e| format!("=== {name}\n{e}")))
        .collect();
    assert!(
        failures.is_empty(),
        "{} of {} negative case(s) failed:\n\n{}",
        failures.len(),
        cases.len(),
        failures.join("\n\n")
    );
}

#[test]
fn annotation_syntax() {
    let src = "a\nb  #~ error[TYPE_MISMATCH]: missing field `y`\n#~^^ warning[UNREACHABLE_ARM]\n#~ error: cannot find module\n";
    let got = annotations("F.thx", src).unwrap();
    let shown: Vec<String> = got.iter().map(|e| e.to_string()).collect();
    assert_eq!(
        shown,
        [
            "F.thx:2: error[TYPE_MISMATCH]: missing field `y`",
            "F.thx:1: warning[UNREACHABLE_ARM]",
            "<unlocated>: error: cannot find module",
        ]
    );
    assert!(annotations("F.thx", "#~^ error").is_err());
    assert!(annotations("F.thx", "x #~ oops").is_err());
}

#[test]
fn stderr_parsing() {
    let stderr = "\
error[TYPE_MISMATCH]: struct `P` has no field `z`
  --> /some/dir/F.thx:4:11
  |
4 | $ p : P = P.{ .x = 1, .z = 2 }
  |           ^^^^^^^^^^^^^^^^^^^^
warning[UNREACHABLE_ARM]: unreachable arm
  --> CORE.thx:9:1
error[META_ABORT]: boom
  --> F.thx
note: raised while running this compile-time `@e`
thrax: cannot find module `NOPE`
";
    let got: Vec<String> = diagnostics(stderr, &["F.thx".to_string()])
        .iter()
        .map(|a| a.to_string())
        .collect();
    assert_eq!(
        got,
        [
            "F.thx:4: error[TYPE_MISMATCH]: struct `P` has no field `z`",
            "<unlocated>: warning[UNREACHABLE_ARM]: unreachable arm",
            "<unlocated>: error[META_ABORT]: boom",
            "<unlocated>: error: cannot find module `NOPE`",
        ]
    );
}
