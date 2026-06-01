// Property-based tests using proptest for determinism and bounds checking
use super::types::{SessionConfig, SessionConfigInput};
use super::validate::{normalize_session_config, validate_config};
use proptest::prelude::*;

#[test]
fn prop_normalize_digits_in_bounds() {
    proptest!(|(digits in 0i64..1000)| {
        let input = SessionConfigInput {
            digits_per_number: digits,
            number_duration_s: 1.0,
            delay_between_numbers_s: 0.0,
            total_numbers: 5,
            allow_negative_numbers: false,
        };
        let (config, _effective) = normalize_session_config(input);

        // Result should always be between 1 and 18
        prop_assert!(config.digits_per_number >= 1);
        prop_assert!(config.digits_per_number <= 18);
    });
}

#[test]
fn prop_normalize_total_numbers_in_bounds() {
    proptest!(|(total in 0i64..100_000)| {
        let input = SessionConfigInput {
            digits_per_number: 1,
            number_duration_s: 1.0,
            delay_between_numbers_s: 0.0,
            total_numbers: total,
            allow_negative_numbers: false,
        };
        let (config, _effective) = normalize_session_config(input);

        // Result should always be between 1 and 10_000
        prop_assert!(config.total_numbers >= 1);
        prop_assert!(config.total_numbers <= 10_000);
    });
}

#[test]
fn prop_normalize_duration_in_bounds() {
    proptest!(|(duration_s in 0.0_f64..1000.0)| {
        if !duration_s.is_finite() {
            return Ok(());
        }

        let input = SessionConfigInput {
            digits_per_number: 1,
            number_duration_s: duration_s,
            delay_between_numbers_s: 0.0,
            total_numbers: 5,
            allow_negative_numbers: false,
        };
        let (config, _effective) = normalize_session_config(input);

        // Result should always be between 1ms and 60_000ms
        prop_assert!(config.number_duration_ms >= 1);
        prop_assert!(config.number_duration_ms <= 60_000);
    });
}

#[test]
fn prop_normalize_delay_in_bounds() {
    proptest!(|(delay_s in -1000.0_f64..1000.0)| {
        if !delay_s.is_finite() {
            return Ok(());
        }

        let input = SessionConfigInput {
            digits_per_number: 1,
            number_duration_s: 1.0,
            delay_between_numbers_s: delay_s,
            total_numbers: 5,
            allow_negative_numbers: false,
        };
        let (config, _effective) = normalize_session_config(input);

        // Result should always be between 0ms and 60_000ms
        prop_assert!(config.delay_between_numbers_ms <= 60_000);
    });
}

#[test]
fn prop_normalize_idempotent() {
    proptest!(|
        (digits in 1i64..19,
         total in 1i64..101,
         duration_s in 0.1_f64..5.0,
         delay_s in 0.0_f64..5.0)
    | {
        let input = SessionConfigInput {
            digits_per_number: digits,
            number_duration_s: duration_s,
            delay_between_numbers_s: delay_s,
            total_numbers: total,
            allow_negative_numbers: false,
        };

        let (config1, _) = normalize_session_config(input.clone());
        let (config2, _) = normalize_session_config(input);

        // Normalizing twice should give the same result
        prop_assert_eq!(config1.digits_per_number, config2.digits_per_number);
        prop_assert_eq!(config1.number_duration_ms, config2.number_duration_ms);
        prop_assert_eq!(config1.delay_between_numbers_ms, config2.delay_between_numbers_ms);
        prop_assert_eq!(config1.total_numbers, config2.total_numbers);
    });
}

#[test]
fn prop_validate_accepts_valid_configs() {
    proptest!(|
        (digits in 1u32..19,
         duration_ms in 1u64..60_001,
         delay_ms in 0u64..60_001,
         total in 1u32..10_001)
    | {
        let config = SessionConfig {
            digits_per_number: digits,
            number_duration_ms: duration_ms,
            delay_between_numbers_ms: delay_ms,
            total_numbers: total,
            allow_negative_numbers: false,
        };

        // Valid configs within bounds should always validate
        let result = validate_config(&config);
        prop_assert!(result.is_ok(), "Config should be valid: {:?}", config);
    });
}

#[test]
fn prop_effective_duration_round_1_decimal() {
    proptest!(|(duration_s in 0.1_f64..5.0)| {
        let input = SessionConfigInput {
            digits_per_number: 1,
            number_duration_s: duration_s,
            delay_between_numbers_s: 0.0,
            total_numbers: 5,
            allow_negative_numbers: false,
        };

        let (_config, effective) = normalize_session_config(input);

        // Check that effective is rounded to 1 decimal place
        let rounded = (effective.number_duration_s * 10.0).round() / 10.0;
        prop_assert_eq!(effective.number_duration_s, rounded);
    });
}

#[test]
fn prop_allow_negative_flag_preserved() {
    proptest!(|(allow_neg in any::<bool>())| {
        let input = SessionConfigInput {
            digits_per_number: 1,
            number_duration_s: 1.0,
            delay_between_numbers_s: 0.0,
            total_numbers: 5,
            allow_negative_numbers: allow_neg,
        };

        let (_config, effective) = normalize_session_config(input);

        // Flag should be preserved through normalization
        prop_assert_eq!(effective.allow_negative_numbers, allow_neg);
    });
}

#[test]
fn prop_duration_monotonic() {
    proptest!(|
        (duration1_s in 0.1_f64..5.0,
         duration2_s in 0.1_f64..5.0)
    | {
        let input1 = SessionConfigInput {
            digits_per_number: 1,
            number_duration_s: duration1_s,
            delay_between_numbers_s: 0.0,
            total_numbers: 5,
            allow_negative_numbers: false,
        };

        let input2 = SessionConfigInput {
            digits_per_number: 1,
            number_duration_s: duration2_s,
            delay_between_numbers_s: 0.0,
            total_numbers: 5,
            allow_negative_numbers: false,
        };

        let (config1, _) = normalize_session_config(input1);
        let (config2, _) = normalize_session_config(input2);

        // If duration1 < duration2, then normalized duration1 <= duration2
        // (accounting for clamping and rounding)
        if duration1_s < duration2_s {
            prop_assert!(config1.number_duration_ms <= config2.number_duration_ms);
        }
    });
}

#[test]
fn prop_nan_duration_clamps_to_min() {
    proptest!(|
        (digits in 1u32..19,
         total in 1u32..101)
    | {
        let input = SessionConfigInput {
            digits_per_number: digits as i64,
            number_duration_s: f64::NAN,
            delay_between_numbers_s: 0.0,
            total_numbers: total as i64,
            allow_negative_numbers: false,
        };

        let (config, _) = normalize_session_config(input);

        // NaN should clamp to minimum 100ms (0.1 seconds)
        prop_assert_eq!(config.number_duration_ms, 100);
    });
}

#[test]
fn prop_infinity_duration_clamps_to_max() {
    proptest!(|(_ in Just(()))| {
        let input = SessionConfigInput {
            digits_per_number: 1,
            number_duration_s: f64::INFINITY,
            delay_between_numbers_s: 0.0,
            total_numbers: 5,
            allow_negative_numbers: false,
        };

        let (config, _) = normalize_session_config(input);

        // Infinity should clamp to maximum 60_000ms
        prop_assert_eq!(config.number_duration_ms, 60_000);
    });
}
