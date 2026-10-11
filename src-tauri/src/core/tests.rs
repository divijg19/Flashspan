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

    #[test]
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
    #[test]
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

    #[test]
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
    #[test]
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

    #[test]
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
    #[test]
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
    #[test]
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
    #[test]
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
    #[test]
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
}
