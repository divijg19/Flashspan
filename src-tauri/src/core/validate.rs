use super::types::{SessionConfig, SessionConfigEffective, SessionConfigInput};

/// Policy cap on digit width. 15 digits keeps every individual value exactly
/// representable in f64 (10^15 - 1 < 2^53 - 1). The generator itself remains
/// u64-safe to 19 digits; only normalized sessions are capped.
pub const MAX_DIGITS_PER_NUMBER: u32 = 15;

/// Shortest and longest exposure per number (s).
const MIN_DURATION_S: f64 = 0.1;
const MAX_DURATION_S: f64 = 60.0;

/// Longest exposure per number (ms), i.e. `MAX_DURATION_S` in milliseconds.
pub const MAX_DURATION_MS: u64 = 60_000;

/// Number of decimals the duration control exposes (and therefore the grid
/// every normalized duration snaps to). Mirrored by the UI's `step="0.1"` and
/// by `round1` in `src/runtime/browser.ts`.
const DURATION_DECIMALS: f64 = 10.0;

fn round_1_decimal(v: f64) -> f64 {
    (v * DURATION_DECIMALS).round() / DURATION_DECIMALS
}

const fn clamp_f64(v: f64, min: f64, max: f64) -> f64 {
    if v.is_nan() {
        return min;
    }
    if v.is_infinite() {
        return if v.is_sign_positive() { max } else { min };
    }
    v.max(min).min(max)
}

fn clamp_i64(v: i64, min: i64, max: i64) -> i64 {
    v.max(min).min(max)
}

/// Tenths of a second for a grid-aligned duration.
///
/// `normalize_session_config` snaps the input to the 0.1s grid *before* this
/// runs, so the result is a small exact integer and no float round-trip is
/// involved: `number_duration_ms == effective.number_duration_s * 1000` holds
/// by construction. The assertion below is therefore unreachable for every
/// reachable input; the `as u64` relies on it rather than on a comment
/// (`From` has no f64 impl).
#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
fn deciseconds_of(duration_s: f64) -> u64 {
    let tenths = (duration_s * DURATION_DECIMALS).round();
    debug_assert!(
        (1.0..=600.0).contains(&tenths),
        "duration_s {duration_s} is not grid-aligned"
    );
    tenths as u64
}

/// Largest integer exactly representable in an f64 (2^53 - 1).
/// The browser runtime accumulates sums in f64, so the worst-case session
/// sum must stay below this for grading to be exact on every platform.
pub const MAX_EXACT_INTEGER: u64 = (1u64 << 53) - 1;

/// Absolute ceiling for session length regardless of digit width.
pub const MAX_TOTAL_NUMBERS: u32 = 10_000;

/// Maximum numbers allowed for a digit width such that even the worst case
/// (every number the maximum magnitude) sums to less than
/// `MAX_EXACT_INTEGER`.
///
/// Since 2^53 is far below `i64::MAX`, this also guarantees the sum fits in i64.
/// Widths whose magnitude leaves `u128` (39+ digits) cannot be represented at
/// all, so their bound collapses to the floor of 1; normalized sessions cap
/// digits at `MAX_DIGITS_PER_NUMBER` long before that.
///
/// # Panics
///
/// Panics only if `u32` could not hold the result, which is impossible: the
/// `bound` never exceeds `MAX_TOTAL_NUMBERS` (`10_000`).
#[must_use]
pub fn max_total_for_digits(digits: u32) -> u32 {
    let max_magnitude: u128 = if digits <= 1 {
        9
    } else {
        10u128.checked_pow(digits).map_or(u128::MAX, |p| p - 1)
    };
    let bound = (u128::from(MAX_EXACT_INTEGER) / max_magnitude).min(u128::from(MAX_TOTAL_NUMBERS));
    u32::try_from(bound.max(1)).expect("bound fits u32 by construction")
}

/// The effective (reported) view of a normalized config.
///
/// Single source of the seconds/milliseconds relation: normalization and the
/// session worker both build the snapshot through this function, so a plan's
/// `config_snapshot` can never disagree with the `effective_config` the UI has
/// already been told.
///
/// The `as f64` is exact for every duration normalization produces (see
/// `deciseconds_of`); `From` has no u64 impl.
#[allow(clippy::cast_precision_loss)]
#[must_use]
pub fn effective_from(config: &SessionConfig) -> SessionConfigEffective {
    SessionConfigEffective {
        digits_per_number: config.digits_per_number,
        number_duration_s: round_1_decimal(config.number_duration_ms as f64 / 1000.0),
        total_numbers: config.total_numbers,
        allow_negative_numbers: config.allow_negative_numbers,
    }
}

/// # Panics
///
/// Panics only if a clamped value could not fit its target integer type,
/// which is impossible: every clamp range lies far inside the target range.
#[must_use]
pub fn normalize_session_config(
    input: &SessionConfigInput,
) -> (SessionConfig, SessionConfigEffective) {
    let digits = u32::try_from(clamp_i64(
        input.digits_per_number,
        1,
        i64::from(MAX_DIGITS_PER_NUMBER),
    ))
    .expect("clamped 1..=15 fits u32");
    let total_numbers = u32::try_from(
        clamp_i64(input.total_numbers, 1, i64::from(MAX_TOTAL_NUMBERS))
            .min(i64::from(max_total_for_digits(digits))),
    )
    .expect("clamped total within digit-width bound fits u32");

    // Snap to the reported grid first, then quantize to whole milliseconds.
    // The UI control steps by 0.1s and the browser runtime normalizes in the
    // same order (`round1(clamp(s))`, then `s * 1000`), so with this order
    // the exposure a number actually gets is exactly the duration the app
    // reports, on every runtime and for every input.
    let duration_s = round_1_decimal(clamp_f64(
        input.number_duration_s,
        MIN_DURATION_S,
        MAX_DURATION_S,
    ));
    let number_duration_ms = deciseconds_of(duration_s) * 100;

    let config = SessionConfig {
        digits_per_number: digits,
        number_duration_ms,
        total_numbers,
        allow_negative_numbers: input.allow_negative_numbers,
    };

    // The inter-number gap is fixed (see crate::core::timing) and no longer
    // part of the config wire format.
    let effective = effective_from(&config);

    (config, effective)
}

/// # Errors
///
/// Returns an error describing the first violated bound (zero digits,
/// duration, or totals; digits above 15; totals above `10_000` or past the
/// digit-width exact-integer `bound`; durations above `60s`).
pub fn validate_config(config: &SessionConfig) -> Result<(), String> {
    if config.digits_per_number == 0 || config.number_duration_ms == 0 || config.total_numbers == 0
    {
        return Err(
            "digits_per_number, number_duration_ms, and total_numbers must be > 0".to_string(),
        );
    }

    // Keep generation simple and safe: normalized sessions cap digits at 15
    // so values stay exactly representable in f64 (see MAX_EXACT_INTEGER).
    if config.digits_per_number > MAX_DIGITS_PER_NUMBER {
        return Err(format!(
            "digits_per_number must be between 1 and {MAX_DIGITS_PER_NUMBER}"
        ));
    }

    // Defensive caps: UI enforces ranges, but IPC inputs must be treated as untrusted.
    // These limits are generous enough for real use while preventing accidental runaway sessions.
    if config.total_numbers > MAX_TOTAL_NUMBERS {
        return Err(format!("total_numbers must be <= {MAX_TOTAL_NUMBERS}"));
    }

    // Worst-case sum must stay exactly representable (see MAX_EXACT_INTEGER).
    if config.total_numbers > max_total_for_digits(config.digits_per_number) {
        return Err("total_numbers too large for the digit width".to_string());
    }

    if config.number_duration_ms > MAX_DURATION_MS {
        return Err(format!("number_duration_ms must be <= {MAX_DURATION_MS}"));
    }

    Ok(())
}
