use super::types::{SessionConfig, SessionConfigEffective, SessionConfigInput};

fn round_1_decimal(v: f64) -> f64 {
    (v * 10.0).round() / 10.0
}

fn clamp_f64(v: f64, min: f64, max: f64) -> f64 {
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

fn seconds_to_ms_clamped(seconds: f64, min_ms: u64, max_ms: u64) -> u64 {
    let ms = (seconds * 1000.0).round();
    if !ms.is_finite() {
        return min_ms;
    }
    let ms_u64 = if ms <= 0.0 { 0 } else { ms as u64 };
    ms_u64.max(min_ms).min(max_ms)
}

/// Largest integer exactly representable in an f64 (2^53 - 1).
/// The browser runtime accumulates sums in f64, so the worst-case session
/// sum must stay below this for grading to be exact on every platform.
pub const MAX_EXACT_INTEGER: u64 = (1u64 << 53) - 1;

/// Absolute ceiling for session length regardless of digit width.
pub const MAX_TOTAL_NUMBERS: u32 = 10_000;

/// Maximum numbers allowed for a digit width such that even the worst case
/// (every number the maximum magnitude) sums to less than MAX_EXACT_INTEGER.
/// Since 2^53 is far below i64::MAX, this also guarantees the sum fits in i64.
pub fn max_total_for_digits(digits: u32) -> u32 {
    let max_magnitude: u128 = if digits <= 1 {
        9
    } else {
        10u128.pow(digits) - 1
    };
    let bound = (MAX_EXACT_INTEGER as u128 / max_magnitude).min(MAX_TOTAL_NUMBERS as u128);
    bound.max(1) as u32
}

pub fn normalize_session_config(
    input: SessionConfigInput,
) -> (SessionConfig, SessionConfigEffective) {
    // Policy cap: 15 digits keeps every individual value exactly
    // representable in f64 (10^15 - 1 < 2^53 - 1). The generator itself
    // remains u64-safe to 18 digits; only normalized sessions are capped.
    let digits = clamp_i64(input.digits_per_number, 1, 15) as u32;
    let total_numbers =
        clamp_i64(input.total_numbers, 1, 10_000).min(max_total_for_digits(digits) as i64) as u32;

    // UI typically uses 0.1–5s, but we allow up to 60s defensively.
    let duration_s = clamp_f64(input.number_duration_s, 0.1, 60.0);

    let number_duration_ms = seconds_to_ms_clamped(duration_s, 1, 60_000);

    let config = SessionConfig {
        digits_per_number: digits,
        number_duration_ms,
        total_numbers,
        allow_negative_numbers: input.allow_negative_numbers,
    };

    // The inter-number gap is fixed (see crate::core::timing) and no longer
    // part of the config wire format.
    let effective = SessionConfigEffective {
        digits_per_number: config.digits_per_number,
        number_duration_s: round_1_decimal(config.number_duration_ms as f64 / 1000.0),
        total_numbers: config.total_numbers,
        allow_negative_numbers: config.allow_negative_numbers,
    };

    (config, effective)
}

pub fn validate_config(config: &SessionConfig) -> Result<(), String> {
    if config.digits_per_number == 0 || config.number_duration_ms == 0 || config.total_numbers == 0
    {
        return Err(
            "digits_per_number, number_duration_ms, and total_numbers must be > 0".to_string(),
        );
    }

    // Keep generation simple and safe: normalized sessions cap digits at 15
    // so values stay exactly representable in f64 (see MAX_EXACT_INTEGER).
    if config.digits_per_number == 0 || config.digits_per_number > 15 {
        return Err("digits_per_number must be between 1 and 15".to_string());
    }

    // Defensive caps: UI enforces ranges, but IPC inputs must be treated as untrusted.
    // These limits are generous enough for real use while preventing accidental runaway sessions.
    if config.total_numbers > 10_000 {
        return Err("total_numbers must be <= 10000".to_string());
    }

    // Worst-case sum must stay exactly representable (see MAX_EXACT_INTEGER).
    if config.total_numbers > max_total_for_digits(config.digits_per_number) {
        return Err("total_numbers too large for the digit width".to_string());
    }

    if config.number_duration_ms > 60_000 {
        return Err("number_duration_ms must be <= 60000".to_string());
    }

    Ok(())
}
