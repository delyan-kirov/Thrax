//! Checks every kernel script under `scripts/`: each must be accepted.

use std::path::Path;

#[test]
fn every_script_checks() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("scripts");
    let mut paths: Vec<_> = std::fs::read_dir(&dir)
        .expect("the scripts directory exists")
        .map(|e| e.expect("a readable entry").path())
        .filter(|p| p.extension().is_some_and(|e| e == "ott"))
        .collect();
    paths.sort();
    assert!(!paths.is_empty(), "no scripts in {}", dir.display());
    for path in paths {
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
