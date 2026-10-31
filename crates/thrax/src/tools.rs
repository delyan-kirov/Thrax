//! The source tools: `thrax fmt`, `thrax fix` and `thrax explain`.

use std::path::Path;
use std::process::ExitCode;

use utilities::{Applicability, Code, Edit, Span};

/// The explanation of every diagnostic code, one `## CODE` section each.
const EXPLANATIONS: &str = include_str!("../../../documentation/errors.md");

fn style(path: &Path) -> frontend::format::Style {
    let config = crate::driver::project_options(path.parent().unwrap_or(Path::new("")));
    frontend::format::Style {
        width: config.int("format", "width") as usize,
        indent: config.int("format", "indent") as usize,
    }
}

/// `thrax fmt [--check] FILE...`: format files in place, or with `--check`
/// only report the ones that are not formatted (and fail if any).
pub fn cmd_fmt(args: &[String]) -> ExitCode {
    let check = args.iter().any(|a| a == "--check");
    let files: Vec<&String> = args.iter().filter(|a| !a.starts_with("--")).collect();
    if files.is_empty() {
        eprintln!("thrax: `fmt` needs at least one file");
        return ExitCode::FAILURE;
    }
    let mut failed = false;
    for file in files {
        let path = Path::new(file);
        let src = match std::fs::read_to_string(path) {
            Ok(s) => s,
            Err(e) => {
                eprintln!("thrax: cannot read {file}: {e}");
                failed = true;
                continue;
            }
        };
        match frontend::format::format_source(&src, &style(path)) {
            Ok(out) if out == src => {}
            Ok(out) if check => {
                println!("{file}: not formatted");
                failed = true;
                let _ = out;
            }
            Ok(out) => {
                if let Err(e) = std::fs::write(path, out) {
                    eprintln!("thrax: cannot write {file}: {e}");
                    failed = true;
                }
            }
            Err(msg) => {
                eprintln!("thrax: {file}: {msg}");
                failed = true;
            }
        }
    }
    if failed {
        ExitCode::FAILURE
    } else {
        ExitCode::SUCCESS
    }
}

/// The most rounds `thrax fix` runs: a fix can uncover the next error.
const FIX_ROUNDS: usize = 8;

/// `thrax fix FILE`: apply every fix the compiler is sure of
/// (machine-applicable), recheck, and repeat until none is left.
pub fn cmd_fix(path: &str) -> ExitCode {
    let root = Path::new(path).canonicalize().unwrap_or_else(|_| Path::new(path).to_path_buf());
    let mut applied = 0;
    for _ in 0..FIX_ROUNDS {
        let snap = match crate::lsp::analysis::analyze(&root, &Default::default()) {
            Ok(s) => s,
            Err(msg) => {
                eprintln!("thrax: {msg}");
                return ExitCode::FAILURE;
            }
        };
        let module = snap.module_of(&root).unwrap_or(snap.root);
        let src = snap.sources[module].2.clone();
        let mut edits: Vec<Edit> = Vec::new();
        let mut titles = Vec::new();
        for f in snap.found.iter().filter(|f| f.module == module) {
            for h in f.diag.helps() {
                let Some(es) = h.located_edits() else { continue };
                let clash = es.iter().any(|e| edits.iter().any(|t| overlaps(t.span, e.span)));
                if h.applicability == Applicability::MachineApplicable && !clash {
                    edits.extend(es.iter().cloned());
                    titles.push(h.msg.clone());
                }
            }
        }
        if edits.is_empty() {
            break;
        }
        let fixed = utilities::error::apply_edits(&src, &edits);
        if let Err(e) = std::fs::write(&root, &fixed) {
            eprintln!("thrax: cannot write {path}: {e}");
            return ExitCode::FAILURE;
        }
        for t in titles {
            println!("fixed: {t}");
        }
        applied += edits.len();
    }
    println!("thrax: applied {applied} edit(s) to {path}");
    ExitCode::SUCCESS
}

fn overlaps(a: Span, b: Span) -> bool {
    (a.start < b.end && b.start < a.end) || a == b
}

/// The `## CODE` section of the error index, if there is one.
pub fn explanation(code: &str) -> Option<&'static str> {
    let marker = format!("## {code}\n");
    let start = EXPLANATIONS.find(&marker)? + marker.len();
    let end = EXPLANATIONS[start..].find("\n## ").map_or(EXPLANATIONS.len(), |i| start + i);
    Some(EXPLANATIONS[start..end].trim())
}

/// `thrax explain CODE`: what a diagnostic code means and how to fix it.
pub fn cmd_explain(code: Option<&str>) -> ExitCode {
    let Some(code) = code else {
        eprintln!("thrax: `explain` needs a code, e.g. `thrax explain TYPE_MISMATCH`");
        return ExitCode::FAILURE;
    };
    let code = code.trim_start_matches("error[").trim_end_matches(']').to_ascii_uppercase();
    match explanation(&code) {
        Some(text) => {
            println!("{code}\n\n{text}");
            ExitCode::SUCCESS
        }
        None => {
            let known = ALL_CODES.iter().map(|c| c.name());
            match utilities::error::closest(&code, known) {
                Some(near) => eprintln!("thrax: no diagnostic code `{code}`; did you mean `{near}`?"),
                None => eprintln!("thrax: no diagnostic code `{code}`"),
            }
            ExitCode::FAILURE
        }
    }
}

/// Every code, for `explain`'s did-you-mean.
pub const ALL_CODES: &[Code] = &[
    Code::AsciiControlChar,
    Code::NonAsciiChar,
    Code::IllegalReservedChar,
    Code::UnclosedQuote,
    Code::InvalidEscape,
    Code::InvalidUtf8,
    Code::NumberParsingFailure,
    Code::UnknownSymbol,
    Code::ExpectedOperand,
    Code::UnexpectedToken,
    Code::UnbalancedParenthesis,
    Code::ExpectedGlobal,
    Code::Unsupported,
    Code::TypeMismatch,
    Code::TypeUnbound,
    Code::TypeAnnotationRequired,
    Code::TypeCycle,
    Code::BadModuleName,
    Code::FilenameMismatch,
    Code::AmbiguousName,
    Code::DuplicateSymbol,
    Code::UnknownModule,
    Code::PrivateSymbol,
    Code::NoEntry,
    Code::EntrySignature,
    Code::MetaAbort,
    Code::IntLiteralRange,
    Code::NonExhaustiveMatch,
    Code::UnreachableArm,
    Code::RuntimeFault,
    Code::ExpansionLimit,
    Code::TypedHole,
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_code_is_explained() {
        for c in ALL_CODES {
            assert!(explanation(c.name()).is_some(), "documentation/errors.md has no `## {}`", c.name());
        }
    }
}
