// SPDX-License-Identifier: GPL-3.0-or-later
//! A monotonic `Instant` for the decoders' deadlines and timing.
//!
//! Natively it is `std::time::Instant`. On `wasm32-unknown-unknown`
//! `std::time::Instant::now()` panics (the target has no clock), so there
//! it reads the page's `performance.now()` through an import the module's
//! host provides: `env.web888_now_ms() -> f64` (milliseconds, monotonic).

#[cfg(not(target_arch = "wasm32"))]
pub use std::time::Instant;

#[cfg(target_arch = "wasm32")]
pub use wasm::Instant;

#[cfg(target_arch = "wasm32")]
mod wasm {
    use std::ops::{Add, Sub};
    use std::time::Duration;

    #[link(wasm_import_module = "env")]
    unsafe extern "C" {
        fn web888_now_ms() -> f64;
    }

    /// Milliseconds on the host's monotonic clock.
    #[derive(Clone, Copy, Debug, PartialEq, PartialOrd)]
    pub struct Instant(f64);

    impl Instant {
        pub fn now() -> Self {
            // SAFETY: an imported function without arguments or memory.
            Instant(unsafe { web888_now_ms() })
        }

        pub fn duration_since(&self, earlier: Instant) -> Duration {
            Duration::from_secs_f64(((self.0 - earlier.0) / 1e3).max(0.0))
        }

        pub fn elapsed(&self) -> Duration {
            Instant::now().duration_since(*self)
        }
    }

    impl Add<Duration> for Instant {
        type Output = Instant;
        fn add(self, d: Duration) -> Instant {
            Instant(self.0 + d.as_secs_f64() * 1e3)
        }
    }

    impl Sub for Instant {
        type Output = Duration;
        fn sub(self, earlier: Instant) -> Duration {
            self.duration_since(earlier)
        }
    }
}
