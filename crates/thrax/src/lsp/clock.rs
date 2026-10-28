//! The server's monotonic clock: `std::time::Instant` wherever the standard
//! library has a clock (natively, and on WASI). On `wasm32-unknown-unknown` it
//! has none and `Instant::now` panics, so the embedder supplies milliseconds
//! through the `thx_host_now_ms` import, which a browser answers with
//! `performance.now()`.

#[cfg(not(all(target_arch = "wasm32", target_os = "unknown")))]
pub use std::time::Instant;

#[cfg(all(target_arch = "wasm32", target_os = "unknown"))]
pub use host::Instant;

#[cfg(all(target_arch = "wasm32", target_os = "unknown"))]
mod host {
    use std::time::Duration;

    extern "C" {
        fn thx_host_now_ms() -> f64;
    }

    #[derive(Clone, Copy, PartialEq, PartialOrd, Debug)]
    pub struct Instant(f64);

    // Host milliseconds are finite and monotonic, so the order is total.
    impl Eq for Instant {}

    impl Ord for Instant {
        fn cmp(&self, other: &Self) -> std::cmp::Ordering {
            self.0.total_cmp(&other.0)
        }
    }

    impl Instant {
        pub fn now() -> Instant {
            // SAFETY: a host import with no arguments and no memory access.
            Instant(unsafe { thx_host_now_ms() })
        }

        pub fn saturating_duration_since(&self, earlier: Instant) -> Duration {
            Duration::from_secs_f64(((self.0 - earlier.0) / 1000.0).max(0.0))
        }

        pub fn elapsed(&self) -> Duration {
            Instant::now().saturating_duration_since(*self)
        }
    }

    impl std::ops::Add<Duration> for Instant {
        type Output = Instant;
        fn add(self, d: Duration) -> Instant {
            Instant(self.0 + d.as_secs_f64() * 1000.0)
        }
    }
}
