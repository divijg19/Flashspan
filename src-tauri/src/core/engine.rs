use crate::core::generate::{magnitude_bounds, random_number_with_constraints};
use crate::core::timing::{INTER_NUMBER_GAP_MS, PRE_FLASH_SETTLE_MS};
use crate::core::types::{SessionConfig, SessionConfigEffective, SessionPlan, SessionStep};
use crate::core::validate::effective_from;
use rand::SeedableRng;
use rand::rngs::StdRng;

/// Countdown ticks ("3", "2", "1") and the gap between them (ms). Mirrors the
/// schedule documented in `crate::core::timing`.
const COUNTDOWN_TICK_MS: u64 = 1000;
const COUNTDOWN_FROM: u32 = 3;

/// Random draws spent trying to avoid two identical numbers in a row before
/// falling back to [`next_distinct_value`]. Two consecutive duplicates become
/// vanishingly unlikely long before this, but the fallback must stay total.
const MAX_DUPLICATE_RETRIES: u32 = 256;

/// Deterministic replacement for a generated value that equals the previous
/// one: rotate the magnitude one step inside its own digit-width domain.
///
/// The result provably (a) keeps the digit width and the no-leading-zero rule,
/// (b) differs from `value`, and (c) keeps the negative form only while the
/// running sum allows it. For a single-digit width this is exactly the
/// magnitude rotation the generator has always used.
fn next_distinct_value(value: i128, running_sum: i128, digits: u32) -> i128 {
    let (min, max) = magnitude_bounds(digits);
    let min = u128::from(min);
    let span = u128::from(max) - min + 1;
    let rotated = i128::try_from(min + (value.unsigned_abs().saturating_sub(min) + 1) % span)
        .expect("rotated magnitude is at most 10^19 and fits i128");

    if value < 0 && running_sum >= rotated {
        -rotated
    } else {
        rotated
    }
}

/// Build a deterministic session plan from configuration and an optional seed.
///
/// A session plan is an immutable snapshot of all progression steps: countdown (always
/// emitted), numbered flashes, clears, and completion. Each step includes the relative
/// delay (ms) before the next step. The plan contains the full sequence of numbers and
/// their arrangement.
///
/// Given the same config and seed, this function always produces identical results,
/// enabling replay, serialization, and testing without timers or platform dependencies.
///
/// # Panics
///
/// Panics if `config` violates the normalization bounds (digits, totals, or
/// sums outside the exact-integer budget); normalized configs never trigger
/// this. See `crate::core::validate`.
#[must_use]
pub fn build_session_plan(
    session_id: u64,
    config: &SessionConfig,
    config_effective: SessionConfigEffective,
    seed_opt: Option<u64>,
) -> SessionPlan {
    // The snapshot is stored verbatim, so check it against the config instead
    // of trusting the caller's pairing.
    debug_assert_eq!(
        config_effective,
        effective_from(config),
        "config_snapshot must be the effective view of the config it is built from"
    );

    let mut rng: StdRng =
        seed_opt.map_or_else(|| StdRng::from_rng(&mut rand::rng()), StdRng::seed_from_u64);

    let mut steps: Vec<SessionStep> = Vec::new();

    // Phase 1: Initial clear screen
    steps.push(SessionStep::ClearScreen {
        session_id,
        index: None,
        delay_ms_before_next: 0,
    });

    // Phase 2: Countdown (always emitted; see crate::core::timing). The final
    // tick also carries the pre-flash settle, so the first number paints late
    // and never short.
    for value in (1..=COUNTDOWN_FROM).rev() {
        let delay_ms_before_next =
            COUNTDOWN_TICK_MS + if value == 1 { PRE_FLASH_SETTLE_MS } else { 0 };
        steps.push(SessionStep::CountdownTick {
            value: value.to_string(),
            delay_ms_before_next,
        });
    }

    // Phase 3: Generate numbers and build flash cycles
    let mut last_value: Option<i128> = None;
    let mut running_sum: i128 = 0;
    // `total_numbers <= 10_000` fits `usize` on every supported target.
    let mut numbers: Vec<i64> = Vec::with_capacity(
        usize::try_from(config.total_numbers).expect("total_numbers fits usize"),
    );

    for i in 0..config.total_numbers {
        // Generate a number with constraints, retrying while it would repeat
        // the previous number.
        let mut value = {
            let mut attempt = 0u32;
            loop {
                let candidate = random_number_with_constraints(
                    &mut rng,
                    config.digits_per_number,
                    config.allow_negative_numbers,
                    i,
                    running_sum,
                );

                if last_value != Some(candidate) {
                    break candidate;
                }

                attempt += 1;
                if attempt >= MAX_DUPLICATE_RETRIES {
                    break candidate;
                }
            }
        };

        // Retry budget spent: replace the repeat deterministically, inside the
        // digit width's own domain, so "never two identical numbers in a row"
        // holds for every seed rather than merely almost surely.
        if last_value == Some(value) {
            value = next_distinct_value(value, running_sum, config.digits_per_number);
        }

        last_value = Some(value);
        running_sum += value;
        debug_assert!(
            running_sum >= 0,
            "negative draws are capped by the running sum, so it cannot go negative"
        );

        // Safe by construction: normalization caps digits at 15 (every value
        // < 2^53) and bounds total_numbers so the worst-case sum stays below
        // 2^53 - 1 (see MAX_EXACT_INTEGER), far inside i64 range.
        let value_i64: i64 = value
            .try_into()
            .expect("value exceeds i64; normalization bound violated");
        numbers.push(value_i64);

        // Add show_number step
        steps.push(SessionStep::ShowNumber {
            session_id,
            index: i + 1,
            total: config.total_numbers,
            value: value_i64,
            running_sum: running_sum
                .try_into()
                .expect("running_sum exceeds i64; normalization bound violated"),
            delay_ms_before_next: config.number_duration_ms,
        });

        // Add clear_screen step with the fixed inter-number gap.
        steps.push(SessionStep::ClearScreen {
            session_id,
            index: Some(i + 1),
            delay_ms_before_next: INTER_NUMBER_GAP_MS,
        });
    }

    // Phase 4: Global clear before complete
    steps.push(SessionStep::ClearScreen {
        session_id,
        index: None,
        delay_ms_before_next: 0,
    });

    // Phase 5: Session complete. The total is derived from the numbers that
    // were actually produced (bound proof: see max_total_for_digits), so the
    // plan's own copies of the sequence and its sum cannot disagree.
    let sum_i64 = numbers
        .iter()
        .try_fold(0i64, |acc, number| acc.checked_add(*number))
        .expect("sum exceeds i64; normalization bound violated");

    steps.push(SessionStep::Complete {
        session_id,
        numbers: numbers.clone(),
        sum: sum_i64,
    });

    let total_duration_ms: u64 = steps.iter().map(SessionStep::delay_ms).sum();

    SessionPlan {
        session_id,
        config_snapshot: config_effective,
        steps,
        total_duration_ms,
        numbers_generated: numbers,
        expected_sum: sum_i64,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::types::{SessionConfig, SessionConfigEffective, SessionConfigInput};
    use crate::core::validate::{MAX_EXACT_INTEGER, normalize_session_config};

    /// The exact-integer budget as a signed bound, for exactness assertions.
    fn max_exact() -> i64 {
        i64::try_from(MAX_EXACT_INTEGER).expect("2^53 - 1 fits i64")
    }

    #[test]
    fn session_plan_determinism_same_seed() {
        let input = SessionConfigInput {
            digits_per_number: 2,
            number_duration_s: 0.5,
            total_numbers: 5,
            allow_negative_numbers: false,
        };

        let (config, config_eff) = normalize_session_config(&input);
        let seed = Some(12345u64);

        let plan1 = build_session_plan(1, &config, config_eff.clone(), seed);
        let plan2 = build_session_plan(2, &config, config_eff, seed);

        // Same seed should produce identical numbers and sum
        assert_eq!(
            plan1.numbers_generated, plan2.numbers_generated,
            "Same seed should produce identical number sequences"
        );
        assert_eq!(
            plan1.expected_sum, plan2.expected_sum,
            "Same seed should produce identical sum"
        );
    }

    #[test]
    fn session_plan_different_seeds_produce_different_numbers() {
        let input = SessionConfigInput {
            digits_per_number: 2,
            number_duration_s: 0.5,
            total_numbers: 10,
            allow_negative_numbers: false,
        };

        let (config, config_eff) = normalize_session_config(&input);

        let plan1 = build_session_plan(1, &config, config_eff.clone(), Some(111u64));
        let plan2 = build_session_plan(2, &config, config_eff, Some(222u64));

        // Different seeds should (very likely) produce different sequences
        assert_ne!(
            plan1.numbers_generated, plan2.numbers_generated,
            "Different seeds should produce different sequences"
        );
    }

    #[test]
    fn session_plan_respects_invariants() {
        let input = SessionConfigInput {
            digits_per_number: 3,
            number_duration_s: 0.5,
            total_numbers: 20,
            allow_negative_numbers: true,
        };

        let total_numbers = input.total_numbers;
        let (config, config_eff) = normalize_session_config(&input);
        let plan = build_session_plan(1, &config, config_eff, Some(54321u64));

        // Check that numbers contain expected count
        assert_eq!(
            plan.numbers_generated.len(),
            usize::try_from(total_numbers).expect("test total fits usize"),
            "Plan should generate correct number of numbers"
        );

        // First number should never be negative
        assert!(
            plan.numbers_generated[0] >= 0,
            "First number should never be negative"
        );

        // Check for consecutive duplicates
        for i in 1..plan.numbers_generated.len() {
            assert_ne!(
                plan.numbers_generated[i],
                plan.numbers_generated[i - 1],
                "Numbers at indices {} and {} are consecutive duplicates",
                i - 1,
                i
            );
        }

        // Verify sum calculation
        let calculated_sum: i64 = plan.numbers_generated.iter().sum();
        assert_eq!(
            plan.expected_sum, calculated_sum,
            "Expected sum should match sum of generated numbers"
        );
    }

    #[test]
    fn session_plan_step_structure() {
        let input = SessionConfigInput {
            digits_per_number: 1,
            number_duration_s: 0.5,
            total_numbers: 3,
            allow_negative_numbers: false,
        };

        let (config, config_eff) = normalize_session_config(&input);
        let plan = build_session_plan(1, &config, config_eff, Some(999u64));

        // Verify step sequence structure
        // Expected: ClearScreen (initial) + 3x CountdownTick + 3x (ShowNumber + ClearScreen) + ClearScreen (final) + Complete
        // = 1 + 3 + 6 + 1 + 1 = 12 steps
        let expected_step_count = 1 + 3 + (3 * 2) + 1 + 1;
        assert_eq!(
            plan.steps.len(),
            expected_step_count,
            "Plan should have correct number of steps"
        );

        // Check that first step is a clear
        assert!(matches!(plan.steps[0], SessionStep::ClearScreen { .. }));

        // Check that last step is a complete
        assert!(matches!(
            plan.steps.last().expect("plan always has steps"),
            SessionStep::Complete { .. }
        ));
    }

    #[test]
    fn session_plan_with_negative_numbers_respects_running_sum() {
        let input = SessionConfigInput {
            digits_per_number: 2,
            number_duration_s: 0.5,
            total_numbers: 20,
            allow_negative_numbers: true,
        };

        let (config, config_eff) = normalize_session_config(&input);
        let plan = build_session_plan(1, &config, config_eff, Some(77777u64));

        // Verify that running sum never goes negative
        let mut running_sum: i128 = 0;
        for (idx, num) in plan.numbers_generated.iter().enumerate() {
            running_sum += i128::from(*num);
            assert!(
                running_sum >= 0,
                "Running sum went negative at index {idx}: sum was {running_sum}"
            );
        }
    }

    #[test]
    fn build_session_plan_total_numbers_0() {
        // Bypass normalize_session_config which clamps to 1, to test boundary directly
        let config = SessionConfig {
            digits_per_number: 1,
            number_duration_ms: 100,
            total_numbers: 0,
            allow_negative_numbers: false,
        };
        let config_eff = SessionConfigEffective {
            digits_per_number: 1,
            number_duration_s: 0.1,
            total_numbers: 0,
            allow_negative_numbers: false,
        };
        let plan = build_session_plan(1, &config, config_eff, Some(123u64));

        // With total_numbers=0: 1(clear) + 3(countdown) + 1(final clear) + 1(complete) = 6 steps
        assert_eq!(plan.steps.len(), 6);
        assert_eq!(plan.numbers_generated, Vec::<i64>::new());
        assert_eq!(plan.expected_sum, 0);
        // 3 * 1000 (countdown) + PRE_FLASH_SETTLE_MS (100) = 3100
        assert_eq!(plan.total_duration_ms, 3100);
    }

    #[test]
    fn build_session_plan_total_numbers_1() {
        let input = SessionConfigInput {
            digits_per_number: 1,
            number_duration_s: 0.5,
            total_numbers: 1,
            allow_negative_numbers: false,
        };
        let (config, config_eff) = normalize_session_config(&input);
        let plan = build_session_plan(1, &config, config_eff, Some(456u64));

        // With total_numbers=1: 1 + 3 + 2 + 1 + 1 = 8 steps
        assert_eq!(plan.steps.len(), 8);
        assert_eq!(plan.numbers_generated.len(), 1);
        assert!(
            plan.numbers_generated[0] >= 0,
            "First number should be non-negative"
        );
        if let SessionStep::ShowNumber { index, .. } = &plan.steps[4] {
            assert_eq!(*index, 1, "First ShowNumber should have index 1");
        } else {
            panic!("Step 4 should be ShowNumber");
        }
    }

    #[test]
    fn build_session_plan_timing_includes_post_countdown_settle() {
        let input = SessionConfigInput {
            digits_per_number: 1,
            number_duration_s: 0.5,
            total_numbers: 3,
            allow_negative_numbers: false,
        };
        let (config, config_eff) = normalize_session_config(&input);
        // number_duration_ms = 500, fixed gap = 100 (INTER_NUMBER_GAP_MS)
        let plan = build_session_plan(1, &config, config_eff, Some(789u64));

        // total_duration_ms = initial_clear(0) + 3*1000(countdown) + PRE_FLASH_SETTLE_MS(100)
        //   + 3*500(number_durations) + 3*100(delays) + final_clear(0) + complete(0)
        //   = 0 + 3000 + 100 + 1500 + 300 = 4900
        assert_eq!(plan.total_duration_ms, 4900);
    }

    #[test]
    fn last_countdown_tick_includes_settle_delay() {
        let input = SessionConfigInput {
            digits_per_number: 1,
            number_duration_s: 0.3,
            total_numbers: 2,
            allow_negative_numbers: false,
        };
        let (config, config_eff) = normalize_session_config(&input);
        let plan = build_session_plan(1, &config, config_eff, Some(321u64));

        // Verify the last countdown tick ("1") has delay = 1000 + PRE_FLASH_SETTLE_MS(100) = 1100
        if let SessionStep::CountdownTick {
            value,
            delay_ms_before_next,
            ..
        } = &plan.steps[3]
        {
            assert_eq!(value, "1");
            assert_eq!(*delay_ms_before_next, 1100);
        } else {
            panic!("Step 3 should be CountdownTick");
        }

        // Other countdown ticks should have delay = 1000
        if let SessionStep::CountdownTick {
            delay_ms_before_next,
            ..
        } = &plan.steps[1]
        {
            assert_eq!(*delay_ms_before_next, 1000);
        } else {
            panic!("Step 1 should be CountdownTick");
        }
    }

    #[test]
    fn build_session_plan_max_config() {
        // digits=15 allows at most 9 numbers (exact-integer bound); a request
        // for 100 must clamp to 9 and still build without panic.
        let input = SessionConfigInput {
            digits_per_number: 15,
            number_duration_s: 0.1,
            total_numbers: 100,
            allow_negative_numbers: false,
        };
        let (config, config_eff) = normalize_session_config(&input);
        assert_eq!(config.total_numbers, 9);

        // Should not panic
        let plan = build_session_plan(1, &config, config_eff, Some(42u64));

        // Step count: 1(clear) + 3(countdown) + 2*9(show+clear) + 1(final clear) + 1(complete) = 24
        assert_eq!(plan.steps.len(), 24);
        assert_eq!(plan.numbers_generated.len(), 9);
        assert!(plan.expected_sum >= 0, "sum should be non-negative");
        // Every value and the sum must stay exactly representable in f64.
        let max_exact = max_exact();
        for value in &plan.numbers_generated {
            assert!(value.abs() <= max_exact);
        }
        assert!(plan.expected_sum.abs() <= max_exact);
        assert_eq!(
            plan.expected_sum,
            plan.numbers_generated.iter().sum::<i64>()
        );
    }

    #[test]
    fn build_session_plan_negative_numbers_forced() {
        // Use a seed known to produce negative numbers
        let input = SessionConfigInput {
            digits_per_number: 2,
            number_duration_s: 0.3,
            total_numbers: 50,
            allow_negative_numbers: true,
        };
        let (config, config_eff) = normalize_session_config(&input);
        let plan = build_session_plan(1, &config, config_eff, Some(77777u64));

        assert_eq!(plan.numbers_generated.len(), 50);

        // Verify first number is non-negative
        assert!(
            plan.numbers_generated[0] >= 0,
            "first number should be non-negative"
        );

        // Verify at least one negative number exists
        let has_negative = plan.numbers_generated.iter().any(|&n| n < 0);
        assert!(
            has_negative,
            "expected at least one negative number with allow_negatives=true and seed 77777"
        );

        // Verify any two consecutive are never duplicates
        for i in 1..plan.numbers_generated.len() {
            assert_ne!(
                plan.numbers_generated[i],
                plan.numbers_generated[i - 1],
                "consecutive duplicate at index {i}"
            );
        }
    }

    #[test]
    fn inter_number_gap_is_fixed_and_exposure_is_uniform() {
        // The gap is fixed by the timing budget (see crate::core::timing),
        // not by user input: any session must produce it.
        for total in [1, 3] {
            let input = SessionConfigInput {
                digits_per_number: 1,
                number_duration_s: 0.5,
                total_numbers: total,
                allow_negative_numbers: false,
            };
            let (config, eff) = normalize_session_config(&input);
            let plan = build_session_plan(1, &config, eff, Some(7u64));

            // Each ShowNumber exposure must equal 500ms, including the first
            // flash (no first-flash bonus).
            let show_delays: Vec<u64> = plan
                .steps
                .iter()
                .filter_map(|s| match s {
                    SessionStep::ShowNumber {
                        delay_ms_before_next,
                        ..
                    } => Some(*delay_ms_before_next),
                    _ => None,
                })
                .collect();
            assert_eq!(
                show_delays,
                vec![500; usize::try_from(total).expect("test total fits usize")]
            );

            // Each indexed clear must carry the fixed 100ms blank gap.
            let clear_delays: Vec<u64> = plan
                .steps
                .iter()
                .filter_map(|s| match s {
                    SessionStep::ClearScreen {
                        index: Some(_),
                        delay_ms_before_next,
                        ..
                    } => Some(*delay_ms_before_next),
                    _ => None,
                })
                .collect();
            assert_eq!(
                clear_delays,
                vec![100; usize::try_from(total).expect("test total fits usize")]
            );
        }
    }

    #[test]
    fn next_distinct_value_rotates_inside_the_digit_width() {
        for digits in [1u32, 2, 3, 15] {
            let (min, max) = crate::core::generate::magnitude_bounds(digits);
            // Every value in the domain must map to a different value that is
            // still inside the same domain: that is what makes the
            // no-consecutive-duplicates rule total rather than probable.
            for magnitude in [min, min + 1, min.midpoint(max), max - 1, max] {
                for value in [i128::from(magnitude), -i128::from(magnitude)] {
                    let next = next_distinct_value(value, i128::MAX, digits);
                    assert_ne!(next, value, "rotation did not change {value}");
                    let rotated = next.unsigned_abs();
                    assert!(
                        rotated >= u128::from(min) && rotated <= u128::from(max),
                        "{next} left the {digits}-digit domain [{min}, {max}]"
                    );
                    assert_eq!(
                        next < 0,
                        value < 0,
                        "an affordable negative must stay negative"
                    );
                }
            }
        }
    }

    #[test]
    fn next_distinct_value_flips_sign_when_the_sum_cannot_afford_it() {
        // A negative replacement is only legal while the running sum can
        // absorb it; otherwise the magnitude is taken as a positive.
        let next = next_distinct_value(-42, 0, 2);
        assert_eq!(next, 43);

        let next = next_distinct_value(-42, 43, 2);
        assert_eq!(next, -43);
    }

    #[test]
    fn countdown_order_and_settle_live_on_the_final_tick() {
        let input = SessionConfigInput {
            digits_per_number: 1,
            number_duration_s: 0.1,
            total_numbers: 1,
            allow_negative_numbers: false,
        };
        let (config, config_eff) = normalize_session_config(&input);
        let plan = build_session_plan(1, &config, config_eff, Some(5u64));

        let ticks: Vec<(String, u64)> = plan
            .steps
            .iter()
            .filter_map(|step| match step {
                SessionStep::CountdownTick {
                    value,
                    delay_ms_before_next,
                } => Some((value.clone(), *delay_ms_before_next)),
                _ => None,
            })
            .collect();

        assert_eq!(
            ticks,
            vec![
                ("3".to_string(), 1000),
                ("2".to_string(), 1000),
                ("1".to_string(), 1000 + PRE_FLASH_SETTLE_MS),
            ]
        );
    }
}
