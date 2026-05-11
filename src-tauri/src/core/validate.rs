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

pub fn normalize_session_config(
    input: SessionConfigInput,
) -> (SessionConfig, SessionConfigEffective) {
    let digits = clamp_i64(input.digits_per_number, 1, 18) as u32;
    let total_numbers = clamp_i64(input.total_numbers, 1, 10_000) as u32;

    // UI typically uses 0.1–5s, but we allow up to 60s defensively.
    let duration_s = clamp_f64(input.number_duration_s, 0.1, 60.0);
    let delay_s = clamp_f64(input.delay_between_numbers_s, 0.0, 60.0);

    let number_duration_ms = seconds_to_ms_clamped(duration_s, 1, 60_000);
    let delay_between_numbers_ms = seconds_to_ms_clamped(delay_s, 0, 60_000);

    let config = SessionConfig {
        digits_per_number: digits,
        number_duration_ms,
        delay_between_numbers_ms,
        total_numbers,
        allow_negative_numbers: input.allow_negative_numbers,
    };

    let effective = SessionConfigEffective {
        digits_per_number: config.digits_per_number,
        number_duration_s: round_1_decimal(config.number_duration_ms as f64 / 1000.0),
        delay_between_numbers_s: round_1_decimal(config.delay_between_numbers_ms as f64 / 1000.0),
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

    // Keep generation simple and safe: 10^digits must fit in u64.
    if config.digits_per_number > 18 {
        return Err("digits_per_number must be <= 18".to_string());
    }

    // Defensive caps: UI enforces ranges, but IPC inputs must be treated as untrusted.
    // These limits are generous enough for real use while preventing accidental runaway sessions.
    if config.total_numbers > 10_000 {
        return Err("total_numbers must be <= 10000".to_string());
    }

    if config.number_duration_ms > 60_000 {
        return Err("number_duration_ms must be <= 60000".to_string());
    }

    if config.delay_between_numbers_ms > 60_000 {
        return Err("delay_between_numbers_ms must be <= 60000".to_string());
    }

    Ok(())
}
