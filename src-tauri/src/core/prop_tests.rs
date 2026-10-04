// Property-based tests using proptest for determinism and bounds checking
use super::engine::build_session_plan;
use super::generate::magnitude_bounds;
use super::types::SessionStep;
use super::types::{SessionConfig, SessionConfigInput, SessionPlan};
use super::validate::{
    MAX_DURATION_MS, effective_from, max_total_for_digits, normalize_session_config,
    validate_config,
};
use proptest::prelude::*;

#[test]
fn prop_normalize_digits_in_bounds() {
    proptest!(|(digits in 0i64..1000)| {
        let input = SessionConfigInput {
            digits_per_number: digits,
            number_duration_s: 1.0,
            total_numbers: 5,
            allow_negative_numbers: false,
        };
        let (config, _effective) = normalize_session_config(&input);

        // Result should always be between 1 and 15
        prop_assert!(config.digits_per_number >= 1);
        prop_assert!(config.digits_per_number <= 15);
    });
}

#[test]
fn prop_normalize_total_numbers_in_bounds() {
    proptest!(|(total in 0i64..100_000)| {
        let input = SessionConfigInput {
            digits_per_number: 1,
            number_duration_s: 1.0,
            total_numbers: total,
            allow_negative_numbers: false,
        };
        let (config, _effective) = normalize_session_config(&input);

        // Result should always be between 1 and 10_000
        prop_assert!(config.total_numbers >= 1);
        prop_assert!(config.total_numbers <= 10_000);
    });
}

#[test]
fn prop_normalize_duration_in_bounds() {
    proptest!(|(duration_s in 0.0_f64..1000.0)| {
        let input = SessionConfigInput {
            digits_per_number: 1,
            number_duration_s: duration_s,
            total_numbers: 5,
            allow_negative_numbers: false,
        };
        let (config, _effective) = normalize_session_config(&input);

        // Result should always be between 1ms and 60_000ms
        prop_assert!(config.number_duration_ms >= 1);
        prop_assert!(config.number_duration_ms <= 60_000);
    });
}

#[test]
fn prop_duration_is_grid_aligned() {
    proptest!(|(duration_s in 0.0_f64..1000.0)| {
        let input = SessionConfigInput {
            digits_per_number: 1,
            number_duration_s: duration_s,
            total_numbers: 5,
            allow_negative_numbers: false,
        };
        let (config, _effective) = normalize_session_config(&input);

        // Normalized durations snap to the 0.1s grid the UI exposes, so the
        // exposure is a whole number of 100ms steps and can always be
        // reported exactly.
        prop_assert_eq!(config.number_duration_ms % 100, 0);
        prop_assert!(config.number_duration_ms >= 100);
        prop_assert!(config.number_duration_ms <= MAX_DURATION_MS);
    });
}

// Exactness lock: the effective values compared here are produced by the same
// rounding, so identical inputs must yield bit-identical output.
#[allow(clippy::float_cmp)]
#[test]
fn prop_normalize_is_a_fixed_point() {
    proptest!(|
        (digits in 1i64..16,
         total in 1i64..101,
         duration_s in 0.1_f64..5.0)
    | {
        let input = SessionConfigInput {
            digits_per_number: digits,
            number_duration_s: duration_s,
            total_numbers: total,
            allow_negative_numbers: false,
        };

        let (config, effective) = normalize_session_config(&input);

        // Re-normalizing what was reported must be a no-op. This is exactly
        // what a second session does when the UI echoes its state back, so it
        // is also the property that the reported duration is precisely the
        // exposure: a lossy report (e.g. 110ms reported as 0.1s) would
        // re-normalize to 100ms and fail here.
        let echoed = SessionConfigInput {
            digits_per_number: i64::from(effective.digits_per_number),
            number_duration_s: effective.number_duration_s,
            total_numbers: i64::from(effective.total_numbers),
            allow_negative_numbers: effective.allow_negative_numbers,
        };
        let (config2, effective2) = normalize_session_config(&echoed);

        prop_assert_eq!(config2.digits_per_number, config.digits_per_number);
        prop_assert_eq!(config2.number_duration_ms, config.number_duration_ms);
        prop_assert_eq!(config2.total_numbers, config.total_numbers);
        prop_assert_eq!(config2.allow_negative_numbers, config.allow_negative_numbers);
        prop_assert_eq!(effective2.number_duration_s, effective.number_duration_s);
    });
}

#[test]
fn prop_validate_accepts_valid_configs() {
    proptest!(|(digits in 1u32..16, duration_ms in 1u64..60_001, total_seed in any::<u32>())| {
        // Total must respect the digit-width exact-integer bound; map an
        // arbitrary seed into range (bound is always >= 1).
        let total = total_seed % max_total_for_digits(digits) + 1;
        let config = SessionConfig {
            digits_per_number: digits,
            number_duration_ms: duration_ms,
            total_numbers: total,
            allow_negative_numbers: false,
        };

        // Valid configs within bounds should always validate
        let result = validate_config(&config);
        prop_assert!(result.is_ok(), "Config should be valid: {:?}", config);
    });
}

// Exactness lock: rounding behavior must be bit-exact, so no epsilon.
#[allow(clippy::float_cmp)]
#[test]
fn prop_effective_duration_round_1_decimal() {
    proptest!(|(duration_s in 0.1_f64..5.0)| {
        let input = SessionConfigInput {
            digits_per_number: 1,
            number_duration_s: duration_s,
            total_numbers: 5,
            allow_negative_numbers: false,
        };

        let (_config, effective) = normalize_session_config(&input);

        // Check that effective is rounded to 1 decimal place
        // (exactness lock: no epsilon).
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
            total_numbers: 5,
            allow_negative_numbers: allow_neg,
        };

        let (_config, effective) = normalize_session_config(&input);

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
            total_numbers: 5,
            allow_negative_numbers: false,
        };

        let input2 = SessionConfigInput {
            digits_per_number: 1,
            number_duration_s: duration2_s,
            total_numbers: 5,
            allow_negative_numbers: false,
        };

        let (config1, _) = normalize_session_config(&input1);
        let (config2, _) = normalize_session_config(&input2);

        // If duration1 < duration2, then normalized duration1 <= duration2
        // (accounting for clamping and rounding)
        if duration1_s < duration2_s {
            prop_assert!(config1.number_duration_ms <= config2.number_duration_ms);
        }
    });
}

#[test]
fn nan_duration_clamps_to_min() {
    for digits in 1u32..19 {
        for total in 1u32..101 {
            let input = SessionConfigInput {
                digits_per_number: i64::from(digits),
                number_duration_s: f64::NAN,
                total_numbers: i64::from(total),
                allow_negative_numbers: false,
            };

            let (config, _) = normalize_session_config(&input);

            // NaN should clamp to minimum 100ms (0.1 seconds)
            assert_eq!(
                config.number_duration_ms, 100,
                "digits {digits}, total {total}"
            );
        }
    }
}

#[test]
fn infinity_duration_clamps_to_max() {
    {
        let input = SessionConfigInput {
            digits_per_number: 1,
            number_duration_s: f64::INFINITY,
            total_numbers: 5,
            allow_negative_numbers: false,
        };

        let (config, _) = normalize_session_config(&input);

        // Infinity should clamp to maximum 60_000ms
        assert_eq!(config.number_duration_ms, MAX_DURATION_MS);
    }
}

#[test]
fn prop_plan_sum_never_negative() {
    proptest!(|(digits in 1u32..16, seed in any::<u64>())| {
        // Total stays well inside the digit-width bound for speed; the
        // non-negativity proof holds for every total.
        let input = SessionConfigInput {
            digits_per_number: i64::from(digits),
            number_duration_s: 0.5,
            total_numbers: 20,
            allow_negative_numbers: true,
        };
        let (config, effective) = normalize_session_config(&input);
        let plan = build_session_plan(1, &config, effective, Some(seed));

        prop_assert!(
            plan.expected_sum >= 0,
            "true sum went negative at width {digits}"
        );
        let recomputed: i64 = plan.numbers_generated.iter().sum();
        prop_assert_eq!(plan.expected_sum, recomputed);
    });
}

/// A normalized config paired with an arbitrary seed: the shape most
/// properties below need.
fn plan_for(digits: u32, total_numbers: i64, seed: u64) -> (SessionConfig, SessionPlan) {
    let input = SessionConfigInput {
        digits_per_number: i64::from(digits),
        number_duration_s: 0.5,
        total_numbers,
        allow_negative_numbers: true,
    };
    let (config, effective) = normalize_session_config(&input);
    let plan = build_session_plan(1, &config, effective, Some(seed));
    (config, plan)
}

#[test]
fn prop_plan_total_duration_matches_its_steps() {
    proptest!(|(digits in 1u32..16, total in 1i64..40, seed in any::<u64>())| {
        let (_config, plan) = plan_for(digits, total, seed);

        // The advertised total is derived from the steps, so the schedule the
        // executor runs and the duration reported to callers cannot disagree.
        let summed: u64 = plan.steps.iter().map(SessionStep::delay_ms).sum();
        prop_assert_eq!(plan.total_duration_ms, summed);
    });
}

// Exactness lock: the snapshot is compared against the value the same
// rounding produced, so identical inputs must be bit-identical.
#[allow(clippy::float_cmp)]
#[test]
fn prop_plan_snapshot_is_the_reported_config() {
    proptest!(|(digits in 1u32..16, total in 1i64..40, seed in any::<u64>())| {
        let (config, plan) = plan_for(digits, total, seed);

        // The plan's snapshot must be the same effective config the caller was
        // told about, for every field.
        let effective = effective_from(&config);
        prop_assert_eq!(
            plan.config_snapshot.number_duration_s,
            effective.number_duration_s
        );
        prop_assert_eq!(
            plan.config_snapshot.digits_per_number,
            effective.digits_per_number
        );
        prop_assert_eq!(plan.config_snapshot.total_numbers, effective.total_numbers);
        prop_assert_eq!(
            plan.config_snapshot.allow_negative_numbers,
            effective.allow_negative_numbers
        );
    });
}

#[test]
fn prop_plan_complete_step_agrees_with_the_plan() {
    proptest!(|(digits in 1u32..16, total in 1i64..40, seed in any::<u64>())| {
        let (_config, plan) = plan_for(digits, total, seed);

        let complete = plan
            .steps
            .last()
            .expect("every plan ends with a Complete step");
        let SessionStep::Complete { numbers, sum, .. } = complete else {
            prop_assert!(false, "last step must be Complete");
            return Ok(());
        };

        prop_assert_eq!(numbers, &plan.numbers_generated);
        prop_assert_eq!(*sum, plan.expected_sum);
        prop_assert_eq!(
            *sum,
            plan.numbers_generated.iter().sum::<i64>(),
            "expected_sum must be the sum of the numbers"
        );
    });
}

#[test]
fn prop_plan_numbers_respect_the_digit_width() {
    proptest!(|(digits in 1u32..16, total in 1i64..40, seed in any::<u64>())| {
        let (_config, plan) = plan_for(digits, total, seed);
        let (min, max) = magnitude_bounds(digits);

        for (index, number) in plan.numbers_generated.iter().enumerate() {
            let magnitude = number.unsigned_abs();
            prop_assert!(
                magnitude >= min && magnitude <= max,
                "number {number} at {index} is outside the {digits}-digit domain [{min}, {max}]"
            );
        }
    });
}

#[test]
fn prop_plan_never_repeats_a_number_consecutively() {
    proptest!(|(digits in 1u32..16, total in 2i64..60, seed in any::<u64>())| {
        let (_config, plan) = plan_for(digits, total, seed);

        // True for every seed: duplicates are rejected by retry, and the
        // deterministic fallback rotates inside the same domain.
        for pair in plan.numbers_generated.windows(2) {
            prop_assert_ne!(pair[0], pair[1], "consecutive duplicate {}", pair[0]);
        }
    });
}

#[test]
fn prop_plan_running_sums_are_consistent() {
    proptest!(|(digits in 1u32..16, total in 1i64..40, seed in any::<u64>())| {
        let (_config, plan) = plan_for(digits, total, seed);

        let mut expected: i64 = 0;
        for step in &plan.steps {
            if let SessionStep::ShowNumber {
                value,
                running_sum,
                ..
            } = step
            {
                expected += value;
                prop_assert_eq!(
                    *running_sum,
                    expected,
                    "running_sum must be the running total of the numbers"
                );
            }
        }
    });
}
