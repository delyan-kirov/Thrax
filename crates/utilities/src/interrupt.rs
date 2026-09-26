//! The process-global interrupt request: the one place a signal handler and a
//! long-running evaluation meet.
//!
//! A `SIGINT` handler cannot do much safely, so it only sets this flag, and the
//! evaluator polls it on its back edges and stops when it is set. The flag is a
//! plain `AtomicBool`, whose store and load are async-signal-safe, so [`request`]
//! may be called from a handler. Being process-global, it is also the reason the
//! interrupt tests live in their own test binary.

use std::sync::atomic::{AtomicBool, Ordering};

static REQUESTED: AtomicBool = AtomicBool::new(false);

/// Ask whatever is currently evaluating to stop. Safe to call from a signal
/// handler.
pub fn request() {
    REQUESTED.store(true, Ordering::Relaxed);
}

/// Whether a stop has been asked for and not yet cleared.
pub fn requested() -> bool {
    REQUESTED.load(Ordering::Relaxed)
}

/// Clear the request, reporting whether one was pending.
pub fn take() -> bool {
    REQUESTED.swap(false, Ordering::Relaxed)
}
