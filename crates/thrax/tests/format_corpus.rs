//! The formatter over every library module and example: it must accept each
//! one (no refusal from its token check) and be idempotent. Files on disk are
//! never written.

use std::path::Path;

use frontend::format::{format_source, Style};

#[test]
fn formats_the_whole_corpus_idempotently() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let style = Style::default();
    let mut failures = Vec::new();
    let mut count = 0;
    for dir in ["library", "examples"] {
        let mut files: Vec<_> = std::fs::read_dir(root.join(dir))
            .expect("corpus directory")
            .filter_map(|e| e.ok().map(|e| e.path()))
            .filter(|p| p.extension().is_some_and(|x| x == "thx"))
            .collect();
        files.sort();
        for path in files {
            count += 1;
            let src = std::fs::read_to_string(&path).expect("readable");
            match format_source(&src, &style) {
                Ok(once) => match format_source(&once, &style) {
                    Ok(twice) if twice == once => {}
                    Ok(_) => failures.push(format!("{}: not idempotent", path.display())),
                    Err(e) => failures.push(format!("{}: second pass refused: {e}", path.display())),
                },
                Err(e) => failures.push(format!("{}: {e}", path.display())),
            }
        }
    }
    assert!(count > 50, "found only {count} corpus files");
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}
