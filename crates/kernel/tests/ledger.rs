//! The theory ledger checker: `documentation/theory/LEDGER.md` and the
//! `// Rule [Name]`, `// Algorithm [Name]` and `// Test [Name]` tags in the
//! workspace's Rust sources must agree, and every vendored source must be in
//! `documentation/theory/references` with the hash the ledger gives.
//!
//! A tag is a line comment whose whole text is `Rule [Name]` or
//! `Algorithm [Name]`, beside the code that implements it, or `Test [Name]`,
//! above the test of a rule. Every implemented rule needs a test, and the
//! text from a test tag to the next must call both `applies(` (a case where
//! the rule fires) and `blocked(` (a near miss where it must not). A
//! reference is `See Rule [Name]` (or `Algorithm`, `Test`) anywhere in a
//! comment, on one line. Any other `Rule [`, `Algorithm [` or `Test [` in a
//! comment is reported, so a misspelled tag fails instead of reading as
//! prose. The rules the ledger must
//! satisfy are listed at its top.
//!
//! This file is skipped by the scan: its own fixtures would trip it.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

// ## The ledger

/// One table row, by column header.
type Row = BTreeMap<String, String>;

/// The rows of every table, by the `## Heading` above it.
fn parse_tables(src: &str) -> BTreeMap<String, Vec<Row>> {
    let mut tables: BTreeMap<String, Vec<Row>> = BTreeMap::new();
    let mut section = String::new();
    let mut header: Option<Vec<String>> = None;
    for line in src.lines() {
        if let Some(title) = line.strip_prefix("## ") {
            section = title.trim().to_string();
            header = None;
            continue;
        }
        let Some(inner) = line.trim().strip_prefix('|').and_then(|l| l.strip_suffix('|')) else {
            header = None;
            continue;
        };
        let cells: Vec<String> = inner.split('|').map(|c| c.trim().to_string()).collect();
        match &header {
            None => header = Some(cells),
            Some(_) if cells.iter().all(|c| c.chars().all(|ch| ch == '-' || ch == ':')) => {}
            Some(h) => {
                assert_eq!(h.len(), cells.len(), "a row of `## {section}` has {} cells, its header {}: {line}", cells.len(), h.len());
                tables.entry(section.clone()).or_default().push(h.iter().cloned().zip(cells).collect());
            }
        }
    }
    tables
}

/// A comma-separated cell, `-` meaning none.
fn list(cell: &str) -> Vec<String> {
    if cell == "-" {
        return Vec::new();
    }
    cell.split(',').map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect()
}

fn is_issue(cell: &str) -> bool {
    cell.strip_prefix('#').is_some_and(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()))
}

const STATUSES: [&str; 4] = ["published", "published-uncovered", "deviation", "conjecture"];
const PROOFS: [&str; 6] = ["paper", "paper-sketch", "Agda", "Coq", "Lean", "model"];

/// Tags found in the sources: kind ("Rule" or "Algorithm") and name, with
/// where each implementation tag is.
#[derive(Default)]
struct Tags {
    implementations: BTreeMap<(String, String), Vec<String>>,
    references: Vec<((String, String), String)>,
    malformed: Vec<(String, String)>,
    /// The text after each `// Test [Name]` tag, up to the next one.
    test_bodies: Vec<(String, String, String)>,
}

const KINDS: [&str; 3] = ["Rule", "Algorithm", "Test"];

impl Tags {
    fn scan(&mut self, file: &str, src: &str) {
        let lines: Vec<&str> = src.lines().collect();
        for (i, line) in lines.iter().enumerate() {
            let site = format!("{file}:{}", i + 1);
            let trimmed = line.trim();
            let tag = KINDS.iter().find_map(|kind| {
                let name = trimmed.strip_prefix("// ")?.strip_prefix(kind)?.strip_prefix(" [")?.strip_suffix(']')?;
                Some((kind.to_string(), name.to_string()))
            });
            if let Some((kind, name)) = tag {
                if name.is_empty() || name.contains(']') {
                    self.malformed.push((trimmed.to_string(), site));
                } else {
                    if kind == "Test" {
                        let rest = &lines[i + 1..];
                        let end = rest.iter().position(|l| l.trim().starts_with("// Test [")).unwrap_or(rest.len());
                        self.test_bodies.push((name.clone(), site.clone(), rest[..end].join("\n")));
                    }
                    self.implementations.entry((kind, name)).or_default().push(site);
                }
                continue;
            }
            let Some(at) = line.find("//") else { continue };
            let comment = &line[at..];
            for kind in KINDS {
                let opener = format!("{kind} [");
                let mut rest = comment;
                while let Some(k) = rest.find(&opener) {
                    let after = &rest[k + opener.len()..];
                    let is_reference = rest[..k].ends_with("See ");
                    match after.find(']') {
                        Some(end) if is_reference && end > 0 => {
                            self.references.push(((kind.to_string(), after[..end].to_string()), site.clone()));
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
    }
}

/// Every violation, one line each.
fn problems(ledger: &str, tags: &Tags, references: Option<&Path>) -> Vec<String> {
    let tables = parse_tables(ledger);
    let table = |name: &str| tables.get(name).map(Vec::as_slice).unwrap_or(&[]);
    let mut out = Vec::new();

    let mut sources = BTreeSet::new();
    for row in table("Sources") {
        let key = &row["Key"];
        if !sources.insert(key.clone()) {
            out.push(format!("source {key} is listed twice"));
        }
        if row["DOI or URL"].is_empty() || row["Licence"].is_empty() {
            out.push(format!("source {key} needs a DOI or URL and a licence"));
        }
        let (path, hash) = (&row["Vendored"], &row["SHA-256"]);
        match (path.as_str(), hash.as_str()) {
            ("-", "-") => {}
            ("-", _) | (_, "-") => out.push(format!("source {key} has a vendored path or a hash, but not both")),
            _ => match references {
                None => {}
                Some(dir) => match std::fs::read(dir.join(path)) {
                    Err(_) => out.push(format!("source {key}: `references/{path}` is missing")),
                    Ok(bytes) if sha256_hex(&bytes) != *hash => {
                        out.push(format!("source {key}: `references/{path}` has SHA-256 {}, the ledger {hash}", sha256_hex(&bytes)))
                    }
                    Ok(_) => {}
                },
            },
        }
    }
    let cites = |out: &mut Vec<String>, what: &str, cell: &str| {
        for key in list(cell) {
            if !sources.contains(&key) {
                out.push(format!("{what} cites source {key}, which the ledger does not list"));
            }
        }
    };

    let mut theorems = BTreeSet::new();
    for row in table("Theorems") {
        let name = &row["Theorem"];
        theorems.insert(name.clone());
        cites(&mut out, &format!("theorem {name}"), &row["Source"]);
        if !PROOFS.contains(&row["Proof"].as_str()) {
            out.push(format!("theorem {name}: proof kind `{}` is not one of {PROOFS:?}", row["Proof"]));
        }
    }

    for (section, kind, column) in [("Rules", "Rule", "Rule"), ("Algorithms", "Algorithm", "Algorithm")] {
        let mut names = BTreeSet::new();
        for row in table(section) {
            let name = &row[column];
            let what = format!("{kind} [{name}]");
            if !names.insert(name.clone()) {
                out.push(format!("{what} is listed twice"));
            }
            cites(&mut out, &what, &row["Source"]);
            let status = row["Status"].as_str();
            let issue = row["Issue"].as_str();
            if !STATUSES.contains(&status) {
                out.push(format!("{what}: status `{status}` is not one of {STATUSES:?}"));
            }
            if issue != "-" && !is_issue(issue) {
                out.push(format!("{what}: issue `{issue}` is not `#N`"));
            }
            if matches!(status, "deviation" | "conjecture") && !is_issue(issue) {
                out.push(format!("{what} is a {status} and names no issue"));
            }
            if status != "conjecture" && list(&row["Source"]).is_empty() {
                out.push(format!("{what} is {status} and cites no source"));
            }
            let covered = row.get("Covered by").map(|c| list(c)).unwrap_or_default();
            for t in &covered {
                if !theorems.contains(t) {
                    out.push(format!("{what} is covered by theorem {t}, which the ledger does not list"));
                }
            }
            if status == "published" && covered.is_empty() {
                out.push(format!("{what} is published and names no theorem that covers it"));
            }
            let tagged = tags.implementations.get(&(kind.to_string(), name.clone()));
            match (row["Code"].as_str(), tagged) {
                ("yes", None) => out.push(format!("{what} is marked implemented, but no `// {kind} [{name}]` tag exists")),
                ("planned", Some(sites)) => out.push(format!("{what} is marked planned, but is tagged at {}", sites.join(", "))),
                ("yes" | "planned", _) => {}
                (other, _) => out.push(format!("{what}: Code `{other}` is not `yes` or `planned`")),
            }
            if kind == "Rule" && row["Code"] == "yes" && !tags.implementations.contains_key(&("Test".to_string(), name.clone())) {
                out.push(format!("{what} is implemented, but no test is tagged `// Test [{name}]`"));
            }
        }
        for ((k, name), sites) in &tags.implementations {
            if k == kind && !names.contains(name) {
                out.push(format!("{}: `// {kind} [{name}]` has no ledger entry", sites[0]));
            }
        }
        for ((k, name), site) in &tags.references {
            if k == kind && !names.contains(name) {
                out.push(format!("{site}: See {kind} [{name}] has no ledger entry"));
            }
        }
    }

    let rules: BTreeSet<&String> = table("Rules").iter().map(|row| &row["Rule"]).collect();
    for (name, site, body) in &tags.test_bodies {
        if !rules.contains(name) {
            out.push(format!("{site}: `// Test [{name}]` names no rule in the ledger"));
        }
        for call in ["applies(", "blocked("] {
            if !body.contains(call) {
                out.push(format!("{site}: the test of Rule [{name}] never calls `{call}..)`"));
            }
        }
    }

    for row in table("Assumptions") {
        if !is_issue(&row["Issue"]) {
            out.push(format!("assumption {} names no issue", row["Assumption"]));
        }
    }

    for (text, site) in &tags.malformed {
        out.push(format!(
            "{site}: `{text}` is neither a tag (`// Rule [Name]` alone on its line) nor a reference (`See Rule [Name]` on one line)"
        ));
    }
    out
}

// ## SHA-256 (FIPS 180-4)

const K: [u32; 64] = [
    0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4, 0xab1c5ed5, 0xd807aa98, 0x12835b01,
    0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe, 0x9bdc06a7, 0xc19bf174, 0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc,
    0x2de92c6f, 0x4a7484aa, 0x5cb0a9dc, 0x76f988da, 0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7, 0xc6e00bf3, 0xd5a79147,
    0x06ca6351, 0x14292967, 0x27b70a85, 0x2e1b2138, 0x4d2c6dfc, 0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85,
    0xa2bfe8a1, 0xa81a664b, 0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070, 0x19a4c116, 0x1e376c08,
    0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3, 0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208,
    0x90befffa, 0xa4506ceb, 0xbef9a3f7, 0xc67178f2,
];

fn sha256_hex(data: &[u8]) -> String {
    let mut h: [u32; 8] = [0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a, 0x510e527f, 0x9b05688c, 0x1f83d9ab, 0x5be0cd19];
    let mut msg = data.to_vec();
    msg.push(0x80);
    while msg.len() % 64 != 56 {
        msg.push(0);
    }
    msg.extend_from_slice(&((data.len() as u64) * 8).to_be_bytes());
    for block in msg.chunks(64) {
        let mut w = [0u32; 64];
        for (i, word) in block.chunks(4).enumerate() {
            w[i] = u32::from_be_bytes([word[0], word[1], word[2], word[3]]);
        }
        for i in 16..64 {
            let s0 = w[i - 15].rotate_right(7) ^ w[i - 15].rotate_right(18) ^ (w[i - 15] >> 3);
            let s1 = w[i - 2].rotate_right(17) ^ w[i - 2].rotate_right(19) ^ (w[i - 2] >> 10);
            w[i] = w[i - 16].wrapping_add(s0).wrapping_add(w[i - 7]).wrapping_add(s1);
        }
        let [mut a, mut b, mut c, mut d, mut e, mut f, mut g, mut hh] = h;
        for i in 0..64 {
            let s1 = e.rotate_right(6) ^ e.rotate_right(11) ^ e.rotate_right(25);
            let ch = (e & f) ^ (!e & g);
            let t1 = hh.wrapping_add(s1).wrapping_add(ch).wrapping_add(K[i]).wrapping_add(w[i]);
            let s0 = a.rotate_right(2) ^ a.rotate_right(13) ^ a.rotate_right(22);
            let maj = (a & b) ^ (a & c) ^ (b & c);
            let t2 = s0.wrapping_add(maj);
            (hh, g, f, e, d, c, b, a) = (g, f, e, d.wrapping_add(t1), c, b, a, t1.wrapping_add(t2));
        }
        for (x, y) in h.iter_mut().zip([a, b, c, d, e, f, g, hh]) {
            *x = x.wrapping_add(y);
        }
    }
    h.iter().map(|x| format!("{x:08x}")).collect()
}

// ## The workspace

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
fn the_ledger_and_the_code_agree() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let this = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/ledger.rs").canonicalize().expect("this file");
    let references = root.join("documentation/theory/references");
    let ledger = std::fs::read_to_string(root.join("documentation/theory/LEDGER.md")).expect("the ledger exists");
    let mut files = Vec::new();
    rust_sources(&root.join("crates"), &mut files);
    files.sort();
    let mut tags = Tags::default();
    for path in &files {
        if path.canonicalize().is_ok_and(|p| p == this) {
            continue;
        }
        let src = std::fs::read_to_string(path).expect("readable");
        let name = path.strip_prefix(&root).unwrap_or(path).display().to_string();
        tags.scan(&name, &src);
    }
    assert!(!tags.implementations.is_empty(), "found no tags under crates/");
    let problems = problems(&ledger, &tags, Some(&references));
    assert!(problems.is_empty(), "{}", problems.join("\n"));
}

// ## The checker itself

const LEDGER: &str = "\
## Sources

| Key | Citation | DOI or URL | Vendored | SHA-256 | Licence |
|---|---|---|---|---|---|
| P | A paper | https://example.org | - | - | CC-BY-4.0 |

## Theorems

| Theorem | Source | Where in source | Fragment | Proof |
|---|---|---|---|---|
| P Normalisation | P | Thm 1 | all of it | Agda |

## Algorithms

| Algorithm | Source | Where in source | Status | Issue | Code |
|---|---|---|---|---|---|
| Walk | P | Sec 2 | published-uncovered | - | yes |

## Rules

| Rule | Source | Where in source | Status | Covered by | Issue | Code |
|---|---|---|---|---|---|---|
| Beta | P | Fig 1 | published | P Normalisation | - | yes |
| Odd | - | - | conjecture | - | #7 | planned |

## Assumptions

| Assumption | Why it is believed | What would discharge it | Issue |
|---|---|---|---|
| Fine | it is | a proof | #8 |
";

/// A test of Rule [Beta] that calls both helpers.
const TESTED: &str = "// Test [Beta]\nfn beta() { applies(); blocked(); }\n";

fn scanned(files: &[(&str, &str)]) -> Tags {
    let mut tags = Tags::default();
    for (name, src) in files {
        tags.scan(name, src);
    }
    tags
}

#[test]
fn checker_accepts_a_consistent_ledger() {
    let tags = scanned(&[("a.rs", "// Rule [Beta]\nfn beta() {}\n// Algorithm [Walk]\n// See Rule [Beta].\n"), ("t.rs", TESTED)]);
    assert_eq!(problems(LEDGER, &tags, None), Vec::<String>::new());
}

#[test]
fn checker_reports_an_unknown_or_missing_tag() {
    let tags = scanned(&[("a.rs", "// Rule [Gamma]\n// Algorithm [Walk]\n")]);
    let found = problems(LEDGER, &tags, None);
    assert!(found.iter().any(|p| p.contains("Rule [Beta] is marked implemented")), "{found:?}");
    assert!(found.iter().any(|p| p.contains("a.rs:1: `// Rule [Gamma]` has no ledger entry")), "{found:?}");
}

#[test]
fn checker_reports_a_tagged_planned_rule_and_a_bad_mention() {
    let tags = scanned(&[("a.rs", "// Rule [Beta]\n// Algorithm [Walk]\n// Rule [Odd]\n// uses Rule [Beta] here\n")]);
    let found = problems(LEDGER, &tags, None);
    assert!(found.iter().any(|p| p.contains("Rule [Odd] is marked planned, but is tagged at a.rs:3")), "{found:?}");
    assert!(found.iter().any(|p| p.starts_with("a.rs:4:")), "{found:?}");
}

#[test]
fn checker_reports_status_without_its_evidence() {
    let tags = scanned(&[("a.rs", "// Rule [Beta]\n// Algorithm [Walk]\n")]);
    let ledger = LEDGER
        .replace("| published | P Normalisation | - | yes |", "| published | - | - | yes |")
        .replace("| conjecture | - | #7 | planned |", "| deviation | - | - | planned |")
        .replace("| a proof | #8 |", "| a proof | - |");
    let found = problems(&ledger, &tags, None);
    assert!(found.iter().any(|p| p.contains("Rule [Beta] is published and names no theorem")), "{found:?}");
    assert!(found.iter().any(|p| p.contains("Rule [Odd] is a deviation and names no issue")), "{found:?}");
    assert!(found.iter().any(|p| p.contains("Rule [Odd] is deviation and cites no source")), "{found:?}");
    assert!(found.iter().any(|p| p.contains("assumption Fine names no issue")), "{found:?}");
}

#[test]
fn checker_reports_an_untested_rule_and_a_one_sided_test() {
    let untested = problems(LEDGER, &scanned(&[("a.rs", "// Rule [Beta]\n// Algorithm [Walk]\n")]), None);
    assert!(untested.iter().any(|p| p.contains("Rule [Beta] is implemented, but no test is tagged")), "{untested:?}");
    let one_sided = "// Test [Beta]\nfn beta() { applies(); }\n// Test [Gamma]\nfn gamma() { applies(); blocked(); }\n";
    let found = problems(LEDGER, &scanned(&[("a.rs", "// Rule [Beta]\n// Algorithm [Walk]\n"), ("t.rs", one_sided)]), None);
    assert!(found.iter().any(|p| p.contains("t.rs:1: the test of Rule [Beta] never calls `blocked(..)`")), "{found:?}");
    assert!(found.iter().any(|p| p.contains("t.rs:3: `// Test [Gamma]` names no rule")), "{found:?}");
}

#[test]
fn checker_reports_a_missing_or_altered_source_file() {
    let dir = std::env::temp_dir().join(format!("ares-ledger-test-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("p.pdf"), b"abc").unwrap();
    let tags = scanned(&[("a.rs", "// Rule [Beta]\n// Algorithm [Walk]\n"), ("t.rs", TESTED)]);
    let abc = "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad";
    let good = LEDGER.replace("| - | - | CC-BY-4.0 |", &format!("| p.pdf | {abc} | CC-BY-4.0 |"));
    assert_eq!(problems(&good, &tags, Some(&dir)), Vec::<String>::new());
    let altered = good.replace(abc, &"0".repeat(64));
    assert!(problems(&altered, &tags, Some(&dir))[0].contains("has SHA-256 ba7816bf"));
    let missing = good.replace("p.pdf", "q.pdf");
    assert!(problems(&missing, &tags, Some(&dir))[0].contains("`references/q.pdf` is missing"));
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn sha256_matches_the_standard_vectors() {
    assert_eq!(sha256_hex(b""), "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855");
    assert_eq!(sha256_hex(b"abc"), "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad");
    let long = b"abcdbcdecdefdefgefghfghighijhijkijkljklmklmnlmnomnopnopq";
    assert_eq!(sha256_hex(long), "248d6a61d20638b8e5c026930c3e6039a33ce45964ff2167f6ecedd419db06c1");
}
