/**
 * Flash-timing constants, declared once and imported by every consumer so the
 * Rust and TypeScript schedules cannot drift apart.
 *
 * The authoritative values all live in `src-tauri/src/core/timing.rs`. This
 * module mirrors them for the browser; `flashTiming.test.ts` and the Rust timing
 * tests pin the same numbers on both sides, and `wasmParity.test.ts` exercises
 * the WASM planner that carries them.
 */

/** Number of countdown ticks. Rust: `COUNTDOWN_FROM` in `core/timing.rs`. */
export const COUNTDOWN_FROM = 3;

/** The displayed countdown values, counting down from `COUNTDOWN_FROM`. */
export const COUNTDOWN_TICKS = [3, 2, 1] as const;

/** Length of each countdown tick in milliseconds. Rust: `COUNTDOWN_TICK_MS`. */
export const COUNTDOWN_TICK_MS = 1000;

/**
 * Pre-flash pause after the final countdown tick before the first number. Rust:
 * `PRE_FLASH_SETTLE_MS`. It must not extend the first number's exposure.
 */
export const PRE_FLASH_SETTLE_MS = 100;

/** Fixed blank gap between consecutive numbers. Rust: `INTER_NUMBER_GAP_MS`. */
export const INTER_NUMBER_GAP_MS = 100;
