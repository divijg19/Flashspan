use crate::core::types::{
    AutoRepeatPlan, ClearScreen, SessionComplete, SessionConfig, SessionConfigEffective,
    SessionPlan, SessionStep, ShowNumber,
};
use crate::core::{engine::build_session_plan, validate::validate_config};
use log::warn;
use std::time::{SystemTime, UNIX_EPOCH};
use std::{
    collections::VecDeque,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    thread,
    thread::JoinHandle,
    time::{Duration, Instant},
};

fn now_epoch_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

pub trait SessionEmitter {
    fn clear_screen(&self, payload: ClearScreen);
    fn countdown_tick(&self, value: String);
    fn show_number(&self, payload: ShowNumber);
    fn session_complete(&self, payload: SessionComplete);
}

#[derive(Debug, Clone)]
pub enum SessionState {
    Idle,
    ShowingNumbers {
        #[allow(dead_code)]
        current: u32,
        #[allow(dead_code)]
        total: u32,
    },
    Complete,
}

pub struct SessionManager {
    state: Arc<Mutex<SessionState>>,
    worker: Mutex<Option<JoinHandle<()>>>,
    stop: Mutex<Option<Arc<AtomicBool>>>,
    next_session_id: AtomicU64,
    recent_results: Arc<Mutex<VecDeque<SessionComplete>>>,
    auto_repeat_plan: Arc<Mutex<Option<AutoRepeatPlan>>>,
    auto_repeat_generation: AtomicU64,
}

impl Default for SessionManager {
    fn default() -> Self {
        Self {
            state: Arc::new(Mutex::new(SessionState::Idle)),
            worker: Mutex::new(None),
            stop: Mutex::new(None),
            next_session_id: AtomicU64::new(1),
            recent_results: Arc::new(Mutex::new(VecDeque::new())),
            auto_repeat_plan: Arc::new(Mutex::new(None)),
            auto_repeat_generation: AtomicU64::new(1),
        }
    }
}

pub(crate) fn recover_lock<'a, T>(mutex: &'a Mutex<T>, name: &str) -> std::sync::MutexGuard<'a, T> {
    mutex.lock().unwrap_or_else(|e| {
        warn!("mutex {} poisoned, recovering", name);
        e.into_inner()
    })
}

impl SessionManager {
    const MAX_RECENT_RESULTS: usize = 8;

    fn cleanup_finished_worker(&self) {
        let mut worker = recover_lock(&self.worker, "worker");
        if let Some(handle) = worker.as_ref()
            && handle.is_finished()
        {
            let handle = worker.take().expect("just checked Some");
            let _ = handle.join();
            *recover_lock(&self.stop, "stop") = None;
        }
    }

    pub fn start_with_emitter<E: SessionEmitter + Send + 'static>(
        &self,
        emitter: E,
        config: SessionConfig,
    ) -> Result<u64, String> {
        self.cleanup_finished_worker();

        validate_config(&config)?;

        {
            let worker = recover_lock(&self.worker, "worker");
            if let Some(handle) = worker.as_ref()
                && !handle.is_finished()
            {
                return Err("session already running".to_string());
            }
        }

        let stop_flag = Arc::new(AtomicBool::new(false));
        *recover_lock(&self.stop, "stop") = Some(stop_flag.clone());

        {
            let mut state = recover_lock(&self.state, "state");
            *state = SessionState::ShowingNumbers {
                current: 0,
                total: config.total_numbers,
            };
        }

        let session_id = self.next_session_id.fetch_add(1, Ordering::SeqCst);

        let state_arc = Arc::clone(&self.state);
        let recent_results_arc = Arc::clone(&self.recent_results);
        let plan_arc = Arc::clone(&self.auto_repeat_plan);
        let handle = std::thread::Builder::new()
            .name("session-worker".into())
            .spawn(move || {
                run_session_loop(
                    emitter,
                    config,
                    state_arc,
                    stop_flag,
                    session_id,
                    recent_results_arc,
                    plan_arc,
                );
            })
            .map_err(|e| format!("failed to spawn session worker: {}", e))?;

        *recover_lock(&self.worker, "worker") = Some(handle);
        Ok(session_id)
    }

    pub fn configure_auto_repeat(&self, plan: Option<AutoRepeatPlan>) {
        warn!(
            "[auto-repeat] configure_auto_repeat: plan={:?}",
            plan.as_ref().map(|p| (p.remaining, p.delay_ms))
        );
        *recover_lock(&self.auto_repeat_plan, "auto_repeat_plan") = plan;
        self.auto_repeat_generation.fetch_add(1, Ordering::SeqCst);
    }

    pub fn auto_repeat_generation(&self) -> u64 {
        self.auto_repeat_generation.load(Ordering::SeqCst)
    }

    pub fn result_for(&self, session_id: u64) -> Result<SessionComplete, String> {
        let guard = recover_lock(&self.recent_results, "recent_results");

        for result in guard.iter().rev() {
            if result.session_id == session_id {
                return Ok(result.clone());
            }
        }

        Err("session result not found".to_string())
    }

    pub fn mark_validated_and_schedule_info(
        &self,
        session_id: u64,
    ) -> Result<Option<(u64, u32, SessionConfig, u64)>, String> {
        let generation = self.auto_repeat_generation.load(Ordering::SeqCst);

        let (delay_ms, config, remaining_after_decrement) = {
            let mut plan_guard = recover_lock(&self.auto_repeat_plan, "auto_repeat_plan");
            let Some(plan) = plan_guard.as_mut() else {
                warn!("[auto-repeat] mark_validated_and_schedule_info: plan is None");
                return Ok(None);
            };
            warn!(
                "[auto-repeat] mark_validated_and_schedule_info: plan exists, awaiting={:?}, session_id={}, remaining={}",
                plan.awaiting_validation_session_id, session_id, plan.remaining
            );

            if plan.awaiting_validation_session_id != Some(session_id) {
                warn!(
                    "[auto-repeat] mark_validated_and_schedule_info: session_id mismatch (expected {:?}, got {})",
                    plan.awaiting_validation_session_id, session_id
                );
                return Ok(None);
            }

            if plan.remaining == 0 {
                warn!("[auto-repeat] mark_validated_and_schedule_info: remaining is 0");
                return Ok(None);
            }

            plan.awaiting_validation_session_id = None;
            plan.remaining = plan.remaining.saturating_sub(1);

            warn!(
                "[auto-repeat] mark_validated_and_schedule_info: success, remaining_after_decrement={}",
                plan.remaining
            );

            (plan.delay_ms, plan.config.clone(), plan.remaining)
        };

        Ok(Some((
            delay_ms,
            remaining_after_decrement,
            config,
            generation,
        )))
    }

    pub fn stop(&self) {
        self.cleanup_finished_worker();

        self.configure_auto_repeat(None);

        let stop_flag = recover_lock(&self.stop, "stop").take();
        if let Some(flag) = stop_flag {
            flag.store(true, Ordering::SeqCst);
        }
        recover_lock(&self.recent_results, "recent_results").clear();

        if let Some(handle) = recover_lock(&self.worker, "worker").take() {
            let _ = handle.join();
        }

        let mut state = recover_lock(&self.state, "state");
        *state = SessionState::Idle;
    }
}

fn run_session_loop<E: SessionEmitter + Send + 'static>(
    emitter: E,
    config: SessionConfig,
    state: Arc<Mutex<SessionState>>,
    stop: Arc<AtomicBool>,
    session_id: u64,
    recent_results: Arc<Mutex<VecDeque<SessionComplete>>>,
    auto_repeat_plan: Arc<Mutex<Option<AutoRepeatPlan>>>,
) {
    // Convert SessionConfig to SessionConfigEffective for plan generation.
    let config_effective = SessionConfigEffective {
        digits_per_number: config.digits_per_number,
        number_duration_s: config.number_duration_ms as f64 / 1000.0,
        delay_between_numbers_s: config.delay_between_numbers_ms as f64 / 1000.0,
        total_numbers: config.total_numbers,
        allow_negative_numbers: config.allow_negative_numbers,
    };

    // Generate deterministic session plan.
    let plan = build_session_plan(session_id, config, config_effective, None);

    // Execute plan using the new plan-based executor.
    run_session_plan(
        &emitter,
        plan,
        state,
        stop,
        recent_results,
        auto_repeat_plan,
        || {
            // Play beep for each number flash (non-blocking)
            let _ = crate::audio::play_kind("beep");
        },
    );
}

fn sleep_until_interruptible(deadline: Instant, stop: &AtomicBool) {
    while Instant::now() < deadline {
        if stop.load(Ordering::SeqCst) {
            return;
        }

        let now = Instant::now();
        let remaining = deadline.saturating_duration_since(now);
        let step = remaining.min(Duration::from_millis(10));
        thread::sleep(step);
    }
}

/// Execute a deterministic session plan produced by the core.
/// This function converts the immutable plan steps into runtime events and handles scheduling.
fn run_session_plan<E: SessionEmitter>(
    emitter: &E,
    plan: SessionPlan,
    state: Arc<Mutex<SessionState>>,
    stop: Arc<AtomicBool>,
    recent_results: Arc<Mutex<VecDeque<SessionComplete>>>,
    auto_repeat_plan: Arc<Mutex<Option<AutoRepeatPlan>>>,
    beep: impl Fn(),
) {
    const FIRST_FLASH_GRACE: Duration = Duration::from_millis(100);

    // Iterate through steps and execute them with relative delays
    for (step_idx, step) in plan.steps.iter().enumerate() {
        // Check stop signal before processing each step
        if stop.load(Ordering::SeqCst) {
            emitter.clear_screen(ClearScreen {
                session_id: plan.session_id,
                index: None,
                emitted_at_ms: now_epoch_ms(),
            });
            let mut st = recover_lock(&*state, "state");
            *st = SessionState::Idle;
            return;
        }

        match step {
            SessionStep::CountdownTick {
                value,
                delay_ms_before_next,
            } => {
                emitter.countdown_tick(value.clone());
                let delay = Duration::from_millis(*delay_ms_before_next);
                sleep_until_interruptible(Instant::now() + delay, &stop);
            }

            SessionStep::ShowNumber {
                session_id,
                index,
                total,
                value,
                running_sum,
                delay_ms_before_next,
            } => {
                // Determine if this is the first flash to apply grace period.
                // First flash comes after initial clear + 3 countdown ticks (step indices 0-3)
                let is_first_flash = step_idx == 4;
                let grace = if is_first_flash {
                    FIRST_FLASH_GRACE
                } else {
                    Duration::from_millis(0)
                };

                emitter.show_number(ShowNumber {
                    session_id: *session_id,
                    index: *index,
                    total: *total,
                    value: *value,
                    running_sum: *running_sum,
                    emitted_at_ms: now_epoch_ms(),
                });
                beep();

                let delay = Duration::from_millis(*delay_ms_before_next) + grace;
                sleep_until_interruptible(Instant::now() + delay, &stop);

                let mut st = recover_lock(&*state, "state");
                *st = SessionState::ShowingNumbers {
                    current: *index,
                    total: *total,
                };
            }

            SessionStep::ClearScreen {
                session_id,
                index,
                delay_ms_before_next,
            } => {
                emitter.clear_screen(ClearScreen {
                    session_id: *session_id,
                    index: *index,
                    emitted_at_ms: now_epoch_ms(),
                });

                let delay = Duration::from_millis(*delay_ms_before_next);
                if delay > Duration::from_millis(0) {
                    sleep_until_interruptible(Instant::now() + delay, &stop);
                }
            }

            SessionStep::Complete {
                session_id,
                numbers,
                sum,
            } => {
                let result = SessionComplete {
                    session_id: *session_id,
                    numbers: numbers.clone(),
                    sum: *sum,
                };

                {
                    let mut guard = recover_lock(&*recent_results, "recent_results");
                    guard.push_back(result.clone());
                    while guard.len() > SessionManager::MAX_RECENT_RESULTS {
                        guard.pop_front();
                    }
                }
                emitter.session_complete(result);

                {
                    let mut plan_guard = recover_lock(&*auto_repeat_plan, "auto_repeat_plan");
                    if let Some(ar_plan) = plan_guard.as_mut()
                        && ar_plan.remaining > 0
                    {
                        warn!(
                            "[auto-repeat] run_session_plan Complete: setting awaiting_validation_session_id={}, remaining_before={}",
                            *session_id, ar_plan.remaining
                        );
                        ar_plan.awaiting_validation_session_id = Some(*session_id);
                    } else {
                        warn!(
                            "[auto-repeat] run_session_plan Complete: NOT setting — plan={:?}",
                            plan_guard
                                .as_ref()
                                .map(|p| (p.remaining, p.awaiting_validation_session_id))
                        );
                    }
                }
            }
        }

        // Check for stop signal after each step
        if stop.load(Ordering::SeqCst) {
            emitter.clear_screen(ClearScreen {
                session_id: plan.session_id,
                index: None,
                emitted_at_ms: now_epoch_ms(),
            });
            let mut st = recover_lock(&*state, "state");
            *st = SessionState::Idle;
            return;
        }
    }

    let mut st = recover_lock(&*state, "state");
    *st = SessionState::Complete;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::generate::{
        random_fixed_digits_no_leading_zero, random_fixed_digits_no_leading_zero_capped,
        random_number_with_constraints,
    };
    use crate::core::types::{SessionConfig, SessionConfigInput};
    use crate::core::validate::normalize_session_config;
    use rand::rng;
    use std::sync::Arc;

    #[test]
    fn generator_respects_invariants() {
        let mut rng = rng();
        let digits = 3;
        let allow_neg = true;
        let mut last: Option<String> = None;
        let mut running_sum: i128 = 0;

        for i in 0..1000u32 {
            // Mimic the run_session_loop sampling behavior: retry until we get a different
            // payload or hit the attempt cap, then apply the deterministic fallback.
            let mut attempt = 0u32;
            let (s, val) = loop {
                let (candidate, candidate_value) =
                    random_number_with_constraints(&mut rng, digits, allow_neg, i, running_sum);
                if last.as_deref() != Some(candidate.as_str()) {
                    break (candidate, candidate_value);
                }
                attempt += 1;
                if attempt >= 256 {
                    // deterministic fallback similar to run_session_loop
                    let fallback = if candidate.starts_with('-') {
                        candidate.trim_start_matches('-').to_string()
                    } else {
                        match candidate.parse::<u64>() {
                            Ok(mag) => {
                                let max_exclusive =
                                    if digits <= 1 { 10 } else { 10u64.pow(digits) };
                                let next = (mag % (max_exclusive - 1)) + 1;
                                next.to_string()
                            }
                            Err(_) => "1".to_string(),
                        }
                    };
                    let fb_val: i128 = fallback.parse::<i128>().unwrap_or(0);
                    let signed = if candidate.starts_with('-') {
                        if running_sum - fb_val >= 0 {
                            -fb_val
                        } else {
                            fb_val
                        }
                    } else {
                        fb_val
                    };
                    break (fallback, signed);
                }
            };

            if let Some(prev) = &last {
                assert_ne!(prev, &s, "consecutive duplicate at {}", i);
            }

            if i == 0 {
                assert!(!s.starts_with('-'), "first number negative");
            }

            running_sum = (running_sum + val).max(0);
            assert!(running_sum >= 0, "running sum went negative at {}", i);

            last = Some(s);
        }
    }

    #[test]
    fn normalize_session_config_clamps_and_rounds() {
        let input = SessionConfigInput {
            digits_per_number: 0,
            number_duration_s: 0.049, // below min
            delay_between_numbers_s: f64::NAN,
            total_numbers: -5,
            allow_negative_numbers: true,
        };

        let (cfg, eff) = normalize_session_config(input);
        assert_eq!(cfg.digits_per_number, 1);
        assert!(cfg.number_duration_ms >= 1 && cfg.number_duration_ms <= 60_000);
        // effective rounds to 1 decimal place
        assert_eq!(
            eff.number_duration_s,
            (cfg.number_duration_ms as f64 / 1000.0 * 10.0).round() / 10.0
        );
        assert_eq!(cfg.total_numbers, 1);
        assert!(eff.allow_negative_numbers);
    }

    #[test]
    fn random_fixed_digits_no_leading_zero_basic() {
        let mut rng = rng();
        // digits=1 should produce 1..=9
        for _ in 0..50 {
            let s = random_fixed_digits_no_leading_zero(&mut rng, 1);
            let v: u32 = s.parse().unwrap();
            assert!((1..=9).contains(&v));
        }

        // digits=3 should be in [100,999]
        for _ in 0..50 {
            let s = random_fixed_digits_no_leading_zero(&mut rng, 3);
            let v: u64 = s.parse().unwrap();
            assert!((100..=999).contains(&v));
        }
    }

    #[test]
    fn random_fixed_digits_no_leading_zero_capped_behaviour() {
        let mut rng = rng();

        // When max_inclusive < min for digits > 1, expect None
        let none = random_fixed_digits_no_leading_zero_capped(&mut rng, 3, 50);
        assert!(none.is_none());

        // For digits=1 with max_inclusive < 1 -> None
        let maybe = random_fixed_digits_no_leading_zero_capped(&mut rng, 1, 0);
        assert!(maybe.is_none());

        // For digits=1 with max_inclusive >=1 -> Some within range
        let some = random_fixed_digits_no_leading_zero_capped(&mut rng, 1, 5).unwrap();
        let v: u64 = some.parse().unwrap();
        assert!((1..=5).contains(&v));
    }

    #[test]
    fn random_number_with_constraints_first_non_negative_and_respects_running_sum() {
        let mut rng = rng();
        let digits = 2;
        let allow_neg = true;
        let mut running_sum: i128 = 0;

        for i in 0..200u32 {
            let (s, val) =
                random_number_with_constraints(&mut rng, digits, allow_neg, i, running_sum);
            if i == 0 {
                assert!(!s.starts_with('-'), "first number negative");
            }
            // magnitude should parse
            let _parsed: i128 = s.trim_start_matches('-').parse().unwrap();
            // If negative, applying it must not drop running_sum below zero when the function returned it as negative
            if val < 0 {
                assert!(running_sum - (-val) >= 0 || running_sum == 0);
            }
            running_sum = (running_sum + val).max(0);
        }
    }

    #[test]
    fn session_manager_auto_repeat_and_mark_validated_flow() {
        let manager = SessionManager::default();

        // prepare a minimal valid SessionConfig
        let (config, _eff) = normalize_session_config(SessionConfigInput {
            digits_per_number: 1,
            number_duration_s: 0.1,
            delay_between_numbers_s: 0.0,
            total_numbers: 1,
            allow_negative_numbers: false,
        });

        let initial_gen = manager.auto_repeat_generation();

        let plan = AutoRepeatPlan {
            remaining: 3,
            delay_ms: 1500,
            config: config.clone(),
            awaiting_validation_session_id: Some(42),
        };

        manager.configure_auto_repeat(Some(plan));
        assert!(manager.auto_repeat_generation() > initial_gen);

        // mark_validated should return scheduling info for session_id 42
        let res = manager.mark_validated_and_schedule_info(42).unwrap();
        assert!(res.is_some());
        let (delay_ms, remaining_after, cfg, generation) = res.unwrap();
        assert_eq!(delay_ms, 1500);
        assert_eq!(remaining_after, 2);
        assert_eq!(cfg.digits_per_number, config.digits_per_number);
        assert_eq!(generation, manager.auto_repeat_generation());

        // subsequent call for same id should return None (awaiting_validation_session_id cleared)
        let res2 = manager.mark_validated_and_schedule_info(42).unwrap();
        assert!(res2.is_none());
    }

    #[test]
    fn mark_validated_none_conditions() {
        let manager = SessionManager::default();
        let (config, _eff) = normalize_session_config(SessionConfigInput {
            digits_per_number: 1,
            number_duration_s: 0.1,
            delay_between_numbers_s: 0.0,
            total_numbers: 1,
            allow_negative_numbers: false,
        });

        // plan without awaiting_validation_session_id -> should return None
        let plan = AutoRepeatPlan {
            remaining: 2,
            delay_ms: 1000,
            config: config.clone(),
            awaiting_validation_session_id: None,
        };
        manager.configure_auto_repeat(Some(plan));
        let res = manager.mark_validated_and_schedule_info(123).unwrap();
        assert!(res.is_none());

        // plan with remaining == 0 -> None
        let plan2 = AutoRepeatPlan {
            remaining: 0,
            delay_ms: 1000,
            config: config.clone(),
            awaiting_validation_session_id: Some(123),
        };
        manager.configure_auto_repeat(Some(plan2));
        let res2 = manager.mark_validated_and_schedule_info(123).unwrap();
        assert!(res2.is_none());
    }

    #[test]
    fn result_for_and_stop_clears_state_and_results() {
        let manager = SessionManager::default();

        // insert a fake recent result
        let sc = SessionComplete {
            session_id: 99,
            numbers: vec![1, 2, 3],
            sum: 6,
        };

        {
            let mut guard = manager.recent_results.lock().expect("lock poisoned");
            guard.push_back(sc.clone());
        }

        let got = manager.result_for(99).expect("should find result");
        assert_eq!(got.sum, 6);

        // configure auto-repeat and a worker thread to ensure stop() clears them and joins
        let (config, _eff) = normalize_session_config(SessionConfigInput {
            digits_per_number: 1,
            number_duration_s: 0.1,
            delay_between_numbers_s: 0.0,
            total_numbers: 1,
            allow_negative_numbers: false,
        });

        manager.configure_auto_repeat(Some(AutoRepeatPlan {
            remaining: 1,
            delay_ms: 10,
            config: config.clone(),
            awaiting_validation_session_id: None,
        }));

        // set a stop flag and spawn a short-lived thread as worker
        let stop_flag = Arc::new(AtomicBool::new(false));
        *manager.stop.lock().expect("stop lock") = Some(stop_flag.clone());

        let handle = std::thread::spawn(move || {
            // do a brief sleep to simulate work
            std::thread::sleep(std::time::Duration::from_millis(5));
        });

        *manager.worker.lock().expect("worker lock") = Some(handle);

        manager.stop();

        // recent_results should be cleared
        let guard = manager.recent_results.lock().expect("lock poisoned");
        assert!(guard.is_empty());

        // auto_repeat_plan cleared
        let plan_guard = manager.auto_repeat_plan.lock().expect("lock poisoned");
        assert!(plan_guard.is_none());

        // state should be Idle
        let state_guard = manager.state.lock().expect("state lock");
        match &*state_guard {
            SessionState::Idle => {}
            _ => panic!("expected Idle state"),
        }
    }

    #[test]
    fn random_fixed_digits_edge_cases() {
        let mut rng = rng();

        // digits = 1, max_inclusive = 1 => must be "1"
        let s = random_fixed_digits_no_leading_zero_capped(&mut rng, 1, 1).unwrap();
        assert_eq!(s, "1");

        // digits = 2, max_inclusive = 9 -> None because min for 2 digits is 10
        assert!(random_fixed_digits_no_leading_zero_capped(&mut rng, 2, 9).is_none());
    }

    #[test]
    fn random_number_with_constraints_no_negative() {
        let mut rng = rng();
        let mut running_sum: i128 = 0;
        for i in 0..100u32 {
            let (s, val) = random_number_with_constraints(&mut rng, 2, false, i, running_sum);
            assert!(!s.starts_with('-'));
            assert!(val >= 0);
            running_sum += val;
        }
    }

    #[test]
    fn sleep_until_interruptible_returns_on_stop() {
        use std::sync::Arc;
        use std::sync::atomic::AtomicBool;
        let stop = Arc::new(AtomicBool::new(true));
        let deadline = Instant::now() + Duration::from_millis(500);
        let start = Instant::now();
        sleep_until_interruptible(deadline, &stop);
        let elapsed = Instant::now() - start;
        assert!(elapsed < Duration::from_millis(50));
    }

    #[test]
    fn validate_config_rejects_bad_values() {
        let bad = SessionConfig {
            digits_per_number: 0,
            number_duration_ms: 0,
            delay_between_numbers_ms: 0,
            total_numbers: 0,
            allow_negative_numbers: false,
        };
        assert!(validate_config(&bad).is_err());

        let too_many_digits = SessionConfig {
            digits_per_number: 19,
            number_duration_ms: 1000,
            delay_between_numbers_ms: 0,
            total_numbers: 1,
            allow_negative_numbers: false,
        };
        assert!(validate_config(&too_many_digits).is_err());

        let too_many_total = SessionConfig {
            digits_per_number: 2,
            number_duration_ms: 1000,
            delay_between_numbers_ms: 0,
            total_numbers: 20_000,
            allow_negative_numbers: false,
        };
        assert!(validate_config(&too_many_total).is_err());

        let too_long = SessionConfig {
            digits_per_number: 2,
            number_duration_ms: 120_000,
            delay_between_numbers_ms: 0,
            total_numbers: 1,
            allow_negative_numbers: false,
        };
        assert!(validate_config(&too_long).is_err());
    }

    #[test]
    fn normalize_session_config_upper_bounds() {
        let input = SessionConfigInput {
            digits_per_number: 100,
            number_duration_s: 120.0,
            delay_between_numbers_s: 120.0,
            total_numbers: 100_000,
            allow_negative_numbers: false,
        };

        let (cfg, eff) = normalize_session_config(input);
        assert!(cfg.digits_per_number <= 18);
        assert!(cfg.number_duration_ms <= 60_000);
        assert!(cfg.delay_between_numbers_ms <= 60_000);
        assert!(cfg.total_numbers <= 10_000);
        assert!(eff.number_duration_s <= 60.0);
    }

    #[test]
    fn random_fixed_digits_capped_exact_min() {
        let mut rng = rng();
        // digits=2, min = 10, max_inclusive = 10 -> should always produce "10"
        let res = random_fixed_digits_no_leading_zero_capped(&mut rng, 2, 10).unwrap();
        // must parse and equal 10
        let v: u64 = res.parse().unwrap();
        assert_eq!(v, 10);
    }

    #[test]
    fn random_fixed_digits_max_digits_length() {
        let mut rng = rng();
        // digits = 18 should produce string length 18
        let s = random_fixed_digits_no_leading_zero(&mut rng, 18);
        assert_eq!(s.len(), 18);
    }

    #[test]
    fn sleep_until_interruptible_waits_when_not_stopped() {
        use std::sync::Arc;
        use std::sync::atomic::AtomicBool;
        let stop = Arc::new(AtomicBool::new(false));
        let deadline = Instant::now() + Duration::from_millis(30);
        let start = Instant::now();
        sleep_until_interruptible(deadline, &stop);
        let elapsed = Instant::now() - start;
        assert!(elapsed >= Duration::from_millis(25));
    }

    #[test]
    fn stop_when_idle_is_safe() {
        let manager = SessionManager::default();
        // stop() on an idle manager should not panic or leave state inconsistent
        manager.stop();
        let state_guard = manager.state.lock().expect("state lock");
        assert!(matches!(*state_guard, SessionState::Idle));
    }

    #[test]
    fn configure_auto_repeat_resets_generation_multiple_times() {
        let manager = SessionManager::default();
        let (config, _eff) = normalize_session_config(SessionConfigInput {
            digits_per_number: 1,
            number_duration_s: 0.1,
            delay_between_numbers_s: 0.0,
            total_numbers: 1,
            allow_negative_numbers: false,
        });

        let base_gen = manager.auto_repeat_generation();

        for i in 0..5 {
            let plan = AutoRepeatPlan {
                remaining: i + 1,
                delay_ms: 100,
                config: config.clone(),
                awaiting_validation_session_id: None,
            };
            manager.configure_auto_repeat(Some(plan));
            assert!(
                manager.auto_repeat_generation() > base_gen,
                "Generation should increase after each configure_auto_repeat call (iteration {})",
                i
            );
        }

        manager.configure_auto_repeat(None);
        let gen_after_clear = manager.auto_repeat_generation();
        let plan_guard = manager.auto_repeat_plan.lock().expect("lock");
        assert!(plan_guard.is_none());
        assert!(gen_after_clear > base_gen);
    }

    #[test]
    fn mark_validated_with_wrong_session_id_returns_none() {
        let manager = SessionManager::default();
        let (config, _eff) = normalize_session_config(SessionConfigInput {
            digits_per_number: 1,
            number_duration_s: 0.1,
            delay_between_numbers_s: 0.0,
            total_numbers: 1,
            allow_negative_numbers: false,
        });

        let plan = AutoRepeatPlan {
            remaining: 3,
            delay_ms: 500,
            config,
            awaiting_validation_session_id: Some(42),
        };
        manager.configure_auto_repeat(Some(plan));

        // Wrong session id should return None
        let res = manager.mark_validated_and_schedule_info(99).unwrap();
        assert!(res.is_none());

        // None awaiting_validation_session_id also returns None
        let plan2 = AutoRepeatPlan {
            remaining: 1,
            delay_ms: 500,
            config: SessionConfig {
                digits_per_number: 1,
                number_duration_ms: 100,
                delay_between_numbers_ms: 0,
                total_numbers: 1,
                allow_negative_numbers: false,
            },
            awaiting_validation_session_id: None,
        };
        manager.configure_auto_repeat(Some(plan2));
        let res2 = manager.mark_validated_and_schedule_info(42).unwrap();
        assert!(res2.is_none());
    }

    struct TestEmitter {
        calls: Arc<Mutex<Vec<String>>>,
    }

    impl TestEmitter {
        fn new() -> Self {
            Self {
                calls: Arc::new(Mutex::new(Vec::new())),
            }
        }
    }

    impl SessionEmitter for TestEmitter {
        fn clear_screen(&self, _payload: ClearScreen) {
            self.calls.lock().unwrap().push("clear_screen".into());
        }
        fn countdown_tick(&self, value: String) {
            self.calls
                .lock()
                .unwrap()
                .push(format!("countdown({})", value));
        }
        fn show_number(&self, payload: ShowNumber) {
            self.calls
                .lock()
                .unwrap()
                .push(format!("show_number({})", payload.value));
        }
        fn session_complete(&self, _payload: SessionComplete) {
            self.calls.lock().unwrap().push("session_complete".into());
        }
    }

    fn make_sample_plan(session_id: u64) -> SessionPlan {
        SessionPlan {
            session_id,
            config_snapshot: SessionConfigEffective {
                digits_per_number: 1,
                number_duration_s: 0.1,
                delay_between_numbers_s: 0.0,
                total_numbers: 2,
                allow_negative_numbers: false,
            },
            steps: vec![
                SessionStep::ClearScreen {
                    session_id,
                    index: None,
                    delay_ms_before_next: 0,
                },
                SessionStep::CountdownTick {
                    value: "3".into(),
                    delay_ms_before_next: 0,
                },
                SessionStep::CountdownTick {
                    value: "2".into(),
                    delay_ms_before_next: 0,
                },
                SessionStep::CountdownTick {
                    value: "1".into(),
                    delay_ms_before_next: 0,
                },
                SessionStep::ShowNumber {
                    session_id,
                    index: 1,
                    total: 2,
                    value: 5,
                    running_sum: 5,
                    delay_ms_before_next: 0,
                },
                SessionStep::ClearScreen {
                    session_id,
                    index: Some(1),
                    delay_ms_before_next: 0,
                },
                SessionStep::ShowNumber {
                    session_id,
                    index: 2,
                    total: 2,
                    value: 3,
                    running_sum: 8,
                    delay_ms_before_next: 0,
                },
                SessionStep::ClearScreen {
                    session_id,
                    index: Some(2),
                    delay_ms_before_next: 0,
                },
                SessionStep::ClearScreen {
                    session_id,
                    index: None,
                    delay_ms_before_next: 0,
                },
                SessionStep::Complete {
                    session_id,
                    numbers: vec![5, 3],
                    sum: 8,
                },
            ],
            total_duration_ms: 0,
            numbers_generated: vec![5, 3],
            expected_sum: 8,
        }
    }

    #[test]
    fn run_session_plan_emits_events_in_correct_order() {
        let emitter = TestEmitter::new();
        let state = Arc::new(Mutex::new(SessionState::Idle));
        let stop = Arc::new(AtomicBool::new(false));
        let recent_results = Arc::new(Mutex::new(VecDeque::new()));
        let auto_repeat_plan = Arc::new(Mutex::new(None));
        let beep_count = Arc::new(std::sync::atomic::AtomicU32::new(0));
        let beep = {
            let bc = Arc::clone(&beep_count);
            move || {
                bc.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            }
        };

        let plan = make_sample_plan(42);
        run_session_plan(
            &emitter,
            plan,
            state,
            stop,
            recent_results,
            auto_repeat_plan,
            beep,
        );

        let calls = emitter.calls.lock().unwrap();
        assert_eq!(
            *calls,
            vec![
                "clear_screen",
                "countdown(3)",
                "countdown(2)",
                "countdown(1)",
                "show_number(5)",
                "clear_screen",
                "show_number(3)",
                "clear_screen",
                "clear_screen",
                "session_complete",
            ],
            "Events should be emitted in plan order"
        );
        assert_eq!(beep_count.load(std::sync::atomic::Ordering::SeqCst), 2);
    }

    #[test]
    fn run_session_plan_stop_before_start() {
        let emitter = TestEmitter::new();
        let state = Arc::new(Mutex::new(SessionState::Idle));
        let stop = Arc::new(AtomicBool::new(true));
        let recent_results = Arc::new(Mutex::new(VecDeque::new()));
        let auto_repeat_plan = Arc::new(Mutex::new(None));

        let plan = make_sample_plan(99);
        run_session_plan(
            &emitter,
            plan,
            Arc::clone(&state),
            stop,
            recent_results,
            auto_repeat_plan,
            || {},
        );

        let calls = emitter.calls.lock().unwrap();
        assert_eq!(*calls, vec!["clear_screen"]);
        let state_guard = state.lock().unwrap();
        assert!(matches!(*state_guard, SessionState::Idle));
    }

    #[test]
    fn run_session_plan_stores_results() {
        let emitter = TestEmitter::new();
        let state = Arc::new(Mutex::new(SessionState::Idle));
        let stop = Arc::new(AtomicBool::new(false));
        let recent_results = Arc::new(Mutex::new(VecDeque::new()));
        let auto_repeat_plan = Arc::new(Mutex::new(None));

        let plan = make_sample_plan(42);
        run_session_plan(
            &emitter,
            plan,
            Arc::clone(&state),
            stop,
            Arc::clone(&recent_results),
            auto_repeat_plan,
            || {},
        );

        // Check complete state
        let state_guard = state.lock().unwrap();
        assert!(matches!(*state_guard, SessionState::Complete));

        // Check results stored
        let results = recent_results.lock().unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].session_id, 42);
        assert_eq!(results[0].sum, 8);
        assert_eq!(results[0].numbers, vec![5, 3]);
    }

    #[test]
    fn run_session_plan_sets_awaiting_validation() {
        let emitter = TestEmitter::new();
        let state = Arc::new(Mutex::new(SessionState::Idle));
        let stop = Arc::new(AtomicBool::new(false));
        let recent_results = Arc::new(Mutex::new(VecDeque::new()));
        let auto_repeat_plan = Arc::new(Mutex::new(Some(AutoRepeatPlan {
            remaining: 3,
            delay_ms: 500,
            config: SessionConfig {
                digits_per_number: 1,
                number_duration_ms: 100,
                delay_between_numbers_ms: 0,
                total_numbers: 1,
                allow_negative_numbers: false,
            },
            awaiting_validation_session_id: None,
        })));

        let plan = make_sample_plan(42);
        run_session_plan(
            &emitter,
            plan,
            state,
            stop,
            recent_results,
            Arc::clone(&auto_repeat_plan),
            || {},
        );

        let plan_guard = auto_repeat_plan.lock().unwrap();
        let ar = plan_guard.as_ref().unwrap();
        assert_eq!(ar.awaiting_validation_session_id, Some(42));
        assert_eq!(ar.remaining, 3);
    }

    #[test]
    fn run_session_plan_does_not_set_awaiting_when_remaining_zero() {
        let emitter = TestEmitter::new();
        let state = Arc::new(Mutex::new(SessionState::Idle));
        let stop = Arc::new(AtomicBool::new(false));
        let recent_results = Arc::new(Mutex::new(VecDeque::new()));
        let auto_repeat_plan = Arc::new(Mutex::new(Some(AutoRepeatPlan {
            remaining: 0,
            delay_ms: 500,
            config: SessionConfig {
                digits_per_number: 1,
                number_duration_ms: 100,
                delay_between_numbers_ms: 0,
                total_numbers: 1,
                allow_negative_numbers: false,
            },
            awaiting_validation_session_id: None,
        })));

        let plan = make_sample_plan(42);
        run_session_plan(
            &emitter,
            plan,
            state,
            stop,
            recent_results,
            Arc::clone(&auto_repeat_plan),
            || {},
        );

        let plan_guard = auto_repeat_plan.lock().unwrap();
        assert!(
            plan_guard
                .as_ref()
                .unwrap()
                .awaiting_validation_session_id
                .is_none()
        );
    }

    #[test]
    fn random_fixed_digits_no_leading_zero_capped_max_boundary() {
        let mut rng = rng();
        // digits=2, max_inclusive=99 (upper bound for 2 digits)
        for _ in 0..50 {
            let res = random_fixed_digits_no_leading_zero_capped(&mut rng, 2, 99).unwrap();
            let v: u64 = res.parse().unwrap();
            assert!((10..=99).contains(&v), "value {} out of range [10, 99]", v);
        }
        // digits=3, max_inclusive=999 (upper bound for 3 digits)
        for _ in 0..50 {
            let res = random_fixed_digits_no_leading_zero_capped(&mut rng, 3, 999).unwrap();
            let v: u64 = res.parse().unwrap();
            assert!(
                (100..=999).contains(&v),
                "value {} out of range [100, 999]",
                v
            );
        }
    }

    #[test]
    fn random_number_with_constraints_digits_18() {
        let mut rng = rng();
        let mut running_sum: i128 = 0;
        for i in 0..50u32 {
            let (s, val) = random_number_with_constraints(&mut rng, 18, true, i, running_sum);
            // Strip leading '-' for length check (number can be negative)
            let magnitude_str = s.trim_start_matches('-');
            assert_eq!(
                magnitude_str.len(),
                18,
                "magnitude string length should be 18 for digits=18, got '{}'",
                s
            );
            let mag: i128 = magnitude_str.parse().unwrap();
            assert!(mag >= 10i128.pow(17), "magnitude too small for digits=18");
            assert!(mag < 10i128.pow(18), "magnitude too large for digits=18");
            let _ = i64::try_from(val).expect("val should fit in i64");
            if i == 0 {
                assert!(!s.starts_with('-'), "first number should not be negative");
            }
            running_sum = (running_sum + val).max(0);
            assert!(running_sum >= 0, "running sum went negative");
        }
    }

    #[test]
    fn result_for_not_found() {
        let manager = SessionManager::default();
        let err = manager.result_for(999).unwrap_err();
        assert_eq!(err, "session result not found");
    }

    #[test]
    fn run_session_plan_with_negative_numbers() {
        let emitter = TestEmitter::new();
        let state = Arc::new(Mutex::new(SessionState::Idle));
        let stop = Arc::new(AtomicBool::new(false));
        let recent_results = Arc::new(Mutex::new(VecDeque::new()));
        let auto_repeat_plan = Arc::new(Mutex::new(None));
        let beep_count = Arc::new(std::sync::atomic::AtomicU32::new(0));
        let beep = {
            let bc = Arc::clone(&beep_count);
            move || {
                bc.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            }
        };

        let plan = SessionPlan {
            session_id: 77,
            config_snapshot: SessionConfigEffective {
                digits_per_number: 1,
                number_duration_s: 0.1,
                delay_between_numbers_s: 0.0,
                total_numbers: 2,
                allow_negative_numbers: true,
            },
            steps: vec![
                SessionStep::ClearScreen {
                    session_id: 77,
                    index: None,
                    delay_ms_before_next: 0,
                },
                SessionStep::CountdownTick {
                    value: "3".into(),
                    delay_ms_before_next: 0,
                },
                SessionStep::CountdownTick {
                    value: "2".into(),
                    delay_ms_before_next: 0,
                },
                SessionStep::CountdownTick {
                    value: "1".into(),
                    delay_ms_before_next: 0,
                },
                SessionStep::ShowNumber {
                    session_id: 77,
                    index: 1,
                    total: 2,
                    value: -5,
                    running_sum: -5,
                    delay_ms_before_next: 0,
                },
                SessionStep::ClearScreen {
                    session_id: 77,
                    index: Some(1),
                    delay_ms_before_next: 0,
                },
                SessionStep::ShowNumber {
                    session_id: 77,
                    index: 2,
                    total: 2,
                    value: 3,
                    running_sum: -2,
                    delay_ms_before_next: 0,
                },
                SessionStep::ClearScreen {
                    session_id: 77,
                    index: Some(2),
                    delay_ms_before_next: 0,
                },
                SessionStep::ClearScreen {
                    session_id: 77,
                    index: None,
                    delay_ms_before_next: 0,
                },
                SessionStep::Complete {
                    session_id: 77,
                    numbers: vec![-5, 3],
                    sum: -2,
                },
            ],
            total_duration_ms: 0,
            numbers_generated: vec![-5, 3],
            expected_sum: -2,
        };

        run_session_plan(
            &emitter,
            plan,
            Arc::clone(&state),
            stop,
            Arc::clone(&recent_results),
            auto_repeat_plan,
            beep,
        );

        let calls = emitter.calls.lock().unwrap();
        assert_eq!(
            *calls,
            vec![
                "clear_screen",
                "countdown(3)",
                "countdown(2)",
                "countdown(1)",
                "show_number(-5)",
                "clear_screen",
                "show_number(3)",
                "clear_screen",
                "clear_screen",
                "session_complete",
            ]
        );
        assert_eq!(beep_count.load(std::sync::atomic::Ordering::SeqCst), 2);

        let state_guard = state.lock().unwrap();
        assert!(matches!(*state_guard, SessionState::Complete));

        let results = recent_results.lock().unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].session_id, 77);
        assert_eq!(results[0].sum, -2);
        assert_eq!(results[0].numbers, vec![-5, 3]);
    }

    #[test]
    fn run_session_plan_with_zero_numbers() {
        let emitter = TestEmitter::new();
        let state = Arc::new(Mutex::new(SessionState::Idle));
        let stop = Arc::new(AtomicBool::new(false));
        let recent_results = Arc::new(Mutex::new(VecDeque::new()));
        let auto_repeat_plan = Arc::new(Mutex::new(None));

        let plan = SessionPlan {
            session_id: 100,
            config_snapshot: SessionConfigEffective {
                digits_per_number: 1,
                number_duration_s: 0.1,
                delay_between_numbers_s: 0.0,
                total_numbers: 0,
                allow_negative_numbers: false,
            },
            steps: vec![
                SessionStep::ClearScreen {
                    session_id: 100,
                    index: None,
                    delay_ms_before_next: 0,
                },
                SessionStep::CountdownTick {
                    value: "3".into(),
                    delay_ms_before_next: 0,
                },
                SessionStep::CountdownTick {
                    value: "2".into(),
                    delay_ms_before_next: 0,
                },
                SessionStep::CountdownTick {
                    value: "1".into(),
                    delay_ms_before_next: 0,
                },
                SessionStep::ClearScreen {
                    session_id: 100,
                    index: None,
                    delay_ms_before_next: 0,
                },
                SessionStep::Complete {
                    session_id: 100,
                    numbers: vec![],
                    sum: 0,
                },
            ],
            total_duration_ms: 3100,
            numbers_generated: vec![],
            expected_sum: 0,
        };

        run_session_plan(
            &emitter,
            plan,
            Arc::clone(&state),
            stop,
            Arc::clone(&recent_results),
            auto_repeat_plan,
            || {},
        );

        let calls = emitter.calls.lock().unwrap();
        assert_eq!(
            *calls,
            vec![
                "clear_screen",
                "countdown(3)",
                "countdown(2)",
                "countdown(1)",
                "clear_screen",
                "session_complete",
            ]
        );

        let state_guard = state.lock().unwrap();
        assert!(matches!(*state_guard, SessionState::Complete));

        let results = recent_results.lock().unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].session_id, 100);
        assert_eq!(results[0].sum, 0);
        assert!(results[0].numbers.is_empty());
    }

    #[test]
    fn run_session_plan_with_total_numbers_1() {
        let emitter = TestEmitter::new();
        let state = Arc::new(Mutex::new(SessionState::Idle));
        let stop = Arc::new(AtomicBool::new(false));
        let recent_results = Arc::new(Mutex::new(VecDeque::new()));
        let auto_repeat_plan = Arc::new(Mutex::new(None));

        let plan = SessionPlan {
            session_id: 50,
            config_snapshot: SessionConfigEffective {
                digits_per_number: 1,
                number_duration_s: 0.1,
                delay_between_numbers_s: 0.0,
                total_numbers: 1,
                allow_negative_numbers: false,
            },
            steps: vec![
                SessionStep::ClearScreen {
                    session_id: 50,
                    index: None,
                    delay_ms_before_next: 0,
                },
                SessionStep::CountdownTick {
                    value: "3".into(),
                    delay_ms_before_next: 0,
                },
                SessionStep::CountdownTick {
                    value: "2".into(),
                    delay_ms_before_next: 0,
                },
                SessionStep::CountdownTick {
                    value: "1".into(),
                    delay_ms_before_next: 0,
                },
                SessionStep::ShowNumber {
                    session_id: 50,
                    index: 1,
                    total: 1,
                    value: 42,
                    running_sum: 42,
                    delay_ms_before_next: 0,
                },
                SessionStep::ClearScreen {
                    session_id: 50,
                    index: Some(1),
                    delay_ms_before_next: 0,
                },
                SessionStep::ClearScreen {
                    session_id: 50,
                    index: None,
                    delay_ms_before_next: 0,
                },
                SessionStep::Complete {
                    session_id: 50,
                    numbers: vec![42],
                    sum: 42,
                },
            ],
            total_duration_ms: 0,
            numbers_generated: vec![42],
            expected_sum: 42,
        };

        run_session_plan(
            &emitter,
            plan,
            Arc::clone(&state),
            stop,
            Arc::clone(&recent_results),
            auto_repeat_plan,
            || {},
        );

        let calls = emitter.calls.lock().unwrap();
        assert_eq!(
            *calls,
            vec![
                "clear_screen",
                "countdown(3)",
                "countdown(2)",
                "countdown(1)",
                "show_number(42)",
                "clear_screen",
                "clear_screen",
                "session_complete",
            ]
        );

        let state_guard = state.lock().unwrap();
        assert!(matches!(*state_guard, SessionState::Complete));

        let results = recent_results.lock().unwrap();
        assert_eq!(results[0].session_id, 50);
        assert_eq!(results[0].sum, 42);
        assert_eq!(results[0].numbers, vec![42]);
    }

    #[test]
    fn run_session_plan_stop_after_first_number() {
        let emitter = TestEmitter::new();
        let state = Arc::new(Mutex::new(SessionState::Idle));
        let stop = Arc::new(AtomicBool::new(false));
        let recent_results = Arc::new(Mutex::new(VecDeque::new()));
        let auto_repeat_plan = Arc::new(Mutex::new(None));
        let beep_count = Arc::new(std::sync::atomic::AtomicU32::new(0));

        let stop_clone = Arc::clone(&stop);
        let bc = Arc::clone(&beep_count);
        let beep = move || {
            bc.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            stop_clone.store(true, Ordering::SeqCst);
        };

        let plan = make_sample_plan(55);
        run_session_plan(
            &emitter,
            plan,
            Arc::clone(&state),
            Arc::clone(&stop),
            recent_results,
            auto_repeat_plan,
            beep,
        );

        let calls = emitter.calls.lock().unwrap();
        assert_eq!(
            *calls,
            vec![
                "clear_screen",
                "countdown(3)",
                "countdown(2)",
                "countdown(1)",
                "show_number(5)",
                "clear_screen",
            ]
        );

        assert_eq!(beep_count.load(std::sync::atomic::Ordering::SeqCst), 1);
        let state_guard = state.lock().unwrap();
        assert!(matches!(*state_guard, SessionState::Idle));
    }

    #[test]
    fn run_session_plan_full_auto_repeat_cycle() {
        let emitter = TestEmitter::new();
        let state = Arc::new(Mutex::new(SessionState::Idle));
        let stop = Arc::new(AtomicBool::new(false));
        let recent_results = Arc::new(Mutex::new(VecDeque::new()));

        let manager = SessionManager::default();
        let (config, _eff) = normalize_session_config(SessionConfigInput {
            digits_per_number: 1,
            number_duration_s: 0.1,
            delay_between_numbers_s: 0.0,
            total_numbers: 2,
            allow_negative_numbers: false,
        });

        // Step 1: Configure auto-repeat with remaining=1 via SessionManager
        manager.configure_auto_repeat(Some(AutoRepeatPlan {
            remaining: 1,
            delay_ms: 500,
            config: config.clone(),
            awaiting_validation_session_id: None,
        }));

        // Step 2: Run session plan using the SAME auto_repeat_plan Arc
        let plan = make_sample_plan(42);
        run_session_plan(
            &emitter,
            plan,
            Arc::clone(&state),
            stop,
            recent_results,
            Arc::clone(&manager.auto_repeat_plan),
            || {},
        );

        // Verify awaiting_validation_session_id was set by run_session_plan
        {
            let plan_guard = manager.auto_repeat_plan.lock().unwrap();
            let ar = plan_guard.as_ref().unwrap();
            assert_eq!(ar.awaiting_validation_session_id, Some(42));
            assert_eq!(ar.remaining, 1);
        }

        // Step 3: Call mark_validated_and_schedule_info to consume it
        let result = manager.mark_validated_and_schedule_info(42).unwrap();
        assert!(result.is_some());
        let (_delay_ms, remaining, _cfg, _gen) = result.unwrap();
        assert_eq!(remaining, 0, "remaining should be 0 after consuming");

        // Subsequent call should return None (awaiting already cleared)
        let result2 = manager.mark_validated_and_schedule_info(42).unwrap();
        assert!(result2.is_none());
    }

    #[test]
    fn start_with_emitter_creates_session() {
        let manager = SessionManager::default();
        let (config, _eff) = normalize_session_config(SessionConfigInput {
            digits_per_number: 1,
            number_duration_s: 0.1,
            delay_between_numbers_s: 0.0,
            total_numbers: 1,
            allow_negative_numbers: false,
        });

        let emitter = TestEmitter::new();
        let result = manager.start_with_emitter(emitter, config);
        assert!(result.is_ok());
        let session_id = result.unwrap();
        assert_eq!(session_id, 1, "first session should have id 1");

        // State should be ShowingNumbers immediately after start
        {
            let state_guard = manager.state.lock().unwrap();
            assert!(matches!(
                *state_guard,
                SessionState::ShowingNumbers {
                    current: 0,
                    total: 1
                }
            ));
        }

        // Stop and verify Idle state
        manager.stop();
        {
            let state_guard = manager.state.lock().unwrap();
            assert!(matches!(*state_guard, SessionState::Idle));
        }
    }

    #[test]
    fn start_with_emitter_rejects_concurrent() {
        let manager = SessionManager::default();
        let (config, _eff) = normalize_session_config(SessionConfigInput {
            digits_per_number: 1,
            number_duration_s: 0.1,
            delay_between_numbers_s: 0.0,
            total_numbers: 1,
            allow_negative_numbers: false,
        });

        // First start succeeds
        let result1 = manager.start_with_emitter(TestEmitter::new(), config.clone());
        assert!(result1.is_ok());

        // Second start should fail while first is running
        let result2 = manager.start_with_emitter(TestEmitter::new(), config.clone());
        assert!(result2.is_err());
        assert_eq!(
            result2.unwrap_err(),
            "session already running",
            "concurrent start should be rejected"
        );

        // Stop the first session
        manager.stop();

        // After stop, start should succeed again
        let result3 = manager.start_with_emitter(TestEmitter::new(), config);
        assert!(result3.is_ok());

        manager.stop();
    }

    #[test]
    fn generator_stress_10000_iterations() {
        let mut rng = rng();
        let digits = 3;
        let allow_neg = true;
        let mut last: Option<String> = None;
        let mut running_sum: i128 = 0;
        let mut fallback_count: u32 = 0;

        for i in 0..10000u32 {
            let mut attempt = 0u32;
            let (s, val) = loop {
                let (candidate, candidate_value) =
                    random_number_with_constraints(&mut rng, digits, allow_neg, i, running_sum);
                if last.as_deref() != Some(candidate.as_str()) {
                    break (candidate, candidate_value);
                }
                attempt += 1;
                if attempt >= 256 {
                    fallback_count += 1;
                    let fallback = if candidate.starts_with('-') {
                        candidate.trim_start_matches('-').to_string()
                    } else {
                        match candidate.parse::<u64>() {
                            Ok(mag) => {
                                let max_exclusive =
                                    if digits <= 1 { 10 } else { 10u64.pow(digits) };
                                let next = (mag % (max_exclusive - 1)) + 1;
                                next.to_string()
                            }
                            Err(_) => "1".to_string(),
                        }
                    };
                    let fb_val: i128 = fallback.parse::<i128>().unwrap_or(0);
                    let signed = if candidate.starts_with('-') {
                        if running_sum - fb_val >= 0 {
                            -fb_val
                        } else {
                            fb_val
                        }
                    } else {
                        fb_val
                    };
                    break (fallback, signed);
                }
            };

            if let Some(prev) = &last {
                assert_ne!(prev, &s, "consecutive duplicate at {}", i);
            }

            if i == 0 {
                assert!(!s.starts_with('-'), "first number negative at {}", i);
            }

            running_sum = (running_sum + val).max(0);
            assert!(running_sum >= 0, "running sum went negative at {}", i);

            last = Some(s);
        }

        // Fallback should rarely trigger; verify it doesn't dominate
        assert!(
            fallback_count < 100,
            "fallback triggered {} times in 10000 iterations (should be rare)",
            fallback_count
        );
    }

    #[test]
    fn session_manager_stop_after_mark_validated() {
        let manager = SessionManager::default();
        let (config, _eff) = normalize_session_config(SessionConfigInput {
            digits_per_number: 1,
            number_duration_s: 0.1,
            delay_between_numbers_s: 0.0,
            total_numbers: 1,
            allow_negative_numbers: false,
        });

        // Configure auto-repeat with awaiting validation
        manager.configure_auto_repeat(Some(AutoRepeatPlan {
            remaining: 1,
            delay_ms: 500,
            config: config.clone(),
            awaiting_validation_session_id: Some(42),
        }));

        // Consume the awaiting via mark_validated_and_schedule_info
        let result = manager.mark_validated_and_schedule_info(42).unwrap();
        assert!(result.is_some());

        // Insert a fake result to verify results are preserved until stop
        {
            let mut guard = manager.recent_results.lock().unwrap();
            guard.push_back(SessionComplete {
                session_id: 42,
                numbers: vec![1, 2, 3],
                sum: 6,
            });
        }

        // Now stop — should clear everything
        manager.stop();

        // State becomes Idle
        {
            let state_guard = manager.state.lock().unwrap();
            assert!(matches!(*state_guard, SessionState::Idle));
        }

        // recent_results are cleared by stop()
        {
            let guard = manager.recent_results.lock().unwrap();
            assert!(guard.is_empty(), "stop() should clear recent_results");
        }

        // auto_repeat_plan is cleared by stop()
        {
            let plan_guard = manager.auto_repeat_plan.lock().unwrap();
            assert!(plan_guard.is_none(), "stop() should clear auto_repeat_plan");
        }
    }
}
