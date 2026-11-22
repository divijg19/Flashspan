//! Authoritative flash-timing budget.
//!
//! Every executor (native session worker, browser scheduler) must realize
//! exactly this schedule. The browser mirrors these values in `src/timing.ts`
//! (a single module imported by both `src/runtime/browser.ts` and `src/App.tsx`);
//! changing any value here requires changing that module and updating the
//! schedule tests on both sides (`engine.rs` timing tests,
//! `src/__tests__/flashTiming.test.ts`).
//!
//! ```text
//! countdown("3") --1000ms--> countdown("2") --1000ms--> countdown("1")
//!     --1000ms + PRE_FLASH_SETTLE_MS (100ms)--> show(1)
//!     --number_duration_ms--> show(2) --> ...
//!     --number_duration_ms--> complete
//! ```
//!
//! Invariants:
//! - I1: every `show(i) -> show(i+1)` equals the configured number duration,
//!   including the first flash (no first-flash bonus).
//! - I2: numbers replace one another continuously: no blank frame, no extra
//!   delay, and no two events share a timestamp. This is the continuous
//!   replacement policy used by competitive flash-number presentations.
//! - I3: the pre-first-flash settle is a pause *before* exposure, never an
//!   extension of it.

/// Number of countdown ticks ("3", "2", "1").
pub const COUNTDOWN_FROM: u32 = 3;

/// Gap between consecutive countdown ticks (ms).
pub const COUNTDOWN_TICK_MS: u64 = 1000;

/// Pause after the final countdown tick, before the first flash (ms).
/// Absorbs the fullscreen-transition grace so the first number paints late
/// rather than short.
pub const PRE_FLASH_SETTLE_MS: u64 = 100;
