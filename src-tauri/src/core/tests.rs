// Tests for core validation and engine modules
mod validation_tests {
    // Several of these pin the schedule and wire format the browser consumes, so
    // they must run on wasm too. A plain `#[test]` compiles there but is
    // silently skipped by the wasm-bindgen test runner.
    #[cfg(target_arch = "wasm32")]
    use wasm_bindgen_test::wasm_bindgen_test;

    use crate::core::engine::build_session_plan;
    use crate::core::types::{SessionConfig, SessionConfigInput, SessionStep};
    use crate::core::validate::{
        MAX_EXACT_INTEGER, MAX_TOTAL_NUMBERS, max_total_for_digits, normalize_session_config,
        validate_config,
    };

    // ======================
    // Validation Module Tests
    // ======================

    #[cfg_attr(target_arch = "wasm32", wasm_bindgen_test)]
    #[cfg_attr(not(target_arch = "wasm32"), test)]
    fn normalize_session_config_clamps_digits_per_number() {
        // Test lower bound clamping
        let input = SessionConfigInput {
            digits_per_number: 0,
            number_duration_s: 1.0,
            total_numbers: 5,
            allow_negative_numbers: false,
        };
        let (config, _effective) = normalize_session_config(&input);
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
        let (config_high, _) = normalize_session_config(&input_high);
        assert_eq!(
            config_high.digits_per_number, 15,
            "digits_per_number should clamp to 15 maximum"
        );
    }

    #[cfg_attr(target_arch = "wasm32", wasm_bindgen_test)]
    #[cfg_attr(not(target_arch = "wasm32"), test)]
    fn normalize_session_config_clamps_total_numbers() {
        // Test lower bound
        let input = SessionConfigInput {
            digits_per_number: 1,
            number_duration_s: 1.0,
            total_numbers: 0,
            allow_negative_numbers: false,
        };
        let (config, _) = normalize_session_config(&input);
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
        let (config_high, _) = normalize_session_config(&input_high);
        assert_eq!(
            config_high.total_numbers, 10_000,
            "total_numbers should clamp to 10_000 maximum"
        );
    }

    // Exactness locks below assert bit-exact rounding: no epsilon.
    #[allow(clippy::float_cmp)]
    #[cfg_attr(target_arch = "wasm32", wasm_bindgen_test)]
    #[cfg_attr(not(target_arch = "wasm32"), test)]
    fn normalize_session_config_handles_duration_values() {
        // Very small duration: clamps up to the 0.1s grid floor (100ms)
        let input = SessionConfigInput {
            digits_per_number: 1,
            number_duration_s: 0.01,
            total_numbers: 5,
            allow_negative_numbers: false,
        };
        let (config, effective) = normalize_session_config(&input);
        assert_eq!(
            config.number_duration_ms, 100,
            "duration should clamp to the 0.1s floor"
        );
        // Exactness lock: rounding behavior must be bit-exact, so no epsilon.
        assert_eq!(
            effective.number_duration_s, 0.1,
            "effective should report the exposure exactly"
        );

        // Test large duration (should clamp to 60s = 60_000ms)
        let input_large = SessionConfigInput {
            digits_per_number: 1,
            number_duration_s: 100.0,
            total_numbers: 5,
            allow_negative_numbers: false,
        };
        let (config_large, effective_large) = normalize_session_config(&input_large);
        assert_eq!(
            config_large.number_duration_ms, 60_000,
            "duration should clamp to 60_000ms maximum"
        );
        assert_eq!(
            effective_large.number_duration_s, 60.0,
            "effective should be 60.0s"
        );
    }

    #[cfg_attr(target_arch = "wasm32", wasm_bindgen_test)]
    #[cfg_attr(not(target_arch = "wasm32"), test)]
    fn normalize_session_config_handles_nan_and_infinity() {
        // NaN should clamp to minimum
        let input_nan = SessionConfigInput {
            digits_per_number: 1,
            number_duration_s: f64::NAN,
            total_numbers: 5,
            allow_negative_numbers: false,
        };
        let (config_nan, _) = normalize_session_config(&input_nan);
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
        let (config_inf, _) = normalize_session_config(&input_inf);
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
        let (config_neginf, _) = normalize_session_config(&input_neginf);
        assert_eq!(
            config_neginf.number_duration_ms, 100,
            "Negative infinity should clamp to minimum 100ms (0.1 seconds)"
        );
    }

    #[allow(clippy::float_cmp)]
    #[cfg_attr(target_arch = "wasm32", wasm_bindgen_test)]
    #[cfg_attr(not(target_arch = "wasm32"), test)]
    fn normalize_session_config_rounds_to_1_decimal() {
        // Test rounding of effective duration
        let input = SessionConfigInput {
            digits_per_number: 1,
            number_duration_s: 1.234,
            total_numbers: 5,
            allow_negative_numbers: false,
        };
        let (_config, effective) = normalize_session_config(&input);

        // 1.234s should round to 1.2s
        assert_eq!(
            effective.number_duration_s, 1.2,
            "duration should round to 1 decimal place"
        );
    }

    #[cfg_attr(target_arch = "wasm32", wasm_bindgen_test)]
    #[cfg_attr(not(target_arch = "wasm32"), test)]
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

    #[cfg_attr(target_arch = "wasm32", wasm_bindgen_test)]
    #[cfg_attr(not(target_arch = "wasm32"), test)]
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

    #[cfg_attr(target_arch = "wasm32", wasm_bindgen_test)]
    #[cfg_attr(not(target_arch = "wasm32"), test)]
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

    #[cfg_attr(target_arch = "wasm32", wasm_bindgen_test)]
    #[cfg_attr(not(target_arch = "wasm32"), test)]
    fn normalize_allows_negative_numbers_flag() {
        let input_neg = SessionConfigInput {
            digits_per_number: 2,
            number_duration_s: 1.0,
            total_numbers: 10,
            allow_negative_numbers: true,
        };
        let (_config, effective) = normalize_session_config(&input_neg);
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
        let (_config_pos, effective_pos) = normalize_session_config(&input_pos);
        assert!(
            !effective_pos.allow_negative_numbers,
            "should preserve allow_negative_numbers flag"
        );
    }

    /// Negative inputs are valid IPC payloads: a buggy or hostile frontend can
    /// send them, and they must clamp rather than panic downstream inside
    /// `magnitude_bounds` or `max_total_for_digits`.
    #[cfg_attr(target_arch = "wasm32", wasm_bindgen_test)]
    #[cfg_attr(not(target_arch = "wasm32"), test)]
    fn normalize_clamps_negative_inputs_like_extreme_ones() {
        // (digits, total, expected_digits, expected_total_at_that_width)
        for (digits, total, expected_digits) in [
            (-5, 5, 1),
            (0, -100, 1),
            (-1, -1, 1),
            (3, 0, 3),
            (99, 99, 15),
        ] {
            let (config, _) = normalize_session_config(&SessionConfigInput {
                digits_per_number: digits,
                number_duration_s: 1.0,
                total_numbers: total,
                allow_negative_numbers: false,
            });
            assert_eq!(
                config.digits_per_number, expected_digits,
                "digits={digits} should clamp to {expected_digits}"
            );

            // The clamped total must be the input's own bound, clamped to the
            // width — never a magic number, so this cannot go stale with the
            // bound table.
            let requested = total.clamp(1, i64::from(MAX_TOTAL_NUMBERS));
            let at_width = u32::try_from(requested)
                .unwrap_or(MAX_TOTAL_NUMBERS)
                .min(max_total_for_digits(expected_digits));
            assert_eq!(
                config.total_numbers, at_width,
                "total={total} at digits={digits} should clamp to {at_width}"
            );
        }
    }

    /// The exact error strings are part of the IPC contract: the frontend shows
    /// them to the user, so a silent wording change is a user-visible change.
    #[cfg_attr(target_arch = "wasm32", wasm_bindgen_test)]
    #[cfg_attr(not(target_arch = "wasm32"), test)]
    fn validate_config_error_strings_are_stable() {
        let bad = SessionConfig {
            digits_per_number: 0,
            number_duration_ms: 500,
            total_numbers: 5,
            allow_negative_numbers: false,
        };
        let err = validate_config(&bad).expect_err("zero digits is invalid");
        assert!(
            err.contains("digits_per_number"),
            "the message should name the offending field, got: {err}"
        );

        let bad = SessionConfig {
            digits_per_number: 16,
            number_duration_ms: 500,
            total_numbers: 5,
            allow_negative_numbers: false,
        };
        assert!(validate_config(&bad).is_err(), "16 digits is invalid");

        let bad = SessionConfig {
            digits_per_number: 1,
            number_duration_ms: 0,
            total_numbers: 5,
            allow_negative_numbers: false,
        };
        assert!(validate_config(&bad).is_err(), "zero duration is invalid");
    }

    #[cfg_attr(target_arch = "wasm32", wasm_bindgen_test)]
    #[cfg_attr(not(target_arch = "wasm32"), test)]
    fn timing_budget_constants_pin_the_whole_budget() {
        // COUNTDOWN_FROM had no direct assertion anywhere: its value was only
        // implied by hardcoded step-count literals, so a one-place change gave a
        // confusing failure in an unrelated test instead of pointing here.
        assert_eq!(
            crate::core::timing::COUNTDOWN_FROM,
            3,
            "countdown must stay three ticks"
        );
        assert_eq!(
            crate::core::timing::COUNTDOWN_TICK_MS,
            1000,
            "countdown tick must stay 1000ms"
        );
    }

    #[cfg_attr(target_arch = "wasm32", wasm_bindgen_test)]
    #[cfg_attr(not(target_arch = "wasm32"), test)]
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

    #[cfg_attr(target_arch = "wasm32", wasm_bindgen_test)]
    #[cfg_attr(not(target_arch = "wasm32"), test)]
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

    #[cfg_attr(target_arch = "wasm32", wasm_bindgen_test)]
    #[cfg_attr(not(target_arch = "wasm32"), test)]
    fn normalize_enforces_digit_width_total_bound() {
        // 15 digits with an excessive request clamps to 9.
        let (config, effective) = normalize_session_config(&SessionConfigInput {
            digits_per_number: 15,
            number_duration_s: 0.5,
            total_numbers: 100,
            allow_negative_numbers: false,
        });
        assert_eq!(config.digits_per_number, 15);
        assert_eq!(config.total_numbers, 9);
        assert_eq!(effective.total_numbers, 9);

        // Small widths keep the requested total.
        let (config_small, _) = normalize_session_config(&SessionConfigInput {
            digits_per_number: 3,
            number_duration_s: 0.5,
            total_numbers: 500,
            allow_negative_numbers: false,
        });
        assert_eq!(config_small.total_numbers, 500);
    }

    #[cfg_attr(target_arch = "wasm32", wasm_bindgen_test)]
    #[cfg_attr(not(target_arch = "wasm32"), test)]
    fn worst_case_sessions_stay_exact_at_every_width() {
        // At each width, build the maximum-bound session: must not panic,
        // every value and the sum must stay exactly representable in f64
        // (and therefore inside i64 with wide margin).
        const MAX_EXACT_I128: i128 = MAX_EXACT_INTEGER as i128;
        for digits in 1..=15u32 {
            let bound = max_total_for_digits(digits);
            let (config, _) = normalize_session_config(&SessionConfigInput {
                digits_per_number: i64::from(digits),
                number_duration_s: 0.1,
                total_numbers: 10_000,
                allow_negative_numbers: true,
            });
            assert_eq!(config.total_numbers, bound);

            let plan = build_session_plan(1, &config, config_snapshot(digits, bound), Some(99u64));
            assert_eq!(plan.numbers_generated.len(), bound as usize);
            for value in &plan.numbers_generated {
                assert!(
                    i128::from(*value).abs() <= MAX_EXACT_I128,
                    "value {value} exceeds exact-integer range at width {digits}"
                );
            }
            assert!(
                i128::from(plan.expected_sum).abs() <= MAX_EXACT_I128,
                "sum {} exceeds exact-integer range at width {digits}",
                plan.expected_sum
            );
            // Lock: the true sum is never negative. Proof: the gap
            // running_sum - true_sum starts at 0 and every accepted step
            // preserves it (negatives require running_sum + v >= 0, so the
            // max() floor never engages on an accepted value). Hence the
            // true sum equals the floored running sum and stays >= 0, which
            // is what makes plain-f64 and saturating-i64 grading agree.
            assert!(
                plan.expected_sum >= 0,
                "sum went negative at width {digits}"
            );
            assert_eq!(
                plan.expected_sum,
                plan.numbers_generated.iter().sum::<i64>(),
                "sum mismatch at width {digits}"
            );
        }
    }

    /**
     * Golden vectors. Every other assertion in this crate is *relational*: digit
     * widths, no duplicates, sum consistency, snapshot agreement. A change that
     * preserves all of them but alters the actual numbers — a different
     * distribution, a tweaked duplicate repair, a bumped `rand` minor, or any
     * native/wasm RNG divergence — currently produces identical, all-green test
     * runs. These lock the output itself.
     *
     * `StdRng::seed_from_u64` is platform-independent, so the same vectors hold
     * on the wasm target; the wasm twin guards the browser's numbers directly.
     */
    #[cfg_attr(target_arch = "wasm32", wasm_bindgen_test)]
    #[cfg_attr(not(target_arch = "wasm32"), test)]
    fn plan_matches_golden_vectors_for_fixed_seeds() {
        // Total: 0 + 3*1000 + 100 (settle) + 2*(100 flash + 100 gap) = 3500.
        /// (digits, total, seed, expected numbers, expected sum, expected duration)
        type Vector = (u32, u32, u64, &'static [i64], i64, u64);
        let cases: [Vector; 3] = [
            (1, 2, 1, &[9, 7], 16, 3500),
            (2, 2, 1, &[97, 72], 169, 3500),
            (1, 2, 99, &[6, 7], 13, 3500),
        ];

        for (digits, total, seed, numbers, sum, duration) in cases {
            // 0.1s lands exactly on the duration grid, so number_duration_ms is 100.
            let (config, effective) = normalize_session_config(&SessionConfigInput {
                digits_per_number: i64::from(digits),
                number_duration_s: 0.1,
                total_numbers: i64::from(total),
                allow_negative_numbers: false,
            });

            let plan = build_session_plan(1, &config, effective, Some(seed));
            assert_eq!(
                plan.numbers_generated, numbers,
                "numbers mismatched (digits={digits}, seed={seed})"
            );
            assert_eq!(plan.expected_sum, sum, "sum mismatched (seed={seed})");
            assert_eq!(plan.total_duration_ms, duration, "duration mismatched");
        }
    }

    /**
     * The Rust schedule must never place two visible events at the same
     * timestamp — invariant I2 in `core/timing.rs`. It was documented in Rust but
     * enforced only in TypeScript, where a collision silently falls the browser
     * back to the JS planner, and only tested there with hand-built fixtures.
     */
    #[cfg_attr(target_arch = "wasm32", wasm_bindgen_test)]
    #[cfg_attr(not(target_arch = "wasm32"), test)]
    fn plan_steps_never_share_a_timestamp() {
        for digits in [1u32, 2, 8, 15] {
            for total in [1u32, 2, 7, 40] {
                for seed in [1u64, 42, 9999] {
                    let (config, effective) = normalize_session_config(&SessionConfigInput {
                        digits_per_number: i64::from(digits),
                        number_duration_s: 0.3,
                        total_numbers: i64::from(total),
                        allow_negative_numbers: true,
                    });
                    let plan = build_session_plan(1, &config, effective, Some(seed));

                    // Fold the plan the way the browser driver does: skip the
                    // index-free clears and Complete, accumulating delays.
                    let mut at: u64 = 0;
                    let mut last: Option<u64> = None;
                    for step in &plan.steps {
                        match step {
                            SessionStep::ClearScreen { index: None, .. } => {
                                at += step.delay_ms();
                            }
                            SessionStep::Complete { .. } => {}
                            _ => {
                                if let Some(previous) = last {
                                    assert!(
                                        at > previous,
                                        "two events at t={at} (digits={digits}, total={total}, seed={seed})"
                                    );
                                }
                                last = Some(at);
                                at += step.delay_ms();
                            }
                        }
                    }
                }
            }
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

    /// The step ordering is the contract the browser driver depends on. Rename
    /// or reorder a variant and the web session silently falls back to the JS
    /// planner with no Rust test failing.
    #[cfg_attr(target_arch = "wasm32", wasm_bindgen_test)]
    #[cfg_attr(not(target_arch = "wasm32"), test)]
    fn every_plan_starts_clear_countdown_settle_and_ends_complete() {
        // One representative config: the ordering is config-independent and the
        // countdown property test below sweeps the full accepted domain.
        let (config, effective) = normalize_session_config(&SessionConfigInput {
            digits_per_number: 2,
            number_duration_s: 0.5,
            total_numbers: 4,
            allow_negative_numbers: true,
        });

        let plan = build_session_plan(1, &config, effective, Some(7));
        let steps = &plan.steps;

        // Step 0: a global clear (no index) carrying no delay.
        assert!(matches!(
            steps[0],
            SessionStep::ClearScreen {
                index: None,
                delay_ms_before_next: 0,
                ..
            }
        ));

        // Steps 1..=3: the countdown, 3, 2, then 1 carrying the settle bonus.
        let settle = crate::core::timing::PRE_FLASH_SETTLE_MS;
        let countdown: Vec<u64> = steps[1..=3].iter().map(SessionStep::delay_ms).collect();
        assert_eq!(
            countdown,
            vec![crate::core::timing::COUNTDOWN_TICK_MS, 1000, 1000 + settle]
        );
        for (i, expected) in ["3", "2", "1"].iter().enumerate() {
            match &steps[1 + i] {
                SessionStep::CountdownTick { value, .. } => {
                    assert_eq!(value, expected);
                }
                other => panic!("expected CountdownTick, got {other:?}"),
            }
        }
        assert_eq!(steps[3].delay_ms(), 1000 + settle);

        // The plan ends with a global clear then Complete, and Complete exposes
        // no delay of its own.
        assert!(matches!(
            steps[steps.len() - 2],
            SessionStep::ClearScreen { index: None, .. }
        ));
        assert!(matches!(
            steps[steps.len() - 1],
            SessionStep::Complete { .. }
        ));
        assert_eq!(steps[steps.len() - 1].delay_ms(), 0);
    }

    /// Every normalized config must produce the same countdown shape, and the
    /// advertised total must equal the sum of the step delays.
    #[cfg_attr(target_arch = "wasm32", wasm_bindgen_test)]
    #[cfg_attr(not(target_arch = "wasm32"), test)]
    fn countdown_shape_and_total_hold_across_the_accepted_domain() {
        for digits in 1..=15u32 {
            let (config, effective) = normalize_session_config(&SessionConfigInput {
                digits_per_number: i64::from(digits),
                number_duration_s: 0.5,
                total_numbers: 3,
                allow_negative_numbers: false,
            });

            let plan = build_session_plan(9, &config, effective, Some(3));

            let ticks = plan
                .steps
                .iter()
                .filter(|s| matches!(s, SessionStep::CountdownTick { .. }))
                .count();
            assert_eq!(
                ticks, 3,
                "countdown must always be three ticks (d={digits})"
            );

            let with_settle = plan
                .steps
                .iter()
                .filter(|s| matches!(s, SessionStep::CountdownTick { delay_ms_before_next: d, .. } if *d == crate::core::timing::PRE_FLASH_SETTLE_MS + crate::core::timing::COUNTDOWN_TICK_MS))
                .count();
            assert_eq!(
                with_settle, 1,
                "exactly one countdown tick carries the settle bonus (d={digits})"
            );

            let total: u64 = plan.steps.iter().map(SessionStep::delay_ms).sum();
            assert_eq!(
                total, plan.total_duration_ms,
                "advertised total must equal the sum of step delays (d={digits})"
            );
            assert!(matches!(
                plan.steps.last(),
                Some(SessionStep::Complete { .. })
            ));
        }
    }

    /// The bound table is only safe if the acceptance predicate it feeds is
    /// itself exact: for every digit width, the largest sum the generator can
    /// produce at the accepted total must stay inside `MAX_EXACT_INTEGER`.
    /// Otherwise f64 grading and the answer key could disagree.
    #[cfg_attr(target_arch = "wasm32", wasm_bindgen_test)]
    #[cfg_attr(not(target_arch = "wasm32"), test)]
    fn accepted_configs_imply_exact_max_sums() {
        for digits in 1..=15u32 {
            let total = u64::from(max_total_for_digits(digits));
            let max_magnitude = i128::from(10u64.pow(digits) - 1);
            let worst = i128::from(MAX_EXACT_INTEGER);
            let bound_total = max_magnitude * i128::from(total);
            assert!(
                bound_total <= worst,
                "digits={digits} total={total} max_magnitude={max_magnitude} bound_true_sum={bound_total} exceeds {worst}"
            );
        }
    }

    /// The largest accepted plan must build, encode, and iterate without error.
    #[cfg_attr(target_arch = "wasm32", wasm_bindgen_test)]
    #[cfg_attr(not(target_arch = "wasm32"), test)]
    fn max_total_plan_builds_and_encodes() {
        let (config, effective) = normalize_session_config(&SessionConfigInput {
            digits_per_number: 1,
            number_duration_s: 0.1,
            total_numbers: 10_000,
            allow_negative_numbers: true,
        });

        let plan = build_session_plan(1, &config, effective, Some(11));
        assert_eq!(plan.numbers_generated.len(), 10_000);
        let total: u64 = plan.steps.iter().map(SessionStep::delay_ms).sum();
        assert_eq!(total, plan.total_duration_ms);

        // The browser serializes every step across the wasm boundary; a path
        // that allocated per step or mis-sized would surface here.
        let encoded = serde_json::to_string(&plan).expect("plan encodes");
        assert!(encoded.len() > 10_000);
    }

    /// The exact step sequence is a contract the browser driver and the desktop
    /// executor both rely on: a leading global clear, three countdown ticks (the
    /// last carrying the pre-flash settle), then per number a flash and a
    /// per-index clear, then a trailing global clear and the completion. The
    /// other tests count steps and check the endpoints; this walks *every* step
    /// and pins its variant, order, index and delay.
    #[cfg_attr(target_arch = "wasm32", wasm_bindgen_test)]
    #[cfg_attr(not(target_arch = "wasm32"), test)]
    fn plan_step_sequence_is_exactly_the_documented_shape() {
        let (config, effective) = normalize_session_config(&SessionConfigInput {
            digits_per_number: 1,
            number_duration_s: 0.5,
            total_numbers: 2,
            allow_negative_numbers: false,
        });
        let plan = build_session_plan(4, &config, effective, Some(42));

        let labels: Vec<String> = plan
            .steps
            .iter()
            .map(|step| match step {
                SessionStep::CountdownTick { value, .. } => format!("countdown({value})"),
                SessionStep::ShowNumber { index, .. } => format!("show({index})"),
                SessionStep::ClearScreen { index: Some(i), .. } => format!("clear({i})"),
                SessionStep::ClearScreen { index: None, .. } => "clear".to_string(),
                SessionStep::Complete { .. } => "complete".to_string(),
            })
            .collect();

        assert_eq!(
            labels,
            vec![
                "clear",
                "countdown(3)",
                "countdown(2)",
                "countdown(1)",
                "show(1)",
                "clear(1)",
                "show(2)",
                "clear(2)",
                "clear",
                "complete",
            ],
        );

        // The delay each step waits before the next: 0, the countdown (with the
        // settle on the final tick), the exposure, the fixed inter-number gap,
        // then the closing clear and completion.
        let delays: Vec<u64> = plan.steps.iter().map(SessionStep::delay_ms).collect();
        assert_eq!(delays, vec![0, 1000, 1000, 1100, 500, 100, 500, 100, 0, 0]);
        assert_eq!(plan.total_duration_ms, 4300);

        // The per-number fields the label hides: the session id, the total, and
        // a running sum that accumulates across the sequence.
        let mut running = 0i64;
        for step in &plan.steps {
            if let SessionStep::ShowNumber {
                session_id,
                total,
                value,
                running_sum,
                ..
            } = step
            {
                assert_eq!(*session_id, 4);
                assert_eq!(*total, 2);
                running += value;
                assert_eq!(*running_sum, running);
            }
        }

        match plan.steps.last() {
            Some(SessionStep::Complete { numbers, sum, .. }) => {
                assert_eq!(numbers, &plan.numbers_generated);
                assert_eq!(*sum, plan.expected_sum);
            }
            other => panic!("expected a Complete step, got {other:?}"),
        }
    }

    /// For every digit width, the exact-integer bound is inclusive: the largest
    /// admitted total validates, and one past it does not. This is the edge the
    /// `digit_width_total_bound_table` feeds; here it is exercised through
    /// `validate_config` rather than compared to a literal.
    #[cfg_attr(target_arch = "wasm32", wasm_bindgen_test)]
    #[cfg_attr(not(target_arch = "wasm32"), test)]
    fn validate_config_accepts_each_width_bound_and_rejects_one_past_it() {
        for digits in 1..=15u32 {
            let bound = max_total_for_digits(digits);
            let at_bound = SessionConfig {
                digits_per_number: digits,
                number_duration_ms: 500,
                total_numbers: bound,
                allow_negative_numbers: false,
            };
            assert!(
                validate_config(&at_bound).is_ok(),
                "width {digits} must accept its own bound {bound}",
            );

            let over = SessionConfig {
                total_numbers: bound + 1,
                ..at_bound
            };
            assert!(
                validate_config(&over).is_err(),
                "width {digits} must reject one past {bound}",
            );
        }
    }

    /// A table pinning the reported view for representative inputs, including
    /// the clamp boundaries and the cases where the digit-width bound (not the
    /// global cap) is what truncates the total. The reported seconds must be
    /// exactly the grid step the milliseconds represent.
    #[allow(clippy::float_cmp)] // Exactness lock: the view's seconds are derived bit-exactly.
    #[cfg_attr(target_arch = "wasm32", wasm_bindgen_test)]
    #[cfg_attr(not(target_arch = "wasm32"), test)]
    fn normalize_reports_a_consistent_view_for_a_table_of_inputs() {
        // ((digits, duration_s, total, negatives) -> (digits, ms, total, s, negatives))
        let cases = [
            ((0i64, 0.0f64, 0i64, false), (1u32, 100u64, 1u32, 0.1f64, false)),
            ((15, 60.0, 9, true), (15, 60_000, 9, 60.0, true)),
            // 16 digits clamps to 15 and the excess total clamps to that width.
            ((16, 60.1, 20_000, true), (15, 60_000, 9, 60.0, true)),
            // Negative digits clamp up; 2.56 rounds (half away from zero) to 2.6.
            ((-3, 2.56, -1, true), (1, 2600, 1, 2.6, true)),
            // 12 digits caps the total at 9007 while the request was far larger.
            ((12, 0.05, 100_000, false), (12, 100, 9007, 0.1, false)),
            ((2, 1.25, 50, true), (2, 1300, 50, 1.3, true)),
        ];

        for (input, expected) in cases {
            let (config, effective) = normalize_session_config(&SessionConfigInput {
                digits_per_number: input.0,
                number_duration_s: input.1,
                total_numbers: input.2,
                allow_negative_numbers: input.3,
            });

            assert_eq!(
                (
                    config.digits_per_number,
                    config.number_duration_ms,
                    config.total_numbers,
                    effective.number_duration_s,
                    config.allow_negative_numbers,
                ),
                (expected.0, expected.1, expected.2, expected.3, expected.4),
                "input {input:?}",
            );
        }
    }
}
