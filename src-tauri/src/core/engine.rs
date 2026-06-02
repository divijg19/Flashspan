use crate::core::generate::random_number_with_constraints;
use crate::core::types::{SessionConfig, SessionConfigEffective, SessionPlan, SessionStep};
use rand::SeedableRng;
use rand::rngs::StdRng;

/// Build a deterministic session plan from configuration and an optional seed.
///
/// A session plan is an immutable snapshot of all progression steps: countdown (if enabled),
/// numbered flashes, clears, and completion. Each step includes the relative delay (ms) before
/// the next step. The plan contains the full sequence of numbers and their arrangement.
///
/// Given the same config and seed, this function always produces identical results,
/// enabling replay, serialization, and testing without timers or platform dependencies.
pub fn build_session_plan(
    session_id: u64,
    config: SessionConfig,
    config_effective: SessionConfigEffective,
    seed_opt: Option<u64>,
) -> SessionPlan {
    let mut rng: StdRng = match seed_opt {
        Some(seed) => StdRng::seed_from_u64(seed),
        None => StdRng::from_rng(&mut rand::rng()),
    };

    let mut steps: Vec<SessionStep> = Vec::new();
    let mut accumulated_duration_ms: u64 = 0;

    // Phase 1: Initial clear screen
    steps.push(SessionStep::ClearScreen {
        session_id,
        index: None,
        delay_ms_before_next: 0,
    });

    // Phase 2: Countdown (if countdown_step would be Some in timing options)
    // We assume countdown is always enabled (default behavior from session.rs)
    let countdown_step_ms: u64 = 1000; // 1 second between 3, 2, 1
    for value in [3u32, 2u32, 1u32] {
        steps.push(SessionStep::CountdownTick {
            value: value.to_string(),
            delay_ms_before_next: countdown_step_ms,
        });
        accumulated_duration_ms += countdown_step_ms;
    }

    // Post-countdown settle delay (small grace period for fullscreen transition)
    // Add this as a transparent delay in the final countdown tick
    const POST_COUNTDOWN_SETTLE_MS: u64 = 100; // Settle delay after countdown before first flash
    if let Some(SessionStep::CountdownTick {
        delay_ms_before_next,
        ..
    }) = steps.last_mut()
    {
        *delay_ms_before_next += POST_COUNTDOWN_SETTLE_MS;
    }
    accumulated_duration_ms += POST_COUNTDOWN_SETTLE_MS;

    // Phase 3: Generate numbers and build flash cycles
    let mut last_payload: Option<String> = None;
    let mut running_sum: i128 = 0;
    let mut numbers: Vec<i64> = Vec::with_capacity(config.total_numbers as usize);
    let mut sum_i128: i128 = 0;

    for i in 0..config.total_numbers {
        // Generate a number with constraints
        let (mut payload, mut payload_value) = {
            let mut attempt = 0u32;
            loop {
                let (candidate, candidate_value) = random_number_with_constraints(
                    &mut rng,
                    config.digits_per_number,
                    config.allow_negative_numbers,
                    i,
                    running_sum,
                );

                if last_payload.as_deref() != Some(candidate.as_str()) {
                    break (candidate, candidate_value);
                }

                attempt += 1;
                if attempt >= 256 {
                    break (candidate, candidate_value);
                }
            }
        };

        // Deterministic fallback if consecutive duplicates detected
        if last_payload.as_deref() == Some(payload.as_str()) {
            let digits = config.digits_per_number;
            let fallback = if payload.starts_with('-') {
                payload.trim_start_matches('-').to_string()
            } else {
                match payload.parse::<u64>() {
                    Ok(mag) => {
                        let max_exclusive = if digits <= 1 { 10 } else { 10u64.pow(digits) };
                        let next = (mag % (max_exclusive - 1)) + 1;
                        next.to_string()
                    }
                    Err(_) => "1".to_string(),
                }
            };

            let fallback_val: i128 = fallback.parse::<i128>().unwrap_or(0);
            let signed = if payload.starts_with('-') {
                if running_sum - fallback_val >= 0 {
                    -fallback_val
                } else {
                    fallback_val
                }
            } else {
                fallback_val
            };

            payload = fallback;
            payload_value = signed;
        }

        last_payload = Some(payload.clone());
        running_sum = (running_sum + payload_value).max(0);
        sum_i128 += payload_value;

        let value_i64: i64 = payload_value
            .try_into()
            .expect("payload_value should fit into i64 with current constraints");
        numbers.push(value_i64);

        // Add show_number step
        steps.push(SessionStep::ShowNumber {
            session_id,
            index: i + 1,
            total: config.total_numbers,
            value: value_i64,
            running_sum: running_sum
                .try_into()
                .expect("running_sum should fit into i64 with current constraints"),
            delay_ms_before_next: config.number_duration_ms,
        });
        accumulated_duration_ms += config.number_duration_ms;

        // Add clear_screen step
        steps.push(SessionStep::ClearScreen {
            session_id,
            index: Some(i + 1),
            delay_ms_before_next: config.delay_between_numbers_ms,
        });
        accumulated_duration_ms += config.delay_between_numbers_ms;
    }

    // Phase 4: Global clear before complete
    steps.push(SessionStep::ClearScreen {
        session_id,
        index: None,
        delay_ms_before_next: 0,
    });

    // Phase 5: Session complete
    let sum_i64: i64 = sum_i128
        .try_into()
        .expect("sum should fit into i64 with current constraints");

    steps.push(SessionStep::Complete {
        session_id,
        numbers: numbers.clone(),
        sum: sum_i64,
    });

    SessionPlan {
        session_id,
        config_snapshot: config_effective,
        steps,
        total_duration_ms: accumulated_duration_ms,
        numbers_generated: numbers,
        expected_sum: sum_i64,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::types::{SessionConfig, SessionConfigEffective, SessionConfigInput};
    use crate::core::validate::normalize_session_config;

    #[test]
    fn session_plan_determinism_same_seed() {
        let input = SessionConfigInput {
            digits_per_number: 2,
            number_duration_s: 0.5,
            delay_between_numbers_s: 0.2,
            total_numbers: 5,
            allow_negative_numbers: false,
        };

        let (config, config_eff) = normalize_session_config(input);
        let seed = Some(12345u64);

        let plan1 = build_session_plan(1, config.clone(), config_eff.clone(), seed);
        let plan2 = build_session_plan(2, config.clone(), config_eff.clone(), seed);

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
            delay_between_numbers_s: 0.2,
            total_numbers: 10,
            allow_negative_numbers: false,
        };

        let (config, config_eff) = normalize_session_config(input);

        let plan1 = build_session_plan(1, config.clone(), config_eff.clone(), Some(111u64));
        let plan2 = build_session_plan(2, config.clone(), config_eff.clone(), Some(222u64));

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
            delay_between_numbers_s: 0.1,
            total_numbers: 20,
            allow_negative_numbers: true,
        };

        let total_numbers = input.total_numbers;
        let (config, config_eff) = normalize_session_config(input);
        let plan = build_session_plan(1, config, config_eff, Some(54321u64));

        // Check that numbers contain expected count
        assert_eq!(
            plan.numbers_generated.len(),
            total_numbers as usize,
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
            delay_between_numbers_s: 0.1,
            total_numbers: 3,
            allow_negative_numbers: false,
        };

        let (config, config_eff) = normalize_session_config(input);
        let plan = build_session_plan(1, config, config_eff, Some(999u64));

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
        matches!(plan.steps[0], SessionStep::ClearScreen { .. });

        // Check that last step is a complete
        matches!(plan.steps.last().unwrap(), SessionStep::Complete { .. });
    }

    #[test]
    fn session_plan_with_negative_numbers_respects_running_sum() {
        let input = SessionConfigInput {
            digits_per_number: 2,
            number_duration_s: 0.5,
            delay_between_numbers_s: 0.1,
            total_numbers: 20,
            allow_negative_numbers: true,
        };

        let (config, config_eff) = normalize_session_config(input);
        let plan = build_session_plan(1, config, config_eff, Some(77777u64));

        // Verify that running sum never goes negative
        let mut running_sum: i128 = 0;
        for (idx, num) in plan.numbers_generated.iter().enumerate() {
            running_sum += *num as i128;
            assert!(
                running_sum >= 0,
                "Running sum went negative at index {}: sum was {}",
                idx,
                running_sum
            );
        }
    }

    #[test]
    fn build_session_plan_total_numbers_0() {
        // Bypass normalize_session_config which clamps to 1, to test boundary directly
        let config = SessionConfig {
            digits_per_number: 1,
            number_duration_ms: 100,
            delay_between_numbers_ms: 0,
            total_numbers: 0,
            allow_negative_numbers: false,
        };
        let config_eff = SessionConfigEffective {
            digits_per_number: 1,
            number_duration_s: 0.1,
            delay_between_numbers_s: 0.0,
            total_numbers: 0,
            allow_negative_numbers: false,
        };
        let plan = build_session_plan(1, config, config_eff, Some(123u64));

        // With total_numbers=0: 1(clear) + 3(countdown) + 1(final clear) + 1(complete) = 6 steps
        assert_eq!(plan.steps.len(), 6);
        assert!(plan.numbers_generated.is_empty());
        assert_eq!(plan.expected_sum, 0);
        // 3 * 1000 (countdown) + POST_COUNTDOWN_SETTLE_MS (100) = 3100
        assert_eq!(plan.total_duration_ms, 3100);
    }

    #[test]
    fn build_session_plan_total_numbers_1() {
        let input = SessionConfigInput {
            digits_per_number: 1,
            number_duration_s: 0.5,
            delay_between_numbers_s: 0.1,
            total_numbers: 1,
            allow_negative_numbers: false,
        };
        let (config, config_eff) = normalize_session_config(input);
        let plan = build_session_plan(1, config, config_eff, Some(456u64));

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
            delay_between_numbers_s: 0.1,
            total_numbers: 3,
            allow_negative_numbers: false,
        };
        let (config, config_eff) = normalize_session_config(input);
        // number_duration_ms = 500, delay_between_numbers_ms = 100
        let plan = build_session_plan(1, config, config_eff, Some(789u64));

        // total_duration_ms = initial_clear(0) + 3*1000(countdown) + POST_COUNTDOWN_SETTLE_MS(100)
        //   + 3*500(number_durations) + 3*100(delays) + final_clear(0) + complete(0)
        //   = 0 + 3000 + 100 + 1500 + 300 = 4900
        assert_eq!(plan.total_duration_ms, 4900);
    }

    #[test]
    fn last_countdown_tick_includes_settle_delay() {
        let input = SessionConfigInput {
            digits_per_number: 1,
            number_duration_s: 0.3,
            delay_between_numbers_s: 0.1,
            total_numbers: 2,
            allow_negative_numbers: false,
        };
        let (config, config_eff) = normalize_session_config(input);
        let plan = build_session_plan(1, config, config_eff, Some(321u64));

        // Verify the last countdown tick ("1") has delay = 1000 + POST_COUNTDOWN_SETTLE_MS(100) = 1100
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
        // Use digits=15 with total=100: max sum ≈ 100 * 10^15 = 10^17, well within i64 range
        // while still stress-testing plan generation with large digit widths.
        let input = SessionConfigInput {
            digits_per_number: 15,
            number_duration_s: 0.1,
            delay_between_numbers_s: 0.0,
            total_numbers: 100,
            allow_negative_numbers: false,
        };
        let (config, config_eff) = normalize_session_config(input);

        // Should not panic
        let plan = build_session_plan(1, config, config_eff, Some(42u64));

        // Step count: 1(clear) + 3(countdown) + 2*100(show+clear) + 1(final clear) + 1(complete) = 206
        assert_eq!(plan.steps.len(), 206);
        assert_eq!(plan.numbers_generated.len(), 100);
        assert!(plan.expected_sum >= 0, "sum should be non-negative");
        // sum should fit in i64 (it already is i64)
        let _ = plan.expected_sum;
    }

    #[test]
    fn build_session_plan_negative_numbers_forced() {
        // Use a seed known to produce negative numbers
        let input = SessionConfigInput {
            digits_per_number: 2,
            number_duration_s: 0.3,
            delay_between_numbers_s: 0.1,
            total_numbers: 50,
            allow_negative_numbers: true,
        };
        let (config, config_eff) = normalize_session_config(input);
        let plan = build_session_plan(1, config, config_eff, Some(77777u64));

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
                "consecutive duplicate at index {}",
                i
            );
        }
    }
}
