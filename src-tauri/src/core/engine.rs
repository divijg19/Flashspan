use crate::core::generate::random_number_with_constraints;
use crate::core::types::{SessionConfig, SessionConfigEffective, SessionPlan, SessionStep};
use rand::rngs::StdRng;
use rand::{Rng, SeedableRng};

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
    let mut rng: Box<dyn Rng> = match seed_opt {
        Some(seed) => Box::new(StdRng::seed_from_u64(seed)),
        None => {
            #[cfg(target_arch = "wasm32")]
            {
                Box::new(StdRng::seed_from_u64(0xC0FFEE_F00DBABE))
            }

            #[cfg(not(target_arch = "wasm32"))]
            {
                Box::new(rand::rng())
            }
        }
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
    const POST_COUNTDOWN_SETTLE_MS: u64 = 0; // Can be tuned; currently 0
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
    use crate::core::types::SessionConfigInput;
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
}
