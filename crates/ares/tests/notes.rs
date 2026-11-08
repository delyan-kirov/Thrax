//! The Note checker: every `See Note [Title]` in the workspace's Rust sources
//! must name a Note that is defined exactly once.
//!
//! A definition is a line comment whose whole text is `Note [Title]`. A
//! reference is `See Note [Title]` anywhere in a comment, on one line. Any other
//! `Note [` in a comment is reported, so a misspelled reference fails instead
//! of reading as prose. The convention is in `documentation/compiler-style.md`.
//!
//! This file is skipped by the scan: its own fixtures would trip it.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// Where a Note is defined or referenced.
#[derive(Clone, Debug)]
struct Site {
    file: String,
    line: usize,
}

impl std::fmt::Display for Site {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}:{}", self.file, self.line)
    }
}

#[derive(Default)]
struct Notes {
    definitions: BTreeMap<String, Vec<Site>>,
    references: Vec<(String, Site)>,
    malformed: Vec<(String, Site)>,
}

impl Notes {
    fn scan(&mut self, file: &str, src: &str) {
        for (i, line) in src.lines().enumerate() {
            let site = Site { file: file.to_string(), line: i + 1 };
            let trimmed = line.trim();
            if let Some(title) = trimmed.strip_prefix("// Note [").and_then(|t| t.strip_suffix(']')) {
                if title.is_empty() || title.contains(']') {
                    self.malformed.push((trimmed.to_string(), site));
                } else {
                    self.definitions.entry(title.to_string()).or_default().push(site);
                }
                continue;
            }
            let Some(at) = line.find("//") else { continue };
            let comment = &line[at..];
            let mut rest = comment;
            while let Some(k) = rest.find("Note [") {
                let after = &rest[k + "Note [".len()..];
                let is_reference = rest[..k].ends_with("See ");
                match after.find(']') {
                    Some(end) if is_reference && end > 0 => {
                        self.references.push((after[..end].to_string(), site.clone()));
                        rest = &after[end + 1..];
                    }
                    _ => {
                        self.malformed.push((comment.trim().to_string(), site.clone()));
                        break;
                    }
                }
            }
        }
    }

    /// Every violation, one line each.
    fn problems(&self) -> Vec<String> {
        let mut out = Vec::new();
        for (title, sites) in &self.definitions {
            if sites.len() > 1 {
                let all: Vec<String> = sites.iter().map(Site::to_string).collect();
                out.push(format!("Note [{title}] is defined {} times: {}", sites.len(), all.join(", ")));
            }
        }
        for (title, site) in &self.references {
            if !self.definitions.contains_key(title) {
                out.push(format!("{site}: See Note [{title}] has no definition"));
            }
        }
        for (text, site) in &self.malformed {
            out.push(format!(
                "{site}: `{text}` is neither a definition (`// Note [Title]` alone on its line) \
                 nor a reference (`See Note [Title]` on one line)"
            ));
        }
        out
    }
}

fn rust_sources(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    for entry in entries.filter_map(Result::ok) {
        let path = entry.path();
        if path.is_dir() {
            if path.file_name().is_some_and(|n| n == "target") {
                continue;
            }
            rust_sources(&path, out);
        } else if path.extension().is_some_and(|x| x == "rs") {
            out.push(path);
        }
    }
}

#[test]
fn every_note_reference_resolves() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let this = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/notes.rs").canonicalize().expect("this file");
    let mut files = Vec::new();
    for dir in ["crates", "applications/web/playground/src"] {
        rust_sources(&root.join(dir), &mut files);
    }
    files.sort();
    assert!(files.len() > 50, "found only {} Rust files", files.len());
    let mut notes = Notes::default();
    for path in &files {
        if path.canonicalize().is_ok_and(|p| p == this) {
            continue;
        }
        let src = std::fs::read_to_string(path).expect("readable");
        let name = path.strip_prefix(&root).unwrap_or(path).display().to_string();
        notes.scan(&name, &src);
    }
    let problems = notes.problems();
    assert!(problems.is_empty(), "{}", problems.join("\n"));
}

#[test]
fn checker_accepts_a_defined_reference() {
    let mut notes = Notes::default();
    notes.scan("a.rs", "// Note [Levels]\n//\n// Body.\nfn f() {}\n");
    notes.scan("b.rs", "fn g() {} // See Note [Levels].\n/// See Note [Levels]\n");
    assert_eq!(notes.problems(), Vec::<String>::new());
    assert_eq!(notes.references.len(), 2);
}

#[test]
fn checker_reports_a_dangling_reference() {
    let mut notes = Notes::default();
    notes.scan("a.rs", "// See Note [Missing]\n");
    assert_eq!(notes.problems(), vec!["a.rs:1: See Note [Missing] has no definition"]);
}

#[test]
fn checker_reports_a_duplicate_definition() {
    let mut notes = Notes::default();
    notes.scan("a.rs", "// Note [Twice]\n");
    notes.scan("b.rs", "    // Note [Twice]\n");
    assert_eq!(notes.problems(), vec!["Note [Twice] is defined 2 times: a.rs:1, b.rs:1"]);
}

#[test]
fn checker_reports_a_bare_or_broken_mention() {
    let mut notes = Notes::default();
    notes.scan("a.rs", "// Note [Real]\n// see Note [Real]\n// See Note [Real\n");
    let problems = notes.problems();
    assert_eq!(problems.len(), 2, "{problems:?}");
    assert!(problems[0].starts_with("a.rs:2:"), "{problems:?}");
    assert!(problems[1].starts_with("a.rs:3:"), "{problems:?}");
}

#[test]
fn checker_ignores_code_outside_comments() {
    let mut notes = Notes::default();
    notes.scan("a.rs", "let s = \"Note [x]\";\n");
    assert_eq!(notes.problems(), Vec::<String>::new());
}
