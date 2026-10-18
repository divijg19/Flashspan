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

#[cfg(test)]
mod tests {
    // These pin the contract the whole app leans on: normalization output must
    // always validate, the reported duration must be *exactly* the exposure the
    // executor runs, and the digit-width bound must stay correct for widths the
    // UI cannot reach (`max_total_for_digits` remains `pub` and is the floor
    // `normalize` applies, so a wrong value silently truncates long sessions).
    //
    // They run on wasm too: the browser calls `normalize_session_config_wasm`
    // on every session start, so a plain `#[test]` would be silently skipped on
    // the one platform that depends on it.
    #[cfg(target_arch = "wasm32")]
    use wasm_bindgen_test::wasm_bindgen_test;

    use super::{
        MAX_DURATION_MS, MAX_EXACT_INTEGER, MAX_TOTAL_NUMBERS, effective_from,
        max_total_for_digits, normalize_session_config, validate_config,
    };
    use crate::core::types::{SessionConfig, SessionConfigInput};

    /// A session input; tests vary the one field under test.
    fn input(digits: i64, duration_s: f64, total: i64, negatives: bool) -> SessionConfigInput {
        SessionConfigInput {
            digits_per_number: digits,
            number_duration_s: duration_s,
            total_numbers: total,
            allow_negative_numbers: negatives,
        }
    }

    /// The floor of the exact-integer budget for a width, without the `10_000` cap.
    fn exact_budget(digits: u32) -> u128 {
        let max_magnitude = if digits <= 1 {
            9
        } else {
            10u128.pow(digits) - 1
        };
        u128::from(MAX_EXACT_INTEGER) / max_magnitude
    }

    /// The bound is `10_000` for every width whose worst case fits the budget, and
    /// collapses toward 1 once the width's maximum magnitude alone exceeds it.
    /// The first half of that range (12..=15) is inside the UI's reach; the rest
    /// is not, which is exactly why it went untested.
    #[cfg_attr(target_arch = "wasm32", wasm_bindgen_test)]
    #[cfg_attr(not(target_arch = "wasm32"), test)]
    fn digit_width_bound_collapses_to_one_once_the_width_leaves_the_exact_budget() {
        for digits in 16u32..=38 {
            assert_eq!(
                max_total_for_digits(digits),
                1,
                "a {digits}-digit magnitude is larger than the f64-exact budget, \
                 so even one number can overflow it",
            );
        }
    }

    /// Past 38 digits `10^digits` no longer fits `u128` at all. The
    /// `checked_pow` fallback maps that to `u128::MAX`, and the bound must still
    /// come out as the floor of 1 rather than panicking or wrapping.
    #[cfg_attr(target_arch = "wasm32", wasm_bindgen_test)]
    #[cfg_attr(not(target_arch = "wasm32"), test)]
    fn digit_width_bound_collapses_to_one_when_the_power_leaves_u128() {
        // 10^39 is the first power past `u128::MAX`, so this is the boundary
        // between the two collapse paths.
        assert!(10u128.checked_pow(38).is_some());
        assert!(10u128.checked_pow(39).is_none());
        for digits in [39u32, 40, 64, 100, u32::MAX] {
            assert_eq!(max_total_for_digits(digits), 1, "width {digits}");
        }
    }

    /// A wider number can never permit *more* of them: the bound is a
    /// non-increasing function of the digit width. This is the property that
    /// makes clamping to it monotone, so a session can never be truncated more
    /// than a session one digit wider.
    #[cfg_attr(target_arch = "wasm32", wasm_bindgen_test)]
    #[cfg_attr(not(target_arch = "wasm32"), test)]
    fn digit_width_bound_is_monotonic_non_increasing() {
        let mut previous = u32::MAX;
        for digits in 1u32..=64 {
            let bound = max_total_for_digits(digits);
            assert!(
                bound <= previous,
                "bound rose from {previous} to {bound} at width {digits}",
            );
            previous = bound;
        }
    }

    /// The bound is a *permission*, never an obligation: it is always at least
    /// 1 (so every normalized config admits a session) and never exceeds the
    /// global ceiling.
    #[cfg_attr(target_arch = "wasm32", wasm_bindgen_test)]
    #[cfg_attr(not(target_arch = "wasm32"), test)]
    fn digit_width_bound_stays_inside_the_global_ceiling() {
        for digits in 1u32..=64 {
            let bound = max_total_for_digits(digits);
            assert!(
                (1..=MAX_TOTAL_NUMBERS).contains(&bound),
                "width {digits} -> {bound}"
            );
        }
    }

    /// Where the width bound actually bites (12..=15) it must equal the floor of
    /// the exact-integer budget, derived here from the constants rather than
    /// copied from a literal table — so this cannot drift alongside a bug in
    /// `max_total_for_digits` the way a hardcoded table would.
    #[cfg_attr(target_arch = "wasm32", wasm_bindgen_test)]
    #[cfg_attr(not(target_arch = "wasm32"), test)]
    fn digit_width_bound_equals_the_floor_of_the_exact_budget() {
        for digits in 1u32..=15 {
            let expected = exact_budget(digits).min(u128::from(MAX_TOTAL_NUMBERS));
            let expected = u32::try_from(expected).expect("budget floor fits u32");
            assert_eq!(max_total_for_digits(digits), expected, "width {digits}");
        }
    }

    /// The reason `normalize` snaps to the grid before quantizing: every step of
    /// the UI's duration control must yield `number_duration_ms` exactly equal
    /// to `number_duration_s * 1000`. If they could disagree, the app would
    /// report one exposure and flash another — the class of bug this project has
    /// already shipped once.
    #[allow(clippy::float_cmp)] // Exactness lock: the relation is bit-exact by construction.
    #[cfg_attr(target_arch = "wasm32", wasm_bindgen_test)]
    #[cfg_attr(not(target_arch = "wasm32"), test)]
    fn reported_seconds_are_exactly_the_executed_milliseconds_on_every_grid_step() {
        for tenths in 1u32..=600 {
            let duration_s = f64::from(tenths) / 10.0;
            let (config, effective) = normalize_session_config(&input(2, duration_s, 5, false));

            assert_eq!(
                config.number_duration_ms,
                u64::from(tenths) * 100,
                "duration {duration_s}s must execute as {}ms",
                u64::from(tenths) * 100,
            );
            assert_eq!(
                effective.number_duration_s, duration_s,
                "the reported seconds must round-trip for {duration_s}s",
            );
        }
    }

    /// Duration clamping and snapping at and around the edges. The half-way
    /// values pin the rounding *direction*: `round_1_decimal` uses `f64::round`
    /// (half away from zero), which is also JS `Math.round`, so the browser and
    /// desktop snap `0.15` to the same `0.2`.
    #[allow(clippy::float_cmp)] // Exactness lock on the snapped seconds.
    #[cfg_attr(target_arch = "wasm32", wasm_bindgen_test)]
    #[cfg_attr(not(target_arch = "wasm32"), test)]
    fn duration_snaps_to_the_nearest_tenth_and_clamps_at_the_edges() {
        for (requested, expected_ms) in [
            (0.0, 100),   // below the 0.1s floor -> shortest allowed step
            (0.05, 100),  // still below the floor
            (0.1, 100),   // exactly the floor
            (0.15, 200),  // half-way: rounds away from zero
            (0.25, 300),  // half-way again
            (0.99, 1000), // just under a whole second
            (1.0, 1000),
            (1.04, 1000),    // rounds down
            (1.06, 1100),    // rounds up
            (59.94, 59_900), // rounds down at the top
            (60.0, 60_000),  // exactly the ceiling
            (60.1, 60_000),  // above the ceiling -> clamped
            (1_000.0, 60_000),
        ] {
            let (config, effective) = normalize_session_config(&input(1, requested, 5, false));
            assert_eq!(
                config.number_duration_ms, expected_ms,
                "requested {requested}s should execute as {expected_ms}ms",
            );
            assert_eq!(
                effective.number_duration_s,
                f64::from(u32::try_from(expected_ms / 100).expect("tenths")) / 10.0,
                "the reported seconds must be the snapped step for {requested}s",
            );
        }
        // The ceiling above is the same value `validate_config` enforces.
        assert_eq!(MAX_DURATION_MS, 60_000);
    }

    /// The worker validates the *normalized* config before it starts. If
    /// normalization could ever produce something `validate_config` rejects, a
    /// perfectly reasonable request would fail with an internal error. Sweep the
    /// clamp boundaries so the two functions cannot disagree.
    #[cfg_attr(target_arch = "wasm32", wasm_bindgen_test)]
    #[cfg_attr(not(target_arch = "wasm32"), test)]
    fn normalize_always_produces_a_config_that_validates() {
        for (digits, duration_s, total, negatives) in [
            (1i64, 0.1, 1i64, false),
            (15, 60.0, 100, true),
            (0, 0.0, 0, false),
            (99, f64::NAN, 99_999, true),
            (12, 1.234, 100_000, false),
            (-5, f64::INFINITY, -3, true),
            (i64::MIN, f64::NEG_INFINITY, i64::MAX, false),
        ] {
            let (config, _effective) =
                normalize_session_config(&input(digits, duration_s, total, negatives));
            assert!(
                validate_config(&config).is_ok(),
                "normalized {config:?} (from {digits}/{duration_s}/{total}/{negatives}) must validate",
            );
        }
    }

    /// The two total-related rejections name different things. A total past the
    /// *width* bound is a session that would overflow the f64-exact budget; a
    /// total past the *hard cap* is merely absurd. The messages must stay
    /// distinct so a caller can tell them apart.
    #[cfg_attr(target_arch = "wasm32", wasm_bindgen_test)]
    #[cfg_attr(not(target_arch = "wasm32"), test)]
    fn validate_names_the_digit_width_bound_when_the_total_exceeds_it() {
        // 12 digits caps the total at 9007, comfortably below the 10_000 hard
        // cap, so the width bound is the one that fires.
        let bound = max_total_for_digits(12);
        assert!(
            bound < MAX_TOTAL_NUMBERS,
            "the test needs the width bound to bite"
        );

        let over = SessionConfig {
            digits_per_number: 12,
            number_duration_ms: 500,
            total_numbers: bound + 1,
            allow_negative_numbers: false,
        };
        assert_eq!(
            validate_config(&over).expect_err("one past the width bound must be rejected"),
            "total_numbers too large for the digit width",
        );

        // Exactly at the bound is accepted.
        let at = SessionConfig {
            total_numbers: bound,
            ..over
        };
        assert!(
            validate_config(&at).is_ok(),
            "the bound itself is inclusive"
        );
    }

    #[cfg_attr(target_arch = "wasm32", wasm_bindgen_test)]
    #[cfg_attr(not(target_arch = "wasm32"), test)]
    fn validate_names_the_hard_cap_when_only_it_is_exceeded() {
        // A single digit admits the whole 10_000, so the hard cap is the only
        // bound in play for one past it.
        let config = SessionConfig {
            digits_per_number: 1,
            number_duration_ms: 500,
            total_numbers: MAX_TOTAL_NUMBERS + 1,
            allow_negative_numbers: false,
        };
        assert_eq!(
            validate_config(&config).expect_err("past the hard cap"),
            format!("total_numbers must be <= {MAX_TOTAL_NUMBERS}"),
        );
    }

    /// `effective_from` is the single place the seconds view is derived, shared
    /// by normalization and the session worker. It must copy the discrete
    /// fields verbatim and only translate the duration.
    #[allow(clippy::float_cmp)] // Exactness lock on the tenths conversion.
    #[cfg_attr(target_arch = "wasm32", wasm_bindgen_test)]
    #[cfg_attr(not(target_arch = "wasm32"), test)]
    fn effective_from_reports_the_configured_fields_verbatim() {
        let config = SessionConfig {
            digits_per_number: 4,
            number_duration_ms: 2300,
            total_numbers: 12,
            allow_negative_numbers: true,
        };
        let effective = effective_from(&config);

        assert_eq!(effective.digits_per_number, 4);
        assert_eq!(effective.total_numbers, 12);
        assert!(effective.allow_negative_numbers);
        assert_eq!(effective.number_duration_s, 2.3);
    }

    /// Every duration the executor can actually hold (a whole number of
    /// deciseconds, 0.1s..=60s) must survive the trip through the reported
    /// seconds unchanged. This is the inverse direction of the grid-step test
    /// above, and it is what lets the UI echo `number_duration_s` straight back.
    #[allow(clippy::float_cmp)] // Exactness lock on the tenths conversion.
    #[cfg_attr(target_arch = "wasm32", wasm_bindgen_test)]
    #[cfg_attr(not(target_arch = "wasm32"), test)]
    fn every_executable_duration_round_trips_through_the_reported_seconds() {
        for ms in (100u64..=60_000).step_by(100) {
            let config = SessionConfig {
                digits_per_number: 2,
                number_duration_ms: ms,
                total_numbers: 5,
                allow_negative_numbers: false,
            };
            assert_eq!(
                effective_from(&config).number_duration_s,
                f64::from(u32::try_from(ms / 100).expect("tenths fit u32")) / 10.0,
                "{ms}ms must report as {}.{}s",
                ms / 1000,
                (ms / 100) % 10,
            );
        }
    }
}
