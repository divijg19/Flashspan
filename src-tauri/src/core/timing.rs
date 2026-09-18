//! Authoritative flash-timing budget.
//!
//! Every executor (native session worker, browser scheduler) must realize
//! exactly this schedule. The browser runtime mirrors these values in
//! `src/runtime/browser.ts` (`PRE_FLASH_SETTLE_MS`, `INTER_NUMBER_GAP_MS`);
//! changing any value here requires changing the mirror and updating the
//! schedule tests on both sides (`engine.rs` timing tests,
//! `src/__tests__/flashTiming.test.ts`).
//!
//! ```text
//! countdown("3") --1000ms--> countdown("2") --1000ms--> countdown("1")
//!     --1000ms + PRE_FLASH_SETTLE_MS (100ms)--> show(1)
//!     --number_duration_ms--> clear(1)
//!     --INTER_NUMBER_GAP_MS (100ms)--> show(2) --> ...
//!     --INTER_NUMBER_GAP_MS (100ms)--> complete
//! ```
//!
//! Invariants:
//! - I1: every `show(i) -> clear(i)` equals the configured number duration,
//!   including the first flash (no first-flash bonus).
//! - I2: every `clear(i) -> show(i+1)` is exactly INTER_NUMBER_GAP_MS, so no
//!   two events share a timestamp and a blank frame always separates numbers.
//! - I3: the pre-first-flash settle is a pause *before* exposure, never an
//!   extension of it.

/// Pause after the final countdown tick, before the first flash (ms).
/// Absorbs the fullscreen-transition grace so the first number paints late
/// rather than short.
pub const PRE_FLASH_SETTLE_MS: u64 = 100;

/// Fixed blank gap between a clear and the next flash (ms).
pub const INTER_NUMBER_GAP_MS: u64 = 100;
