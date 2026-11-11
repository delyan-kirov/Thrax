//! Checks every kernel script under `scripts/`: each must be accepted, and
//! the axioms its globals rest on must match the `# axioms:` lines in it.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

fn scripts() -> Vec<PathBuf> {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("scripts");
    let mut paths: Vec<_> = std::fs::read_dir(&dir)
        .expect("the scripts directory exists")
        .map(|e| e.expect("a readable entry").path())
        // FIXME(#473): `.ott` until the kernel reads Ares, then `.ares`.
        .filter(|p| p.extension().is_some_and(|e| e == "ott"))
        .collect();
    paths.sort();
    assert!(!paths.is_empty(), "no scripts in {}", dir.display());
    paths
}

#[test]
fn every_script_checks() {
    for path in scripts() {
        let src = std::fs::read_to_string(&path).expect("a readable script");
        let name = path.file_name().unwrap().to_string_lossy().to_string();
        if let Err(e) = kernel::check_script(&src) {
            panic!("{name} is rejected:\n{}", e.render(&src, &name));
        }
    }
}

#[test]
fn the_rationals_reject_wrong_claims() {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("scripts/rationals.ott");
    let base = std::fs::read_to_string(path).expect("a readable script");
    let cases = [
        // 1/2 * 1/2 is not 1/2.
        (format!("{base}\n$ wrong : qmul half half ~ half = @tt\n"), "expected `@Bottom`, found `@Top`"),
        // The numerator of a fraction is not a function of its class.
        (
            format!("{base}\n$ numerator : Q -> Integer = \\q => @qelim (\\_ => Integer) (\\x => x.1) (\\x y e => e) q\n"),
            "expected `x.1 ~ y.1`",
        ),
    ];
    for (src, needle) in cases {
        match kernel::check_script(&src) {
            Ok(_) => panic!("accepted, expected an error containing `{needle}`"),
            Err(e) => {
                let shown = e.render(&src, "rationals.ott");
                assert!(shown.contains(needle), "error does not mention `{needle}`:\n{shown}");
            }
        }
    }
}

/// The axioms each global rests on, for every global that is not itself an
/// axiom and rests on at least one.
fn axioms_of(sig: &kernel::Signature) -> BTreeMap<String, String> {
    let mut out = BTreeMap::new();
    for name in sig.names() {
        let axioms = kernel::axioms(sig, name).expect("a declared name");
        if !axioms.is_empty() && axioms != [name] {
            out.insert(name.to_string(), axioms.join(" "));
        }
    }
    out
}

/// The `# axioms: a b` lines, each keyed by the declaration below it.
fn pinned_axioms(src: &str) -> BTreeMap<String, String> {
    let mut out = BTreeMap::new();
    let mut pending: Option<String> = None;
    for line in src.lines() {
        if let Some(axioms) = line.strip_prefix("# axioms:") {
            pending = Some(axioms.split_whitespace().collect::<Vec<_>>().join(" "));
        } else if let Some(pin) = pending.take() {
            let name = line.strip_prefix("$ ").and_then(|l| l.split([' ', ':']).next());
            let name = name.unwrap_or_else(|| panic!("a `# axioms:` line is not directly above a declaration: {line}"));
            out.insert(name.to_string(), pin);
        }
    }
    out
}

#[test]
fn every_script_pins_its_axioms() {
    for path in scripts() {
        let name = path.file_name().unwrap().to_string_lossy().to_string();
        let src = std::fs::read_to_string(&path).expect("a readable script");
        let sig = kernel::check_script(&src).unwrap_or_else(|e| panic!("{}", e.render(&src, &name)));
        let (actual, pinned) = (axioms_of(&sig), pinned_axioms(&src));
        let mut problems = Vec::new();
        for (global, axioms) in &actual {
            match pinned.get(global) {
                Some(pin) if pin == axioms => {}
                Some(pin) => problems.push(format!("`{global}` rests on `{axioms}`, but its pin says `{pin}`")),
                None => problems.push(format!("`{global}` rests on `{axioms}` and needs `# axioms: {axioms}`")),
            }
        }
        for (global, pin) in &pinned {
            if !actual.contains_key(global) {
                problems.push(format!("`{global}` rests on no axiom, but is pinned to `{pin}`"));
            }
        }
        assert!(problems.is_empty(), "{name}:\n{}", problems.join("\n"));
    }
}

#[test]
fn axioms_used_only_inside_a_proof_are_reported() {
    let src = "\
$ Nat : @Type 1 = @data | zero | succ Nat
$ ax : zero ~ succ zero
$ pr : (P: Nat -> @Type 0) -> P zero -> P (succ zero) = \\P p => @transp zero (\\x => P x) p (succ zero) ax
$ uses_pr : (P: Nat -> @Type 0) -> P zero -> P (succ zero) = pr
$ clean : Nat = succ zero
";
    let sig = kernel::check_script(src).unwrap_or_else(|e| panic!("{}", e.render(src, "test")));
    assert_eq!(kernel::axioms(&sig, "pr").unwrap(), ["ax"]);
    assert_eq!(kernel::axioms(&sig, "uses_pr").unwrap(), ["ax"]);
    assert_eq!(kernel::axioms(&sig, "ax").unwrap(), ["ax"]);
    assert!(kernel::axioms(&sig, "clean").unwrap().is_empty());
    assert!(kernel::axioms(&sig, "Nat.elim").unwrap().is_empty());
}

/// CIC^obs Section 2.3: making `D X ~ D Y` compute to `X ~ Y` injects
/// `@Type 1 -> @Type 1` into `@Type 1`, from which Russell's paradox follows.
/// The kernel accepted this proof until #477.
#[test]
fn type_formers_are_not_injective() {
    let src = "\
$ D : (X: @Type 1 -> @Type 1) -> @Type 1 = @data | mk
$ inj : (X Y: @Type 1 -> @Type 1) -> D X ~ D Y -> X ~ Y = \\X Y e => e
";
    match kernel::check_script(src) {
        Ok(_) => panic!("accepted a proof that `D` is injective"),
        Err(e) => {
            let shown = e.render(src, "test");
            assert!(shown.contains("TYPE_MISMATCH"), "{shown}");
        }
    }
}
