// Tests for core validation and engine modules
#[cfg(test)]
#[allow(clippy::module_inception)]
mod tests {
    use crate::core::types::{SessionConfig, SessionConfigInput};
    use crate::core::validate::{normalize_session_config, validate_config};

    // ======================
    // Validation Module Tests
    // ======================

    #[test]
    fn normalize_session_config_clamps_digits_per_number() {
        // Test lower bound clamping
        let input = SessionConfigInput {
            digits_per_number: 0,
            number_duration_s: 1.0,
            delay_between_numbers_s: 0.0,
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
            delay_between_numbers_s: 0.0,
            total_numbers: 5,
            allow_negative_numbers: false,
        };
        let (config_high, _) = normalize_session_config(input_high);
        assert_eq!(
            config_high.digits_per_number, 18,
            "digits_per_number should clamp to 18 maximum"
        );
    }

    #[test]
    fn normalize_session_config_clamps_total_numbers() {
        // Test lower bound
        let input = SessionConfigInput {
            digits_per_number: 1,
            number_duration_s: 1.0,
            delay_between_numbers_s: 0.0,
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
            delay_between_numbers_s: 0.0,
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
            delay_between_numbers_s: 0.0,
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
            delay_between_numbers_s: 0.0,
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
            delay_between_numbers_s: 0.0,
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
            delay_between_numbers_s: 0.0,
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
            delay_between_numbers_s: 0.0,
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
            delay_between_numbers_s: 2.567,
            total_numbers: 5,
            allow_negative_numbers: false,
        };
        let (_config, effective) = normalize_session_config(input);

        // 1.234s should round to 1.2s
        assert_eq!(
            effective.number_duration_s, 1.2,
            "duration should round to 1 decimal place"
        );
        // 2.567s should round to 2.6s
        assert_eq!(
            effective.delay_between_numbers_s, 2.6,
            "delay should round to 1 decimal place"
        );
    }

    #[test]
    fn validate_config_rejects_zero_values() {
        let config_bad_digits = SessionConfig {
            digits_per_number: 0,
            number_duration_ms: 100,
            delay_between_numbers_ms: 0,
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
            delay_between_numbers_ms: 0,
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
            delay_between_numbers_ms: 0,
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
        // digits_per_number > 18
        let config_digits_over = SessionConfig {
            digits_per_number: 19,
            number_duration_ms: 100,
            delay_between_numbers_ms: 0,
            total_numbers: 5,
            allow_negative_numbers: false,
        };
        assert!(
            validate_config(&config_digits_over).is_err(),
            "should reject digits_per_number > 18"
        );

        // total_numbers > 10_000
        let config_numbers_over = SessionConfig {
            digits_per_number: 1,
            number_duration_ms: 100,
            delay_between_numbers_ms: 0,
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
            delay_between_numbers_ms: 0,
            total_numbers: 5,
            allow_negative_numbers: false,
        };
        assert!(
            validate_config(&config_duration_over).is_err(),
            "should reject number_duration_ms > 60_000"
        );

        // delay_between_numbers_ms > 60_000
        let config_delay_over = SessionConfig {
            digits_per_number: 1,
            number_duration_ms: 100,
            delay_between_numbers_ms: 60_001,
            total_numbers: 5,
            allow_negative_numbers: false,
        };
        assert!(
            validate_config(&config_delay_over).is_err(),
            "should reject delay_between_numbers_ms > 60_000"
        );
    }

    #[test]
    fn validate_config_accepts_boundary_values() {
        // All minimum values (except zero)
        let config_min = SessionConfig {
            digits_per_number: 1,
            number_duration_ms: 1,
            delay_between_numbers_ms: 0,
            total_numbers: 1,
            allow_negative_numbers: false,
        };
        assert!(
            validate_config(&config_min).is_ok(),
            "should accept minimum values"
        );

        // All maximum values
        let config_max = SessionConfig {
            digits_per_number: 18,
            number_duration_ms: 60_000,
            delay_between_numbers_ms: 60_000,
            total_numbers: 10_000,
            allow_negative_numbers: true,
        };
        assert!(
            validate_config(&config_max).is_ok(),
            "should accept maximum values"
        );
    }

    #[test]
    fn normalize_allows_negative_numbers_flag() {
        let input_neg = SessionConfigInput {
            digits_per_number: 2,
            number_duration_s: 1.0,
            delay_between_numbers_s: 0.0,
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
            delay_between_numbers_s: 0.0,
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
    fn normalize_session_config_delay_clamping() {
        // Test delay below minimum (should be 0)
        let input = SessionConfigInput {
            digits_per_number: 1,
            number_duration_s: 1.0,
            delay_between_numbers_s: -5.0,
            total_numbers: 5,
            allow_negative_numbers: false,
        };
        let (config, effective) = normalize_session_config(input);
        assert_eq!(
            config.delay_between_numbers_ms, 0,
            "negative delay should clamp to 0"
        );
        assert_eq!(
            effective.delay_between_numbers_s, 0.0,
            "effective delay should be 0.0"
        );

        // Test delay above maximum (should clamp to 60s)
        let input_high = SessionConfigInput {
            digits_per_number: 1,
            number_duration_s: 1.0,
            delay_between_numbers_s: 100.0,
            total_numbers: 5,
            allow_negative_numbers: false,
        };
        let (config_high, effective_high) = normalize_session_config(input_high);
        assert_eq!(
            config_high.delay_between_numbers_ms, 60_000,
            "large delay should clamp to 60_000ms"
        );
        assert_eq!(
            effective_high.delay_between_numbers_s, 60.0,
            "effective delay should be 60.0s"
        );
    }
}
