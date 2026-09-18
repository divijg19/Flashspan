// Tests for core validation and engine modules
#[cfg(test)]
#[allow(clippy::module_inception)]
mod tests {
    use crate::core::engine::build_session_plan;
    use crate::core::types::{SessionConfig, SessionConfigInput};
    use crate::core::validate::{
        MAX_EXACT_INTEGER, max_total_for_digits, normalize_session_config, validate_config,
    };

    // ======================
    // Validation Module Tests
    // ======================

    #[test]
    fn normalize_session_config_clamps_digits_per_number() {
        // Test lower bound clamping
        let input = SessionConfigInput {
            digits_per_number: 0,
            number_duration_s: 1.0,
            total_numbers: 5,
            allow_negative_numbers: false,
        };
        let (config, _effective) = normalize_session_config(input);
        assert_eq!(
            config.digits_per_number, 1,
            "digits_per_number should clamp to 1 minimum"
        );

        // Test upper bound clamping
        let input_high = SessionConfigInput {
            digits_per_number: 100,
            number_duration_s: 1.0,
            total_numbers: 5,
            allow_negative_numbers: false,
        };
        let (config_high, _) = normalize_session_config(input_high);
        assert_eq!(
            config_high.digits_per_number, 15,
            "digits_per_number should clamp to 15 maximum"
        );
    }

    #[test]
    fn normalize_session_config_clamps_total_numbers() {
        // Test lower bound
        let input = SessionConfigInput {
            digits_per_number: 1,
            number_duration_s: 1.0,
            total_numbers: 0,
            allow_negative_numbers: false,
        };
        let (config, _) = normalize_session_config(input);
        assert_eq!(
            config.total_numbers, 1,
            "total_numbers should clamp to 1 minimum"
        );

        // Test upper bound
        let input_high = SessionConfigInput {
            digits_per_number: 1,
            number_duration_s: 1.0,
            total_numbers: 20_000,
            allow_negative_numbers: false,
        };
        let (config_high, _) = normalize_session_config(input_high);
        assert_eq!(
            config_high.total_numbers, 10_000,
            "total_numbers should clamp to 10_000 maximum"
        );
    }

    #[test]
    fn normalize_session_config_handles_duration_values() {
        // Test very small duration (should clamp to 0.1s = 100ms)
        let input = SessionConfigInput {
            digits_per_number: 1,
            number_duration_s: 0.01,
            total_numbers: 5,
            allow_negative_numbers: false,
        };
        let (config, effective) = normalize_session_config(input);
        assert!(
            config.number_duration_ms >= 1,
            "duration should clamp to minimum 1ms"
        );
        assert_eq!(
            effective.number_duration_s, 0.1,
            "effective should round to 0.1s"
        );

        // Test large duration (should clamp to 60s = 60_000ms)
        let input_large = SessionConfigInput {
            digits_per_number: 1,
            number_duration_s: 100.0,
            total_numbers: 5,
            allow_negative_numbers: false,
        };
        let (config_large, effective_large) = normalize_session_config(input_large);
        assert_eq!(
            config_large.number_duration_ms, 60_000,
            "duration should clamp to 60_000ms maximum"
        );
        assert_eq!(
            effective_large.number_duration_s, 60.0,
            "effective should be 60.0s"
        );
    }

    #[test]
    fn normalize_session_config_handles_nan_and_infinity() {
        // NaN should clamp to minimum
        let input_nan = SessionConfigInput {
            digits_per_number: 1,
            number_duration_s: f64::NAN,
            total_numbers: 5,
            allow_negative_numbers: false,
        };
        let (config_nan, _) = normalize_session_config(input_nan);
        assert_eq!(
            config_nan.number_duration_ms, 100,
            "NaN duration should clamp to minimum 100ms (0.1 seconds)"
        );

        // Positive infinity should clamp to maximum
        let input_inf = SessionConfigInput {
            digits_per_number: 1,
            number_duration_s: f64::INFINITY,
            total_numbers: 5,
            allow_negative_numbers: false,
        };
        let (config_inf, _) = normalize_session_config(input_inf);
        assert_eq!(
            config_inf.number_duration_ms, 60_000,
            "Infinity should clamp to maximum 60_000ms"
        );

        // Negative infinity should clamp to minimum
        let input_neginf = SessionConfigInput {
            digits_per_number: 1,
            number_duration_s: f64::NEG_INFINITY,
            total_numbers: 5,
            allow_negative_numbers: false,
        };
        let (config_neginf, _) = normalize_session_config(input_neginf);
        assert_eq!(
            config_neginf.number_duration_ms, 100,
            "Negative infinity should clamp to minimum 100ms (0.1 seconds)"
        );
    }

    #[test]
    fn normalize_session_config_rounds_to_1_decimal() {
        // Test rounding of effective duration
        let input = SessionConfigInput {
            digits_per_number: 1,
            number_duration_s: 1.234,
            total_numbers: 5,
            allow_negative_numbers: false,
        };
        let (_config, effective) = normalize_session_config(input);

        // 1.234s should round to 1.2s
        assert_eq!(
            effective.number_duration_s, 1.2,
            "duration should round to 1 decimal place"
        );
    }

    #[test]
    fn validate_config_rejects_zero_values() {
        let config_bad_digits = SessionConfig {
            digits_per_number: 0,
            number_duration_ms: 100,
            total_numbers: 5,
            allow_negative_numbers: false,
        };
        assert!(
            validate_config(&config_bad_digits).is_err(),
            "should reject digits_per_number = 0"
        );

        let config_bad_duration = SessionConfig {
            digits_per_number: 1,
            number_duration_ms: 0,
            total_numbers: 5,
            allow_negative_numbers: false,
        };
        assert!(
            validate_config(&config_bad_duration).is_err(),
            "should reject number_duration_ms = 0"
        );

        let config_bad_numbers = SessionConfig {
            digits_per_number: 1,
            number_duration_ms: 100,
            total_numbers: 0,
            allow_negative_numbers: false,
        };
        assert!(
            validate_config(&config_bad_numbers).is_err(),
            "should reject total_numbers = 0"
        );
    }

    #[test]
    fn validate_config_rejects_out_of_bounds_values() {
        // digits_per_number > 15
        let config_digits_over = SessionConfig {
            digits_per_number: 16,
            number_duration_ms: 100,
            total_numbers: 5,
            allow_negative_numbers: false,
        };
        assert!(
            validate_config(&config_digits_over).is_err(),
            "should reject digits_per_number > 15"
        );

        // total_numbers > 10_000
        let config_numbers_over = SessionConfig {
            digits_per_number: 1,
            number_duration_ms: 100,
            total_numbers: 10_001,
            allow_negative_numbers: false,
        };
        assert!(
            validate_config(&config_numbers_over).is_err(),
            "should reject total_numbers > 10_000"
        );

        // number_duration_ms > 60_000
        let config_duration_over = SessionConfig {
            digits_per_number: 1,
            number_duration_ms: 60_001,
            total_numbers: 5,
            allow_negative_numbers: false,
        };
        assert!(
            validate_config(&config_duration_over).is_err(),
            "should reject number_duration_ms > 60_000"
        );
    }

    #[test]
    fn validate_config_accepts_boundary_values() {
        // All minimum values (except zero)
        let config_min = SessionConfig {
            digits_per_number: 1,
            number_duration_ms: 1,
            total_numbers: 1,
            allow_negative_numbers: false,
        };
        assert!(
            validate_config(&config_min).is_ok(),
            "should accept minimum values"
        );

        // Maximum values within the exact-integer bound: 11 digits allow
        // the full 10,000-number range (worst sum still below 2^53).
        let config_max = SessionConfig {
            digits_per_number: 11,
            number_duration_ms: 60_000,
            total_numbers: 10_000,
            allow_negative_numbers: true,
        };
        assert!(
            validate_config(&config_max).is_ok(),
            "should accept maximum values"
        );

        // Widest digits at their bound: 15 digits allow at most 9 numbers.
        let config_wide = SessionConfig {
            digits_per_number: 15,
            number_duration_ms: 100,
            total_numbers: 9,
            allow_negative_numbers: false,
        };
        assert!(
            validate_config(&config_wide).is_ok(),
            "should accept 15 digits with 9 numbers"
        );

        // One past the bound must be rejected.
        let config_over = SessionConfig {
            digits_per_number: 15,
            number_duration_ms: 100,
            total_numbers: 10,
            allow_negative_numbers: false,
        };
        assert!(
            validate_config(&config_over).is_err(),
            "should reject total_numbers past the digit-width bound"
        );
    }

    #[test]
    fn normalize_allows_negative_numbers_flag() {
        let input_neg = SessionConfigInput {
            digits_per_number: 2,
            number_duration_s: 1.0,
            total_numbers: 10,
            allow_negative_numbers: true,
        };
        let (_config, effective) = normalize_session_config(input_neg);
        assert!(
            effective.allow_negative_numbers,
            "should preserve allow_negative_numbers flag"
        );

        let input_pos = SessionConfigInput {
            digits_per_number: 2,
            number_duration_s: 1.0,
            total_numbers: 10,
            allow_negative_numbers: false,
        };
        let (_config_pos, effective_pos) = normalize_session_config(input_pos);
        assert!(
            !effective_pos.allow_negative_numbers,
            "should preserve allow_negative_numbers flag"
        );
    }

    #[test]
    fn timing_budget_constants_are_100ms() {
        // The inter-number gap is no longer configurable; both executors
        // share these fixed values (see crate::core::timing).
        assert_eq!(
            crate::core::timing::INTER_NUMBER_GAP_MS,
            100,
            "inter-number gap must stay 100ms"
        );
        assert_eq!(
            crate::core::timing::PRE_FLASH_SETTLE_MS,
            100,
            "pre-flash settle must stay 100ms"
        );
    }

    #[test]
    fn digit_width_total_bound_table() {
        // Bound = min(10_000, floor((2^53 - 1) / (10^d - 1))). Only d >= 12
        // is constrained; anything below keeps the global 10,000 ceiling.
        let expected: [(u32, u32); 15] = [
            (1, 10_000),
            (2, 10_000),
            (3, 10_000),
            (4, 10_000),
            (5, 10_000),
            (6, 10_000),
            (7, 10_000),
            (8, 10_000),
            (9, 10_000),
            (10, 10_000),
            (11, 10_000),
            (12, 9007),
            (13, 900),
            (14, 90),
            (15, 9),
        ];
        for (digits, bound) in expected {
            assert_eq!(
                max_total_for_digits(digits),
                bound,
                "wrong bound for {digits} digits"
            );
        }
    }

    #[test]
    fn normalize_enforces_digit_width_total_bound() {
        // 15 digits with an excessive request clamps to 9.
        let (config, effective) = normalize_session_config(SessionConfigInput {
            digits_per_number: 15,
            number_duration_s: 0.5,
            total_numbers: 100,
            allow_negative_numbers: false,
        });
        assert_eq!(config.digits_per_number, 15);
        assert_eq!(config.total_numbers, 9);
        assert_eq!(effective.total_numbers, 9);

        // Small widths keep the requested total.
        let (config_small, _) = normalize_session_config(SessionConfigInput {
            digits_per_number: 3,
            number_duration_s: 0.5,
            total_numbers: 500,
            allow_negative_numbers: false,
        });
        assert_eq!(config_small.total_numbers, 500);
    }

    #[test]
    fn worst_case_sessions_stay_exact_at_every_width() {
        // At each width, build the maximum-bound session: must not panic,
        // every value and the sum must stay exactly representable in f64
        // (and therefore inside i64 with wide margin).
        const MAX_EXACT_I128: i128 = MAX_EXACT_INTEGER as i128;
        for digits in 1..=15u32 {
            let bound = max_total_for_digits(digits);
            let (config, _) = normalize_session_config(SessionConfigInput {
                digits_per_number: digits as i64,
                number_duration_s: 0.1,
                total_numbers: 10_000,
                allow_negative_numbers: true,
            });
            assert_eq!(config.total_numbers, bound);

            let plan = build_session_plan(1, config, config_snapshot(digits, bound), Some(99u64));
            assert_eq!(plan.numbers_generated.len(), bound as usize);
            for value in &plan.numbers_generated {
                assert!(
                    (*value as i128).abs() <= MAX_EXACT_I128,
                    "value {value} exceeds exact-integer range at width {digits}"
                );
            }
            assert!(
                (plan.expected_sum as i128).abs() <= MAX_EXACT_I128,
                "sum {} exceeds exact-integer range at width {digits}",
                plan.expected_sum
            );
            assert_eq!(
                plan.expected_sum,
                plan.numbers_generated.iter().sum::<i64>(),
                "sum mismatch at width {digits}"
            );
        }
    }

    fn config_snapshot(digits: u32, total: u32) -> crate::core::types::SessionConfigEffective {
        crate::core::types::SessionConfigEffective {
            digits_per_number: digits,
            number_duration_s: 0.1,
            total_numbers: total,
            allow_negative_numbers: true,
        }
    }
}
