//! Interruptible evaluation: a pending interrupt request stops the machine.
//!
//! This lives in its own test binary, as a single test, because the request flag
//! is process-global (a signal handler has nowhere else to put it), so setting it
//! would stop evaluations running concurrently in the same process.

use frontend::{lower_program, Checker, Decls};

/// Parse, check and lower one module to the IR. CORE is not injected, so the
/// module must stick to intrinsics.
fn lower(src: &str) -> frontend::ir::data::Program {
    let parsed = frontend::parse(src).expect("parse");
    let mut checker = Checker::new(&parsed.ast);
    checker
        .check_program(&parsed.program)
        .unwrap_or_else(|e| panic!("{}", e.render(src, "T")));
    let resolved = frontend::collect_resolved(std::slice::from_ref(&checker));
    let decls = Decls::collect(&parsed.ast, std::slice::from_ref(&parsed.program));
    let lowered = lower_program(&parsed.ast, &parsed.program, &decls, &resolved);
    frontend::ir::lower_modules(std::slice::from_ref(&lowered))
}

#[test]
fn a_pending_request_stops_a_runaway_evaluation() {
    // A non-terminating global: without the step loop's interrupt poll, forcing it
    // would never return.
    let spin = lower(
        "@mod T\n\
         $ spin : @int -> @int = \\n = spin (@iadd n 1)\n\
         $ test : @int = spin 0\n",
    );
    utilities::interrupt::request();
    let diag = interpreter::machine::eval(&spin, "test").expect_err("the spin must be stopped");
    assert!(diag.render("", "T").contains("interrupted"));

    // Taking the request clears it, so the next evaluation runs to completion.
    assert!(utilities::interrupt::take());
    assert!(!utilities::interrupt::requested());
    let sum = lower("@mod T\n$ test : @int = @iadd 1 2\n");
    assert_eq!(
        interpreter::machine::eval(&sum, "test").expect("no request pending"),
        "3"
    );
}
