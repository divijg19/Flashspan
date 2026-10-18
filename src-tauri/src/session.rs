use crate::core::types::{
    AutoRepeatPlan, ClearScreen, SessionComplete, SessionConfig, SessionPlan, SessionStep,
    ShowNumber,
};
use crate::core::{
    engine::build_session_plan,
    validate::{effective_from, validate_config},
};
use log::warn;
use std::time::{SystemTime, UNIX_EPOCH};
use std::{
    collections::VecDeque,
    sync::{
        Arc, Condvar, Mutex,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    thread::JoinHandle,
    time::{Duration, Instant},
};

/// A duration's length in milliseconds, saturating rather than truncating.
///
/// `Duration::as_millis` is `u128` and pedantic clippy rightly rejects a blind
/// cast. Saturating is the right semantic for a countdown: an absurdly long
/// duration should read as "effectively forever", not wrap to a small number
/// that looks like the wait is nearly over.
pub(crate) fn millis_u64(duration: Duration) -> u64 {
    u64::try_from(duration.as_millis()).unwrap_or(u64::MAX)
}

/// Whole seconds left in a delay, rounding up so a countdown never reads 0
/// before the delay has actually elapsed.
pub(crate) fn seconds_left_for(remaining: Duration) -> u64 {
    millis_u64(remaining).div_ceil(1000)
}

/// Whether an auto-repeat wait observed at `observed_generation` may still
/// start a session.
///
/// `configure_auto_repeat(None)` bumps the generation, so a stale auto-repeat
/// thread must abandon its restart rather than start a session the user has
/// cancelled — or that a manual Start has already begun.
pub(crate) fn generation_is_current(current: u64, observed: u64) -> bool {
    current == observed
}

/// Drives an auto-repeat wait: one tick per distinct whole second remaining,
/// then a final "0 seconds" tick, then permission to restart.
///
/// `on_tick` is called *as each tick is produced*, never batched. The countdown
/// exists to reach the UI while it is still counting down, so the emission
/// cannot be deferred: a caller that collected the ticks and replayed them once
/// the wait returned would deliver the whole sequence in one burst at the
/// deadline, leaving the bar frozen on its first value until the session
/// restarted. Handing the caller the ticks instead of a sink invited exactly
/// that, so the sink is the only shape on offer.
///
/// The generation is re-checked at three points — each poll iteration, once the
/// delay has elapsed, and immediately before the restart is allowed. Without
/// that last check a cancel landing in the gap between the delay and `start()`
/// would still start a session.
///
/// The clock, the generation probe and the poll step are injected so this is
/// deterministic and needs no threads or real waiting: the live caller
/// supplies a real deadline and a real sleep, the test supplies a countdown and
/// a bump.
///
/// Returns whether the restart may go ahead.
pub(crate) fn drive_auto_repeat_wait(
    observed_generation: u64,
    remaining_ms: &mut dyn FnMut() -> u64,
    generation: &mut dyn FnMut() -> u64,
    mut on_tick: impl FnMut(u64),
    mut step: impl FnMut(),
) -> bool {
    let mut last: Option<u64> = None;
    loop {
        if !generation_is_current(generation(), observed_generation) {
            return false;
        }
        let remaining = remaining_ms();
        if remaining == 0 {
            break;
        }
        let seconds_left = seconds_left_for(Duration::from_millis(remaining));
        if last != Some(seconds_left) {
            last = Some(seconds_left);
            on_tick(seconds_left);
        }
        step();
    }

    if !generation_is_current(generation(), observed_generation) {
        return false;
    }
    on_tick(0);
    if !generation_is_current(generation(), observed_generation) {
        return false;
    }
    true
}

pub(crate) fn now_epoch_ms() -> u64 {
    u64::try_from(
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis(),
    )
    .expect("epoch millis fits u64 for hundreds of millions of years")
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
    ShowingNumbers,
    Complete,
}

pub struct SessionManager {
    state: Arc<Mutex<SessionState>>,
    worker: Mutex<Option<JoinHandle<()>>>,
    stop: Mutex<Option<Arc<StopSignal>>>,
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
        warn!("mutex {name} poisoned, recovering");
        e.into_inner()
    })
}

impl SessionManager {
    const MAX_RECENT_RESULTS: usize = 8;

    /// Join a finished worker, if any, and drop its stop flag.
    ///
    /// The handle is taken out from under the lock before joining so a caller
    /// never holds the worker mutex while waiting on a thread.
    fn cleanup_finished_worker(&self) {
        let handle = {
            let mut worker = recover_lock(&self.worker, "worker");
            if worker.as_ref().is_some_and(JoinHandle::is_finished) {
                worker.take()
            } else {
                None
            }
        };

        if let Some(handle) = handle {
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

        let stop_signal = Arc::new(StopSignal::new());
        *recover_lock(&self.stop, "stop") = Some(Arc::clone(&stop_signal));

        {
            let mut state = recover_lock(&self.state, "state");
            *state = SessionState::ShowingNumbers;
        }

        let session_id = self.next_session_id.fetch_add(1, Ordering::SeqCst);

        let state_arc = Arc::clone(&self.state);
        let recent_results_arc = Arc::clone(&self.recent_results);
        let plan_arc = Arc::clone(&self.auto_repeat_plan);
        let spawn = std::thread::Builder::new()
            .name("session-worker".into())
            .spawn(move || {
                run_session_loop(
                    emitter,
                    config,
                    state_arc,
                    stop_signal,
                    session_id,
                    recent_results_arc,
                    plan_arc,
                );
            });

        let handle = match spawn {
            Ok(handle) => handle,
            Err(e) => {
                // Nothing will run: roll the manager back to the state a
                // caller observes before any session starts, so a failed spawn
                // cannot leave the manager stuck in "ShowingNumbers" or leave a
                // stop flag behind for the next session.
                *recover_lock(&self.stop, "stop") = None;
                *recover_lock(&self.state, "state") = SessionState::Idle;
                return Err(format!("failed to spawn session worker: {e}"));
            }
        };

        *recover_lock(&self.worker, "worker") = Some(handle);
        Ok(session_id)
    }

    pub fn configure_auto_repeat(&self, plan: Option<AutoRepeatPlan>) {
        warn!(
            target: "auto-repeat",
            "configure_auto_repeat: plan={:?}",
            plan.as_ref().map(|p| (p.remaining, p.delay_ms))
        );
        let mut plan_guard = recover_lock(&self.auto_repeat_plan, "auto_repeat_plan");
        *plan_guard = plan;
        // Bumped while still holding the plan lock, so any reader that pairs
        // the plan with a generation (see `mark_validated_and_schedule_info`)
        // observes both before or both after this update, never a mix.
        self.auto_repeat_generation.fetch_add(1, Ordering::SeqCst);
    }

    pub fn auto_repeat_generation(&self) -> u64 {
        self.auto_repeat_generation.load(Ordering::SeqCst)
    }

    pub fn result_for(&self, session_id: u64) -> Result<SessionComplete, String> {
        let guard = recover_lock(&self.recent_results, "recent_results");
        guard
            .iter()
            .rev()
            .find(|result| result.session_id == session_id)
            .cloned()
            .ok_or_else(|| "session result not found".to_string())
    }

    pub fn mark_validated_and_schedule_info(
        &self,
        session_id: u64,
    ) -> Option<(u64, u32, SessionConfig, u64)> {
        let (delay_ms, config, remaining_after_decrement, generation) = {
            let mut plan_guard = recover_lock(&self.auto_repeat_plan, "auto_repeat_plan");
            let Some(plan) = plan_guard.as_mut() else {
                warn!(target: "auto-repeat", "mark_validated_and_schedule_info: plan is None");
                return None;
            };
            warn!(
                target: "auto-repeat",
                "mark_validated_and_schedule_info: plan exists, awaiting={:?}, session_id={}, remaining={}",
                plan.awaiting_validation_session_id, session_id, plan.remaining
            );

            if plan.awaiting_validation_session_id != Some(session_id) {
                warn!(
                    target: "auto-repeat",
                    "mark_validated_and_schedule_info: session_id mismatch (expected {:?}, got {})",
                    plan.awaiting_validation_session_id, session_id
                );
                return None;
            }

            if plan.remaining == 0 {
                warn!(target: "auto-repeat", "mark_validated_and_schedule_info: remaining is 0");
                return None;
            }

            plan.awaiting_validation_session_id = None;
            plan.remaining = plan.remaining.saturating_sub(1);

            warn!(
                target: "auto-repeat",
                "mark_validated_and_schedule_info: success, remaining_after_decrement={}",
                plan.remaining
            );

            // Sampled under the plan lock, which `configure_auto_repeat` also
            // holds while it swaps the plan and bumps the generation. Sampling
            // it outside the lock could pair a stale generation with this plan,
            // making the caller abandon a repeat it already consumed.
            let generation = self.auto_repeat_generation.load(Ordering::SeqCst);

            (
                plan.delay_ms,
                plan.config.clone(),
                plan.remaining,
                generation,
            )
        };

        Some((delay_ms, remaining_after_decrement, config, generation))
    }

    pub fn stop(&self) {
        self.cleanup_finished_worker();

        self.configure_auto_repeat(None);

        // Take the flag out from under the lock first, so requesting the stop
        // never runs while the manager's stop mutex is held.
        let signal = recover_lock(&self.stop, "stop").take();
        if let Some(signal) = signal {
            signal.request();
        }
        recover_lock(&self.recent_results, "recent_results").clear();

        // Take the handle out from under the lock, then join: a concurrent
        // `stop`/`start_with_emitter` must not queue behind this join. The
        // stop request above has already woken the worker out of any delay.
        let handle = recover_lock(&self.worker, "worker").take();
        if let Some(handle) = handle {
            let _ = handle.join();
        }

        let mut state = recover_lock(&self.state, "state");
        *state = SessionState::Idle;
    }
}

// Owned `Arc` handles are idiomatic shared ownership here (not `&Arc`,
// which would be a style regression for zero measurable gain); each runs
// once per session. Millisecond values below are exactly representable
// in f64 (`From` has no u64 impl).
#[allow(clippy::needless_pass_by_value)]
fn run_session_loop<E: SessionEmitter + Send + 'static>(
    emitter: E,
    config: SessionConfig,
    state: Arc<Mutex<SessionState>>,
    stop: Arc<StopSignal>,
    session_id: u64,
    recent_results: Arc<Mutex<VecDeque<SessionComplete>>>,
    auto_repeat_plan: Arc<Mutex<Option<AutoRepeatPlan>>>,
) {
    // Convert SessionConfig to SessionConfigEffective for plan generation,
    // through the same conversion normalization reported to the UI.
    let config_effective = effective_from(&config);

    // Generate deterministic session plan.
    let plan = build_session_plan(session_id, &config, config_effective, None);

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

/// Shared stop signal for one session.
///
/// The flag is the single source of truth; the condvar only carries the
/// wake-up, so a worker waiting out a flash delay can be interrupted at once
/// instead of polling a clock. The wake-up cannot be lost: `request` sets the
/// flag while holding the same mutex `wait_until` holds, so a request either
/// lands before the worker parks (and is seen by its next check) or blocks
/// until the worker is parked and can be notified. An early wake-up simply
/// re-checks the flag and the deadline.
#[derive(Debug, Default)]
pub(crate) struct StopSignal {
    requested: AtomicBool,
    mutex: Mutex<()>,
    wake: Condvar,
}

impl StopSignal {
    fn new() -> Self {
        Self::default()
    }

    /// Ask the session to stop and wake it if it is waiting.
    ///
    /// Takes the mutex so the flag store and the notification cannot interleave
    /// with a waiter that has checked the flag but not yet parked.
    fn request(&self) {
        let _guard = recover_lock(&self.mutex, "stop-signal");
        self.requested.store(true, Ordering::SeqCst);
        self.wake.notify_all();
    }

    fn is_requested(&self) -> bool {
        self.requested.load(Ordering::SeqCst)
    }

    /// Wait until `deadline` or until a stop is requested.
    ///
    /// Returns `true` when a stop was requested. Costs nothing while waiting:
    /// the thread sleeps in the kernel until the deadline or the wake-up.
    fn wait_until(&self, deadline: Instant) -> bool {
        let mut guard = recover_lock(&self.mutex, "stop-signal");
        loop {
            if self.is_requested() {
                return true;
            }

            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return false;
            }

            let (next, outcome) = self
                .wake
                .wait_timeout(guard, remaining)
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            guard = next;
            if outcome.timed_out() {
                return self.is_requested();
            }
        }
    }
}

/// Execute a deterministic session plan produced by the core.
/// This function converts the immutable plan steps into runtime events and handles scheduling.
// Long by design: one match arm per step kind keeps the executor auditable
// against the plan schema in a single reading.
#[allow(clippy::too_many_lines)]
// Owned `Arc`/`SessionPlan` handles: called once per session or test; the
// `&Arc` alternative would be a style regression for zero measurable gain.
#[allow(clippy::needless_pass_by_value)]
fn run_session_plan<E: SessionEmitter>(
    emitter: &E,
    plan: SessionPlan,
    state: Arc<Mutex<SessionState>>,
    stop: Arc<StopSignal>,
    recent_results: Arc<Mutex<VecDeque<SessionComplete>>>,
    auto_repeat_plan: Arc<Mutex<Option<AutoRepeatPlan>>>,
    beep: impl Fn(),
) {
    // Iterate through steps and execute them with relative delays
    for step in &plan.steps {
        // Check stop signal before processing each step
        if stop.is_requested() {
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
                stop.wait_until(Instant::now() + delay);
            }

            SessionStep::ShowNumber {
                session_id,
                index,
                total,
                value,
                running_sum,
                delay_ms_before_next,
            } => {
                emitter.show_number(ShowNumber {
                    session_id: *session_id,
                    index: *index,
                    total: *total,
                    value: *value,
                    running_sum: *running_sum,
                    emitted_at_ms: now_epoch_ms(),
                });
                beep();

                // Every flash uses the same configured exposure. The
                // pre-first-flash settle lives in the plan's final
                // countdown delay, not here.
                let delay = Duration::from_millis(*delay_ms_before_next);
                stop.wait_until(Instant::now() + delay);

                let mut st = recover_lock(&*state, "state");
                *st = SessionState::ShowingNumbers;
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
                    stop.wait_until(Instant::now() + delay);
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
                    // Keep the newest `MAX_RECENT_RESULTS`; drop from the
                    // front. (`VecDeque::truncate` would drop from the back,
                    // i.e. the entry just pushed.)
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
                            target: "auto-repeat",
                            "run_session_plan Complete: setting awaiting_validation_session_id={}, remaining_before={}",
                            *session_id, ar_plan.remaining
                        );
                        ar_plan.awaiting_validation_session_id = Some(*session_id);
                    } else {
                        warn!(
                            target: "auto-repeat",
                            "run_session_plan Complete: NOT setting — plan={:?}",
                            plan_guard
                                .as_ref()
                                .map(|p| (p.remaining, p.awaiting_validation_session_id))
                        );
                    }
                }
            }
        }

        // Check for stop signal after each step
        if stop.is_requested() {
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
    use crate::core::types::{SessionConfig, SessionConfigEffective, SessionConfigInput};
    use crate::core::validate::normalize_session_config;
    use std::sync::Arc;

    /// A fresh, un-requested stop signal.
    fn stop_signal() -> Arc<StopSignal> {
        Arc::new(StopSignal::new())
    }

    /// A stop signal that is already requested.
    fn requested_stop_signal() -> Arc<StopSignal> {
        let signal = stop_signal();
        signal.request();
        signal
    }

    // Exactness lock below asserts bit-exact rounding: no epsilon.
    #[allow(clippy::float_cmp)]
    #[test]
    fn normalize_session_config_clamps_and_rounds() {
        let input = SessionConfigInput {
            digits_per_number: 0,
            number_duration_s: 0.049, // below min
            total_numbers: -5,
            allow_negative_numbers: true,
        };

        let (cfg, eff) = normalize_session_config(&input);
        assert_eq!(cfg.digits_per_number, 1);
        assert_eq!(cfg.total_numbers, 1);
        assert!(eff.allow_negative_numbers);
        // Below the 0.1s floor: the exposure snaps up to the shortest allowed
        // step and the reported duration is exactly that step.
        assert_eq!(cfg.number_duration_ms, 100);
        assert_eq!(eff.number_duration_s, 0.1);
    }

    #[test]
    fn session_manager_auto_repeat_and_mark_validated_flow() {
        let manager = SessionManager::default();

        // prepare a minimal valid SessionConfig
        let (config, _eff) = normalize_session_config(&SessionConfigInput {
            digits_per_number: 1,
            number_duration_s: 0.1,
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
        let Some((delay_ms, remaining_after, cfg, generation)) =
            manager.mark_validated_and_schedule_info(42)
        else {
            panic!("expected scheduling info for session 42");
        };
        assert_eq!(delay_ms, 1500);
        assert_eq!(remaining_after, 2);
        assert_eq!(cfg.digits_per_number, config.digits_per_number);
        assert_eq!(generation, manager.auto_repeat_generation());

        // subsequent call for same id should return None (awaiting_validation_session_id cleared)
        assert!(manager.mark_validated_and_schedule_info(42).is_none());
    }

    #[test]
    fn mark_validated_none_conditions() {
        let manager = SessionManager::default();
        let (config, _eff) = normalize_session_config(&SessionConfigInput {
            digits_per_number: 1,
            number_duration_s: 0.1,
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
        assert!(manager.mark_validated_and_schedule_info(123).is_none());

        // plan with remaining == 0 -> None
        let plan2 = AutoRepeatPlan {
            remaining: 0,
            delay_ms: 1000,
            config,
            awaiting_validation_session_id: Some(123),
        };
        manager.configure_auto_repeat(Some(plan2));
        assert!(manager.mark_validated_and_schedule_info(123).is_none());
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
            let mut guard = recover_lock(&manager.recent_results, "recent_results");
            guard.push_back(sc);
        }

        let got = manager.result_for(99).expect("should find result");
        assert_eq!(got.sum, 6);

        // configure auto-repeat and a worker thread to ensure stop() clears them and joins
        let (config, _eff) = normalize_session_config(&SessionConfigInput {
            digits_per_number: 1,
            number_duration_s: 0.1,
            total_numbers: 1,
            allow_negative_numbers: false,
        });

        manager.configure_auto_repeat(Some(AutoRepeatPlan {
            remaining: 1,
            delay_ms: 10,
            config,
            awaiting_validation_session_id: None,
        }));

        // set a stop flag and spawn a short-lived thread as worker
        let stop_flag = stop_signal();
        *recover_lock(&manager.stop, "stop") = Some(stop_flag);

        let handle = std::thread::spawn(move || {
            // do a brief sleep to simulate work
            std::thread::sleep(std::time::Duration::from_millis(5));
        });

        *recover_lock(&manager.worker, "worker") = Some(handle);

        manager.stop();

        // recent_results should be cleared
        let guard = recover_lock(&manager.recent_results, "recent_results");
        assert!(guard.is_empty());

        // auto_repeat_plan cleared
        let plan_guard = recover_lock(&manager.auto_repeat_plan, "auto_repeat_plan");
        assert!(plan_guard.is_none());

        // state should be Idle
        let state_guard = recover_lock(&manager.state, "state");
        assert!(
            matches!(*state_guard, SessionState::Idle),
            "expected Idle state"
        );
    }

    #[test]
    fn wait_until_returns_immediately_when_already_requested() {
        let stop = requested_stop_signal();
        let start = Instant::now();
        assert!(stop.wait_until(Instant::now() + Duration::from_secs(30)));
        assert!(start.elapsed() < Duration::from_millis(50));
    }

    #[test]
    fn wait_until_is_interrupted_by_a_concurrent_request() {
        // A worker waiting out a long delay must be woken by the request, not
        // left to poll: 30s of waiting is cut short here.
        let stop = stop_signal();
        let signaller = Arc::clone(&stop);
        let handle = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(20));
            signaller.request();
        });

        let start = Instant::now();
        let interrupted = stop.wait_until(Instant::now() + Duration::from_secs(30));
        let elapsed = start.elapsed();
        handle.join().expect("signaller thread joins");

        assert!(interrupted, "wait must report the stop request");
        assert!(
            elapsed < Duration::from_secs(5),
            "was not woken: {elapsed:?}"
        );
    }

    #[test]
    fn validate_config_rejects_bad_values() {
        let bad = SessionConfig {
            digits_per_number: 0,
            number_duration_ms: 0,
            total_numbers: 0,
            allow_negative_numbers: false,
        };
        assert!(validate_config(&bad).is_err());

        let too_many_digits = SessionConfig {
            digits_per_number: 19,
            number_duration_ms: 1000,
            total_numbers: 1,
            allow_negative_numbers: false,
        };
        assert!(validate_config(&too_many_digits).is_err());

        let too_many_total = SessionConfig {
            digits_per_number: 2,
            number_duration_ms: 1000,
            total_numbers: 20_000,
            allow_negative_numbers: false,
        };
        assert!(validate_config(&too_many_total).is_err());

        let too_long = SessionConfig {
            digits_per_number: 2,
            number_duration_ms: 120_000,
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
            total_numbers: 100_000,
            allow_negative_numbers: false,
        };

        let (cfg, eff) = normalize_session_config(&input);
        assert!(cfg.digits_per_number <= 15);
        assert!(cfg.number_duration_ms <= 60_000);
        assert!(cfg.total_numbers <= 10_000);
        assert!(eff.number_duration_s <= 60.0);
    }

    #[test]
    fn wait_until_blocks_until_the_deadline_when_not_requested() {
        let stop = stop_signal();
        let start = Instant::now();
        let interrupted = stop.wait_until(Instant::now() + Duration::from_millis(30));
        let elapsed = start.elapsed();
        assert!(!interrupted, "no stop was requested");
        assert!(
            elapsed >= Duration::from_millis(25),
            "returned early: {elapsed:?}"
        );
    }

    #[test]
    fn stop_when_idle_is_safe() {
        let manager = SessionManager::default();
        // stop() on an idle manager should not panic or leave state inconsistent
        manager.stop();
        let state_guard = recover_lock(&manager.state, "state");
        assert!(matches!(*state_guard, SessionState::Idle));
    }

    #[test]
    /**
     * The auto-repeat generation guard.
     *
     * `configure_auto_repeat(None)` bumps the generation, and a stale auto-repeat
     * thread must abandon its restart rather than start a session the user
     * cancelled or superseded. Before this was extracted, the only coverage was
     * a test asserting the counter *increments* — which still passes if every
     * generation check is deleted.
     */
    fn auto_repeat_wait_proceeds_when_generation_is_unchanged() {
        // Remaining time counts down to 0, so the loop ticks 3, 2, 1 and stops.
        let mut remaining = std::collections::VecDeque::from(vec![3000, 2000, 1000, 0]);
        let mut ticks: Vec<u64> = Vec::new();
        let may_start = drive_auto_repeat_wait(
            7,
            &mut || remaining.pop_front().unwrap_or(0),
            &mut || 7,
            |seconds_left| ticks.push(seconds_left),
            || {},
        );

        // One tick per whole second, then the final "0 seconds" tick.
        assert_eq!(ticks, vec![3, 2, 1, 0]);
        assert!(may_start);
    }

    /**
     * Ticks must be emitted *while* the wait is running.
     *
     * Collected-and-replayed — the shape this replaced — delivers the whole
     * countdown in one burst once the wait returns, so the bar sits frozen on
     * its first value until the session restarts. Pinning the interleaving is
     * what keeps emission inside the wait: every whole-second tick must land
     * before the following wait step, and the closing "0 seconds" tick after
     * the last one.
     */
    #[test]
    fn auto_repeat_wait_emits_each_tick_before_the_next_step() {
        let mut remaining = std::collections::VecDeque::from(vec![3000, 2000, 1000, 0]);
        // `RefCell` because the tick sink and the wait step both need to write
        // to the same log, and they are handed to the wait at the same time.
        let log: std::cell::RefCell<Vec<String>> = std::cell::RefCell::new(Vec::new());
        let may_start = drive_auto_repeat_wait(
            7,
            &mut || remaining.pop_front().unwrap_or(0),
            &mut || 7,
            |seconds_left| log.borrow_mut().push(format!("tick {seconds_left}")),
            || log.borrow_mut().push("step".to_string()),
        );

        assert_eq!(
            *log.borrow(),
            vec![
                "tick 3", "step", "tick 2", "step", "tick 1", "step", "tick 0"
            ]
        );
        assert!(may_start);
    }

    /**
     * Drives the wait while bumping the auto-repeat generation on a chosen
     * generation-check call, so each of the guard's three checks can be exercised
     * individually and deterministically — no threads, no real waiting.
     *
     * The deque below holds four values, so the loop runs four iterations and
     * the generation probe is called:
     *
     * | calls  | check                              |
     * |--------|------------------------------------|
     * | 1 to 4 | once per poll iteration            |
     * | 5      | once the delay has elapsed         |
     * | 6      | immediately before the restart     |
     *
     * Call 4 is still a *poll* check: the fourth iteration is the one that reads
     * the trailing `0` and breaks. Numbering the checks by probe call rather
     * than by position is what stops a test aimed at the post-delay guard from
     * quietly landing on the poll guard instead — which it would, silently,
     * with the same ticks and the same answer.
     */
    fn wait_with_generation_bump_on(bump_on: u32) -> (Vec<u64>, bool) {
        let mut remaining = std::collections::VecDeque::from(vec![3000, 2000, 1000, 0]);
        let mut calls = 0u32;
        let mut generation = 7u64;
        let mut ticks: Vec<u64> = Vec::new();
        let may_start = crate::session::drive_auto_repeat_wait(
            7,
            &mut || remaining.pop_front().unwrap_or(0),
            &mut || {
                calls += 1;
                if calls == bump_on {
                    generation += 1;
                }
                generation
            },
            |seconds_left| ticks.push(seconds_left),
            || {},
        );
        (ticks, may_start)
    }

    #[test]
    fn auto_repeat_wait_abandons_when_generation_bumps_before_the_delay() {
        // Call 2 is the poll check on the second iteration: the delay has not
        // elapsed, so the restart is abandoned and no further ticks are produced.
        let (ticks, may_start) = wait_with_generation_bump_on(2);
        assert_eq!(ticks, vec![3]);
        assert!(!may_start);
    }

    #[test]
    fn auto_repeat_wait_abandons_when_generation_bumps_on_a_later_poll() {
        // Call 4 is the poll check on the fourth iteration — the one that reads
        // the trailing `0`. Every whole-second tick has been emitted, but the
        // restart must still be abandoned.
        let (ticks, may_start) = wait_with_generation_bump_on(4);
        assert_eq!(ticks, vec![3, 2, 1]);
        assert!(!may_start);
    }

    #[test]
    fn auto_repeat_wait_abandons_when_generation_bumps_after_the_delay() {
        // Call 5 is the check once the delay has elapsed. A cancel landing here
        // has already counted all the way down, so the "0 seconds" tick must
        // not reach the UI: the wait is over, but the restart is not permitted.
        // Dropping this guard would emit that tick anyway before the final
        // check caught the cancel, which is the difference between a countdown
        // that stops and one that lies about reaching zero.
        let (ticks, may_start) = wait_with_generation_bump_on(5);
        assert_eq!(ticks, vec![3, 2, 1]);
        assert!(!may_start);
    }

    #[test]
    fn auto_repeat_wait_abandons_at_the_final_check_before_start() {
        // Call 6 is the check immediately before the restart — the one that
        // closes the window between "delay finished" and "start() called".
        let (ticks, may_start) = wait_with_generation_bump_on(6);
        assert_eq!(ticks, vec![3, 2, 1, 0]);
        assert!(!may_start);
    }

    #[test]
    fn auto_repeat_wait_dedups_ticks_within_one_second() {
        // A poll interval finer than a second must not emit several ticks for the
        // same displayed second.
        let mut remaining = std::collections::VecDeque::from(
            (0..=3000u64).rev().step_by(100).collect::<Vec<u64>>(),
        );
        let mut ticks: Vec<u64> = Vec::new();
        let may_start = drive_auto_repeat_wait(
            7,
            &mut || remaining.pop_front().unwrap_or(0),
            &mut || 7,
            |seconds_left| ticks.push(seconds_left),
            || {},
        );

        assert_eq!(ticks, vec![3, 2, 1, 0]);
        assert!(may_start);
    }

    #[test]
    fn seconds_left_for_rounds_up() {
        // A countdown must never read 0 before the delay has actually elapsed.
        assert_eq!(seconds_left_for(Duration::from_millis(1)), 1);
        assert_eq!(seconds_left_for(Duration::from_millis(999)), 1);
        assert_eq!(seconds_left_for(Duration::from_millis(1000)), 1);
        assert_eq!(seconds_left_for(Duration::from_millis(1001)), 2);
        assert_eq!(seconds_left_for(Duration::from_millis(3000)), 3);
        assert_eq!(seconds_left_for(Duration::from_millis(0)), 0);
    }

    /// `Duration::as_millis` is `u128`, and pedantic clippy rightly rejects a
    /// blind cast to the `u64` the auto-repeat countdown works in.
    #[test]
    fn millis_u64_is_exact_up_to_u64_max() {
        assert_eq!(millis_u64(Duration::ZERO), 0);
        assert_eq!(millis_u64(Duration::from_millis(1)), 1);
        assert_eq!(millis_u64(Duration::from_secs(30)), 30_000);
        assert_eq!(
            millis_u64(Duration::from_secs(u64::from(u32::MAX))),
            4_294_967_295_000
        );
        // The largest duration whose millisecond count still fits.
        assert_eq!(millis_u64(Duration::from_millis(u64::MAX)), u64::MAX);
    }

    /// The saturating branch is the reason this helper exists rather than a
    /// cast. An absurd duration must read as "effectively forever", not wrap
    /// round to a small number that would look like the wait is nearly over.
    #[test]
    fn millis_u64_saturates_instead_of_truncating() {
        // `u64::MAX` seconds is a millisecond count around 1000x larger than
        // `u64::MAX`, which is exactly the case a truncating cast gets wrong.
        let absurd = Duration::from_secs(u64::MAX);
        assert!(u128::from(u64::MAX) < absurd.as_millis());
        assert_eq!(millis_u64(absurd), u64::MAX);
        // And the countdown derived from it stays huge rather than wrapping.
        assert_eq!(seconds_left_for(absurd), u64::MAX.div_ceil(1000));
    }

    #[test]
    fn stop_twice_is_idempotent_and_leaves_a_startable_manager() {
        let manager = SessionManager::default();
        manager.stop();
        manager.stop();
        assert!(matches!(
            *recover_lock(&manager.state, "state"),
            SessionState::Idle
        ));

        // A manager that has been stopped twice must still accept a session.
        let (config, _eff) = normalize_session_config(&SessionConfigInput {
            digits_per_number: 1,
            number_duration_s: 0.1,
            total_numbers: 1,
            allow_negative_numbers: false,
        });
        let start = manager.start_with_emitter(TestEmitter::new(), config);
        assert!(start.is_ok(), "start after a double stop should still work");
        manager.stop();
    }

    /**
     * The eviction branch never executes in any existing test. It is the only
     * non-trivial logic in that block, and its own comment warns the obvious
     * `VecDeque::truncate` would drop the entry just pushed — breaking
     * `result_for` for the session the user has just finished.
     */
    #[test]
    fn recent_results_evicts_the_oldest_not_the_newest() {
        /// Blocks the test on each completion so the deque is seen settled.
        struct BarrierEmitter {
            done: std::sync::mpsc::Sender<u64>,
        }

        impl SessionEmitter for BarrierEmitter {
            fn clear_screen(&self, _payload: ClearScreen) {}
            fn countdown_tick(&self, _value: String) {}
            fn show_number(&self, _payload: ShowNumber) {}
            fn session_complete(&self, payload: SessionComplete) {
                let _ = self.done.send(payload.session_id);
            }
        }

        let manager = SessionManager::default();
        let (config, _eff) = normalize_session_config(&SessionConfigInput {
            digits_per_number: 1,
            number_duration_s: 0.1,
            total_numbers: 1,
            allow_negative_numbers: false,
        });

        // One more than the deque holds, so the eviction loop runs.
        let total = SessionManager::MAX_RECENT_RESULTS + 3;
        let (tx, rx) = std::sync::mpsc::channel();
        for _ in 0..total {
            // `session_complete` is emitted from *inside* the worker, before the
            // worker returns, so a start issued the instant the completion event
            // arrives can race `start_with_emitter`'s "is the previous worker
            // still alive?" guard and be rejected with "session already running".
            // Wait for the worker to exit before starting the next session —
            // production gets that for free, because a new start follows a user
            // action or the 5s+ auto-repeat delay. This test used to fail
            // intermittently here.
            let mut started = false;
            for _ in 0..100_000 {
                match manager
                    .start_with_emitter(BarrierEmitter { done: tx.clone() }, config.clone())
                {
                    Ok(_) => {
                        started = true;
                        break;
                    }
                    Err(e) if e == "session already running" => {
                        std::thread::sleep(std::time::Duration::from_micros(100));
                    }
                    Err(e) => panic!("start: {e}"),
                }
            }
            assert!(started, "session never became startable again");

            rx.recv_timeout(std::time::Duration::from_secs(10))
                .expect("session should have completed");
        }

        let survivors: Vec<u64> = (1..=total as u64)
            .filter(|id| manager.result_for(*id).is_ok())
            .collect();

        assert_eq!(
            survivors.len(),
            SessionManager::MAX_RECENT_RESULTS,
            "the deque must hold exactly MAX_RECENT_RESULTS"
        );
        assert_eq!(
            *survivors.first().expect("oldest survivor"),
            total as u64 - SessionManager::MAX_RECENT_RESULTS as u64 + 1,
            "the survivors should be the newest window, not the first entries"
        );
        assert_eq!(
            *survivors.last().expect("newest"),
            total as u64,
            "the newest result must survive eviction"
        );
    }
    #[test]
    fn configure_auto_repeat_resets_generation_multiple_times() {
        let manager = SessionManager::default();
        let (config, _eff) = normalize_session_config(&SessionConfigInput {
            digits_per_number: 1,
            number_duration_s: 0.1,
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
                "Generation should increase after each configure_auto_repeat call (iteration {i})"
            );
        }

        manager.configure_auto_repeat(None);
        let gen_after_clear = manager.auto_repeat_generation();
        let plan_guard = recover_lock(&manager.auto_repeat_plan, "auto_repeat_plan");
        assert!(plan_guard.is_none());
        assert!(gen_after_clear > base_gen);
    }

    #[test]
    fn mark_validated_with_wrong_session_id_returns_none() {
        let manager = SessionManager::default();
        let (config, _eff) = normalize_session_config(&SessionConfigInput {
            digits_per_number: 1,
            number_duration_s: 0.1,
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
        assert!(manager.mark_validated_and_schedule_info(99).is_none());

        // None awaiting_validation_session_id also returns None
        let plan2 = AutoRepeatPlan {
            remaining: 1,
            delay_ms: 500,
            config: SessionConfig {
                digits_per_number: 1,
                number_duration_ms: 100,
                total_numbers: 1,
                allow_negative_numbers: false,
            },
            awaiting_validation_session_id: None,
        };
        manager.configure_auto_repeat(Some(plan2));
        assert!(manager.mark_validated_and_schedule_info(42).is_none());
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

        /// A second handle to the same log, for handing to another thread.
        fn share(&self) -> Self {
            Self {
                calls: Arc::clone(&self.calls),
            }
        }
    }

    impl SessionEmitter for TestEmitter {
        fn clear_screen(&self, _payload: ClearScreen) {
            recover_lock(&self.calls, "calls").push("clear_screen".into());
        }
        fn countdown_tick(&self, value: String) {
            recover_lock(&self.calls, "calls").push(format!("countdown({value})"));
        }
        fn show_number(&self, payload: ShowNumber) {
            recover_lock(&self.calls, "calls").push(format!("show_number({})", payload.value));
        }
        fn session_complete(&self, _payload: SessionComplete) {
            recover_lock(&self.calls, "calls").push("session_complete".into());
        }
    }

    /// Records each emission with its `emitted_at_ms`, and can be told to burn
    /// time on a specific emission.
    ///
    /// The delay on a callback is how a real event loop is slow: the webview is
    /// mid-paint, or the emitter's `show_number` is what the UI reacts to. It is
    /// the only lever a test has over the time between two `wait_until` calls,
    /// because `run_session_plan` owns everything else.
    struct TimingEmitter {
        events: Arc<Mutex<Vec<(String, u64)>>>,
        /// Milliseconds to burn on the Nth emission overall, if set.
        lag_on_call: Option<(usize, u64)>,
    }

    impl TimingEmitter {
        fn new(lag_on_call: Option<(usize, u64)>) -> Self {
            Self {
                events: Arc::new(Mutex::new(Vec::new())),
                lag_on_call,
            }
        }

        fn record(&self, label: String, emitted_at_ms: u64) {
            let position = {
                let mut events = recover_lock(&self.events, "events");
                events.push((label, emitted_at_ms));
                events.len()
            };
            if let Some((target, ms)) = self.lag_on_call
                && position == target
            {
                std::thread::sleep(Duration::from_millis(ms));
            }
        }

        fn at(&self, label: &str) -> u64 {
            recover_lock(&self.events, "events")
                .iter()
                .find(|(l, _)| l == label)
                .map_or_else(|| panic!("{label} was never emitted"), |(_, t)| *t)
        }

        fn labels(&self) -> Vec<String> {
            recover_lock(&self.events, "events")
                .iter()
                .map(|(l, _)| l.clone())
                .collect()
        }
    }

    impl SessionEmitter for TimingEmitter {
        fn clear_screen(&self, payload: ClearScreen) {
            let label = match payload.index {
                Some(i) => format!("clear({i})"),
                None => "clear".to_string(),
            };
            self.record(label, payload.emitted_at_ms);
        }
        fn countdown_tick(&self, value: String) {
            let t = now_epoch_ms();
            self.record(format!("countdown({value})"), t);
        }
        fn show_number(&self, payload: ShowNumber) {
            self.record(format!("show({})", payload.index), payload.emitted_at_ms);
        }
        fn session_complete(&self, _payload: SessionComplete) {
            self.record("complete".to_string(), now_epoch_ms());
        }
    }

    /// A two-number plan whose delays are non-zero, so the executor's waiting is
    /// observable.
    fn timed_plan(session_id: u64, exposure_ms: u64, gap_ms: u64) -> SessionPlan {
        SessionPlan {
            session_id,
            config_snapshot: SessionConfigEffective {
                digits_per_number: 1,
                number_duration_s: 0.1,
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
                    delay_ms_before_next: gap_ms,
                },
                SessionStep::CountdownTick {
                    value: "2".into(),
                    delay_ms_before_next: gap_ms,
                },
                SessionStep::CountdownTick {
                    value: "1".into(),
                    delay_ms_before_next: gap_ms + exposure_ms / 4,
                },
                SessionStep::ShowNumber {
                    session_id,
                    index: 1,
                    total: 2,
                    value: 5,
                    running_sum: 5,
                    delay_ms_before_next: exposure_ms,
                },
                SessionStep::ClearScreen {
                    session_id,
                    index: Some(1),
                    delay_ms_before_next: gap_ms,
                },
                SessionStep::ShowNumber {
                    session_id,
                    index: 2,
                    total: 2,
                    value: 3,
                    running_sum: 8,
                    delay_ms_before_next: exposure_ms,
                },
                SessionStep::ClearScreen {
                    session_id,
                    index: Some(2),
                    delay_ms_before_next: gap_ms,
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

    /// Wall-clock floor for `timed_plan`: the sum of every delay the executor
    /// waits on.
    ///
    /// Countdown contributes `gap` twice plus `gap + exposure/4` on the final
    /// tick (which carries the pre-flash settle), and each number contributes
    /// `exposure` then `gap`. The leading and trailing global clears and the
    /// completion carry no delay.
    fn timed_plan_delay_floor(exposure_ms: u64, gap_ms: u64) -> Duration {
        let countdown = gap_ms * 3 + exposure_ms / 4;
        let numbers = 2 * (exposure_ms + gap_ms);
        Duration::from_millis(countdown + numbers)
    }

    /// Runs `timed_plan` and returns the elapsed time alongside the emitter.
    fn run_timed_plan(
        exposure_ms: u64,
        gap_ms: u64,
        lag: Option<(usize, u64)>,
    ) -> (Duration, TimingEmitter) {
        let emitter = TimingEmitter::new(lag);
        let state = Arc::new(Mutex::new(SessionState::Idle));
        let stop = stop_signal();
        let recent_results = Arc::new(Mutex::new(VecDeque::new()));
        let auto_repeat_plan = Arc::new(Mutex::new(None));

        let started = Instant::now();
        run_session_plan(
            &emitter,
            timed_plan(7, exposure_ms, gap_ms),
            state,
            stop,
            recent_results,
            auto_repeat_plan,
            || {},
        );
        (started.elapsed(), emitter)
    }

    /// The executor must actually wait each step's delay.
    ///
    /// Every other `run_session_plan` test builds its plan with
    /// `delay_ms_before_next: 0`, so nothing else here would notice if the
    /// waiting were dropped entirely and a session flashed as fast as the event
    /// loop allowed.
    #[test]
    fn run_session_plan_waits_each_step_delay() {
        let (exposure, gap) = (40u64, 20u64);
        let (elapsed, _emitter) = run_timed_plan(exposure, gap, None);

        assert!(
            elapsed >= timed_plan_delay_floor(exposure, gap),
            "the plan's delays sum to at least {:?} but the session finished in {elapsed:?}",
            timed_plan_delay_floor(exposure, gap),
        );
    }

    /// Lateness must be added to the schedule, never subtracted from it.
    ///
    /// This is invariant I1 in `core/timing.rs` and the desktop counterpart of
    /// the first-flash-exposure bug the browser suite caught in
    /// `flashExposure.test.tsx`. A slow emitter is how the bug appeared in
    /// production: the previous event's lateness used to be deducted from this
    /// number's window, so a flash came up short.
    ///
    /// The lag is injected on `clear(1)` - the event that *ends* number 1's
    /// window - so that under the historical absolute-grid scheduling it would
    /// eat into number 2's exposure. Injecting it on `show(1)` instead would
    /// not show the symptom: that payload is stamped before the callback runs,
    /// so number 1's own measured exposure stays intact either way.
    ///
    /// The executor re-anchors each deadline to `Instant::now() + delay`, so
    /// burning the time can only push the rest of the session later.
    #[test]
    fn run_session_plan_lateness_does_not_shorten_exposure() {
        let (exposure, gap) = (60u64, 20u64);
        let lag_ms = 150u64;
        // Emission 6 is `clear(1)`: 1 global clear, 3 countdowns, show(1).
        let (elapsed, emitter) = run_timed_plan(exposure, gap, Some((6, lag_ms)));

        let second_shown = emitter.at("show(2)");
        let second_cleared = emitter.at("clear(2)");
        let on_screen = Duration::from_millis(second_cleared.saturating_sub(second_shown));

        assert!(
            on_screen >= Duration::from_millis(exposure),
            "number 2 was on screen for {on_screen:?}, less than its {exposure}ms exposure; \
             the lateness before it was taken out of its window",
        );
        assert!(
            elapsed >= timed_plan_delay_floor(exposure, gap) + Duration::from_millis(lag_ms),
            "the injected {lag_ms}ms lateness should be added to the schedule, \
             not absorbed into it (elapsed {elapsed:?})",
        );
    }

    /// `emitted_at_ms` is what the UI compares against its own `Date.now()`, so
    /// a clock that went backwards would make the UI compute a negative
    /// duration for a number it is showing.
    #[test]
    fn run_session_plan_emitted_at_ms_is_monotonic() {
        let (_elapsed, emitter) = run_timed_plan(20, 10, None);
        let stamps: Vec<u64> = recover_lock(&emitter.events, "events")
            .iter()
            .map(|(_, t)| *t)
            .collect();

        assert!(
            stamps.len() > 1,
            "expected several emissions, got {stamps:?}",
        );
        for pair in stamps.windows(2) {
            assert!(
                pair[0] <= pair[1],
                "emitted_at_ms went backwards: {} then {}",
                pair[0],
                pair[1],
            );
        }
        assert_eq!(
            emitter.labels().last().map(String::as_str),
            Some("complete"),
            "the session should end with the completion",
        );
    }

    /// An emitter that also records the manager's state at each emission.
    ///
    /// Every other emitter here sees only the event stream, so it cannot
    /// distinguish "the session is between events" from "the session is still
    /// running but the executor has not updated the state yet". Reading the state
    /// from inside the callback pins the observation to the exact instant of the
    /// emission, which is the only way to assert the transition happens *when*
    /// the number is shown rather than at some later point the executor happens to
    /// get to.
    ///
    /// It shares the `Arc<Mutex<SessionState>>` that `run_session_plan` writes, so
    /// the observation is of the state the executor is actually driving.
    struct StateObservingEmitter {
        events: Arc<Mutex<Vec<String>>>,
        states_log: Arc<Mutex<Vec<SessionState>>>,
        state: Arc<Mutex<SessionState>>,
    }

    impl StateObservingEmitter {
        fn new(state: Arc<Mutex<SessionState>>) -> Self {
            Self {
                events: Arc::new(Mutex::new(Vec::new())),
                states_log: Arc::new(Mutex::new(Vec::new())),
                state,
            }
        }

        fn observe(&self) {
            let snapshot = recover_lock(&self.state, "state").clone();
            recover_lock(&self.states_log, "states_log").push(snapshot);
        }

        /// Pairs each label with the state observed at that emission.
        fn trace(&self) -> Vec<(String, SessionState)> {
            let events = recover_lock(&self.events, "events");
            let states = recover_lock(&self.states_log, "states_log");
            events.iter().cloned().zip(states.iter().cloned()).collect()
        }
    }

    impl SessionEmitter for StateObservingEmitter {
        fn clear_screen(&self, payload: ClearScreen) {
            self.observe();
            // Index-aware labels, matching `TimingEmitter`, so a test can tell
            // the global clear from the one that ends a specific number's
            // exposure.
            let label = match payload.index {
                Some(i) => format!("clear({i})"),
                None => "clear".to_string(),
            };
            recover_lock(&self.events, "events").push(label);
        }
        fn countdown_tick(&self, value: String) {
            self.observe();
            recover_lock(&self.events, "events").push(format!("countdown({value})"));
        }
        fn show_number(&self, _payload: ShowNumber) {
            self.observe();
            recover_lock(&self.events, "events").push("show_number".into());
        }
        fn session_complete(&self, _payload: SessionComplete) {
            self.observe();
            recover_lock(&self.events, "events").push("session_complete".into());
        }
    }

    /// The session state machine must reach `ShowingNumbers` while a number is on
    /// screen and return to `Complete` at the end.
    ///
    /// `run_session_plan` sets `SessionState::ShowingNumbers` inside its `ShowNumber`
    /// arm (session.rs:525) but never sets it back, and nothing outside the plan
    /// resets it either — so if that arm ever stopped running, the manager would
    /// stay `ShowingNumbers` forever and a second `start` would be refused as a
    /// concurrent session. The only existing assertion of the mid-session state
    /// goes through `SessionManager` (line 2144), which observes it after the
    /// whole session has finished rather than while a number is displayed, so it
    /// cannot catch "the state was right at the end but never during".
    #[test]
    fn run_session_plan_marks_showing_numbers_while_a_number_is_on_screen() {
        let state = Arc::new(Mutex::new(SessionState::Idle));
        let emitter = StateObservingEmitter::new(Arc::clone(&state));

        let stop = stop_signal();
        let recent_results = Arc::new(Mutex::new(VecDeque::new()));
        let auto_repeat_plan = Arc::new(Mutex::new(None));

        run_session_plan(
            &emitter,
            make_sample_plan(17),
            Arc::clone(&state),
            stop,
            recent_results,
            auto_repeat_plan,
            || {},
        );

        let trace = emitter.trace();

        // Pin the *ordering* of the transition relative to the emissions, which
        // is the part that matters: the state is set at the end of the exposure,
        // so the show itself still observes Idle and the clear that ends the
        // exposure observes ShowingNumbers. Asserting only the end state (as the
        // manager test at line 2144 does) cannot distinguish "set before the
        // show" from "set after the wait".
        let show = trace
            .iter()
            .find(|(label, _)| *label == "show_number")
            .unwrap_or_else(|| panic!("expected show_number in {trace:?}"));
        assert!(
            matches!(show.1, SessionState::Idle),
            "the show emission happens before the state is set, got {:?}",
            show.1,
        );

        let cleared = trace
            .iter()
            .find(|(label, _)| *label == "clear(1)")
            .unwrap_or_else(|| panic!("expected clear(1) in {trace:?}"));
        assert!(
            matches!(cleared.1, SessionState::ShowingNumbers),
            "the clear that ends the exposure must observe ShowingNumbers, got {:?}",
            cleared.1,
        );

        // Countdowns run before any number exists, so they must observe Idle.
        for (label, st) in &trace {
            if label.starts_with("countdown(") {
                assert!(
                    matches!(st, SessionState::Idle),
                    "{label} should observe Idle, got {st:?}",
                );
            }
        }

        // The session must end in Complete, and the completion must observe it.
        let final_state = recover_lock(&state, "state");
        assert!(
            matches!(*final_state, SessionState::Complete),
            "the session must end in Complete, got {final_state:?}",
        );
        // The completion emission happens *before* the state is flipped, so it
        // observes ShowingNumbers; only the post-run read sees Complete. That
        // ordering is what keeps a concurrent `start` refused for the whole
        // exposure rather than racing the flip.
        let completed = trace
            .iter()
            .find(|(label, _)| *label == "session_complete")
            .unwrap_or_else(|| panic!("expected session_complete in {trace:?}"));
        assert!(
            matches!(completed.1, SessionState::ShowingNumbers),
            "the completion is emitted before the state flips, so it must observe \
             ShowingNumbers, got {:?}",
            completed.1,
        );
    }

    /// A stop that arrives while the executor is genuinely blocked inside a wait
    /// must wake it, end the session, and leave the state idle.
    ///
    /// Every other stop test requests the stop from a callback that runs *between*
    /// two waits — `run_session_plan_stop_after_first_number` fires the request
    /// inside the beep hook, i.e. before the wait has started. Nothing here
    /// interrupts a call already parked in `wait_until`, so "woken mid-wait means
    /// the executor returns at the next boundary rather than continuing" and
    /// "a blocked session cannot be stuck forever" were both unverified.
    #[test]
    fn run_session_plan_wakes_when_stop_arrives_while_blocked() {
        let emitter = TestEmitter::new();
        let state = Arc::new(Mutex::new(SessionState::Idle));
        let stop = stop_signal();
        let recent_results = Arc::new(Mutex::new(VecDeque::new()));
        let auto_repeat_plan = Arc::new(Mutex::new(None));

        // A delay long enough that the test can request the stop while the
        // executor is parked. Only the lower bound on the wait is asserted, so a
        // slow machine cannot fail this.
        let plan = SessionPlan {
            session_id: 99,
            config_snapshot: SessionConfigEffective {
                digits_per_number: 1,
                number_duration_s: 0.1,
                total_numbers: 1,
                allow_negative_numbers: false,
            },
            steps: vec![
                SessionStep::ClearScreen {
                    session_id: 99,
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
                    session_id: 99,
                    index: 1,
                    total: 1,
                    value: 5,
                    running_sum: 5,
                    delay_ms_before_next: 5000,
                },
                SessionStep::ClearScreen {
                    session_id: 99,
                    index: Some(1),
                    delay_ms_before_next: 0,
                },
                SessionStep::Complete {
                    session_id: 99,
                    numbers: vec![5],
                    sum: 5,
                },
            ],
            total_duration_ms: 5000,
            numbers_generated: vec![5],
            expected_sum: 5,
        };

        // A channel rather than `JoinHandle::is_finished`, which is not stable on
        // every toolchain: the test waits for the executor to signal completion,
        // with a timeout so a hang fails the test instead of the suite.
        let (done_tx, done_rx) = std::sync::mpsc::channel::<()>();
        let stop_handle = Arc::clone(&stop);
        let thread_emitter = emitter.share();
        let state_for_thread = Arc::clone(&state);
        let joiner = std::thread::Builder::new()
            .name("blocked-stop".into())
            .spawn(move || {
                run_session_plan(
                    &thread_emitter,
                    plan,
                    state_for_thread,
                    stop,
                    recent_results,
                    auto_repeat_plan,
                    || {},
                );
                let _ = done_tx.send(());
            })
            .expect("spawn the executor thread");

        // Give the executor a moment to reach the 5s wait, then request the stop.
        std::thread::sleep(Duration::from_millis(50));
        stop_handle.request();

        done_rx
            .recv_timeout(Duration::from_secs(5))
            .expect("the executor must return within 5s of a stop request while blocked");
        joiner.join().expect("the executor thread panicked");

        // The executor must have returned to Idle, not stayed parked or been left
        // in ShowingNumbers by a session that never finished.
        let final_state = recover_lock(&state, "state");
        assert!(
            matches!(*final_state, SessionState::Idle),
            "a stop while blocked must leave the state Idle, got {final_state:?}",
        );

        // And it must have cleared the screen rather than leaving the last number
        // on it.
        let calls = recover_lock(&emitter.calls, "calls");
        assert!(
            calls.contains(&"clear_screen".to_string()),
            "a stop must clear the screen before returning",
        );
        // The stop landed during the 5s exposure, so the number was already
        // shown. What must not happen is the session running on to its own
        // completion after the stop was requested.
        assert!(
            !calls.contains(&"session_complete".to_string()),
            "a stopped session must not run to completion, got {calls:?}",
        );
    }

    fn make_sample_plan(session_id: u64) -> SessionPlan {
        SessionPlan {
            session_id,
            config_snapshot: SessionConfigEffective {
                digits_per_number: 1,
                number_duration_s: 0.1,
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
        let stop = stop_signal();
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

        let calls = recover_lock(&emitter.calls, "calls");
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
        let stop = requested_stop_signal();
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

        let calls = recover_lock(&emitter.calls, "calls");
        assert_eq!(*calls, vec!["clear_screen"]);
        let state_guard = recover_lock(&state, "state");
        assert!(matches!(*state_guard, SessionState::Idle));
    }

    #[test]
    fn run_session_plan_stores_results() {
        let emitter = TestEmitter::new();
        let state = Arc::new(Mutex::new(SessionState::Idle));
        let stop = stop_signal();
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
        let state_guard = recover_lock(&state, "state");
        assert!(matches!(*state_guard, SessionState::Complete));

        // Check results stored
        let results = recover_lock(&recent_results, "recent_results");
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].session_id, 42);
        assert_eq!(results[0].sum, 8);
        assert_eq!(results[0].numbers, vec![5, 3]);
    }

    #[test]
    fn run_session_plan_sets_awaiting_validation() {
        let emitter = TestEmitter::new();
        let state = Arc::new(Mutex::new(SessionState::Idle));
        let stop = stop_signal();
        let recent_results = Arc::new(Mutex::new(VecDeque::new()));
        let auto_repeat_plan = Arc::new(Mutex::new(Some(AutoRepeatPlan {
            remaining: 3,
            delay_ms: 500,
            config: SessionConfig {
                digits_per_number: 1,
                number_duration_ms: 100,
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

        let plan_guard = recover_lock(&auto_repeat_plan, "auto_repeat_plan");
        let ar = plan_guard.as_ref().unwrap();
        assert_eq!(ar.awaiting_validation_session_id, Some(42));
        assert_eq!(ar.remaining, 3);
    }

    #[test]
    fn run_session_plan_does_not_set_awaiting_when_remaining_zero() {
        let emitter = TestEmitter::new();
        let state = Arc::new(Mutex::new(SessionState::Idle));
        let stop = stop_signal();
        let recent_results = Arc::new(Mutex::new(VecDeque::new()));
        let auto_repeat_plan = Arc::new(Mutex::new(Some(AutoRepeatPlan {
            remaining: 0,
            delay_ms: 500,
            config: SessionConfig {
                digits_per_number: 1,
                number_duration_ms: 100,
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

        let plan_guard = recover_lock(&auto_repeat_plan, "auto_repeat_plan");
        assert!(
            plan_guard
                .as_ref()
                .unwrap()
                .awaiting_validation_session_id
                .is_none()
        );
    }

    #[test]
    fn result_for_not_found() {
        let manager = SessionManager::default();
        let err = manager.result_for(999).unwrap_err();
        assert_eq!(err, "session result not found");
    }

    // Long by design: an explicit multi-step plan literal keeps the
    // negative-number flow auditable without indirection.
    #[allow(clippy::too_many_lines)]
    #[test]
    fn run_session_plan_with_negative_numbers() {
        let emitter = TestEmitter::new();
        let state = Arc::new(Mutex::new(SessionState::Idle));
        let stop = stop_signal();
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

        let calls = recover_lock(&emitter.calls, "calls");
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

        let state_guard = recover_lock(&state, "state");
        assert!(matches!(*state_guard, SessionState::Complete));

        let results = recover_lock(&recent_results, "recent_results");
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].session_id, 77);
        assert_eq!(results[0].sum, -2);
        assert_eq!(results[0].numbers, vec![-5, 3]);
    }

    #[test]
    fn run_session_plan_with_zero_numbers() {
        let emitter = TestEmitter::new();
        let state = Arc::new(Mutex::new(SessionState::Idle));
        let stop = stop_signal();
        let recent_results = Arc::new(Mutex::new(VecDeque::new()));
        let auto_repeat_plan = Arc::new(Mutex::new(None));

        let plan = SessionPlan {
            session_id: 100,
            config_snapshot: SessionConfigEffective {
                digits_per_number: 1,
                number_duration_s: 0.1,
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

        let calls = recover_lock(&emitter.calls, "calls");
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

        let state_guard = recover_lock(&state, "state");
        assert!(matches!(*state_guard, SessionState::Complete));

        let results = recover_lock(&recent_results, "recent_results");
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].session_id, 100);
        assert_eq!(results[0].sum, 0);
        assert_eq!(results[0].numbers, Vec::<i64>::new());
    }

    #[test]
    fn run_session_plan_with_total_numbers_1() {
        let emitter = TestEmitter::new();
        let state = Arc::new(Mutex::new(SessionState::Idle));
        let stop = stop_signal();
        let recent_results = Arc::new(Mutex::new(VecDeque::new()));
        let auto_repeat_plan = Arc::new(Mutex::new(None));

        let plan = SessionPlan {
            session_id: 50,
            config_snapshot: SessionConfigEffective {
                digits_per_number: 1,
                number_duration_s: 0.1,
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

        let calls = recover_lock(&emitter.calls, "calls");
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

        let state_guard = recover_lock(&state, "state");
        assert!(matches!(*state_guard, SessionState::Complete));

        let results = recover_lock(&recent_results, "recent_results");
        assert_eq!(results[0].session_id, 50);
        assert_eq!(results[0].sum, 42);
        assert_eq!(results[0].numbers, vec![42]);
    }

    #[test]
    fn run_session_plan_stop_after_first_number() {
        let emitter = TestEmitter::new();
        let state = Arc::new(Mutex::new(SessionState::Idle));
        let stop = stop_signal();
        let recent_results = Arc::new(Mutex::new(VecDeque::new()));
        let auto_repeat_plan = Arc::new(Mutex::new(None));
        let beep_count = Arc::new(std::sync::atomic::AtomicU32::new(0));

        let stop_clone = Arc::clone(&stop);
        let bc = Arc::clone(&beep_count);
        let beep = move || {
            bc.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            stop_clone.request();
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

        let calls = recover_lock(&emitter.calls, "calls");
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
        let state_guard = recover_lock(&state, "state");
        assert!(matches!(*state_guard, SessionState::Idle));
    }

    #[test]
    fn run_session_plan_full_auto_repeat_cycle() {
        let emitter = TestEmitter::new();
        let state = Arc::new(Mutex::new(SessionState::Idle));
        let stop = stop_signal();
        let recent_results = Arc::new(Mutex::new(VecDeque::new()));

        let manager = SessionManager::default();
        let (config, _eff) = normalize_session_config(&SessionConfigInput {
            digits_per_number: 1,
            number_duration_s: 0.1,
            total_numbers: 2,
            allow_negative_numbers: false,
        });

        // Step 1: Configure auto-repeat with remaining=1 via SessionManager
        manager.configure_auto_repeat(Some(AutoRepeatPlan {
            remaining: 1,
            delay_ms: 500,
            config,
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
            let plan_guard = recover_lock(&manager.auto_repeat_plan, "auto_repeat_plan");
            let ar = plan_guard.as_ref().unwrap();
            assert_eq!(ar.awaiting_validation_session_id, Some(42));
            assert_eq!(ar.remaining, 1);
        }

        // Step 3: Call mark_validated_and_schedule_info to consume it
        let Some((_delay_ms, remaining, _cfg, _gen)) = manager.mark_validated_and_schedule_info(42)
        else {
            panic!("expected scheduling info for session 42");
        };
        assert_eq!(remaining, 0, "remaining should be 0 after consuming");

        // Subsequent call should return None (awaiting already cleared)
        assert!(manager.mark_validated_and_schedule_info(42).is_none());
    }

    #[test]
    fn start_with_emitter_creates_session() {
        let manager = SessionManager::default();
        let (config, _eff) = normalize_session_config(&SessionConfigInput {
            digits_per_number: 1,
            number_duration_s: 0.1,
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
            let state_guard = recover_lock(&manager.state, "state");
            assert!(matches!(*state_guard, SessionState::ShowingNumbers));
        }

        // Stop and verify Idle state
        manager.stop();
        {
            let state_guard = recover_lock(&manager.state, "state");
            assert!(matches!(*state_guard, SessionState::Idle));
        }
    }

    #[test]
    fn start_with_emitter_rejects_concurrent() {
        let manager = SessionManager::default();
        let (config, _eff) = normalize_session_config(&SessionConfigInput {
            digits_per_number: 1,
            number_duration_s: 0.1,
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
    fn session_manager_stop_after_mark_validated() {
        let manager = SessionManager::default();
        let (config, _eff) = normalize_session_config(&SessionConfigInput {
            digits_per_number: 1,
            number_duration_s: 0.1,
            total_numbers: 1,
            allow_negative_numbers: false,
        });

        // Configure auto-repeat with awaiting validation
        manager.configure_auto_repeat(Some(AutoRepeatPlan {
            remaining: 1,
            delay_ms: 500,
            config,
            awaiting_validation_session_id: Some(42),
        }));

        // Consume the awaiting via mark_validated_and_schedule_info
        assert!(manager.mark_validated_and_schedule_info(42).is_some());

        // Insert a fake result to verify results are preserved until stop
        {
            let mut guard = recover_lock(&manager.recent_results, "recent_results");
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
            let state_guard = recover_lock(&manager.state, "state");
            assert!(matches!(*state_guard, SessionState::Idle));
        }

        // recent_results are cleared by stop()
        {
            let guard = recover_lock(&manager.recent_results, "recent_results");
            assert!(guard.is_empty(), "stop() should clear recent_results");
        }

        // auto_repeat_plan is cleared by stop()
        {
            let plan_guard = recover_lock(&manager.auto_repeat_plan, "auto_repeat_plan");
            assert!(plan_guard.is_none(), "stop() should clear auto_repeat_plan");
        }
    }

    /// `recover_lock` exists precisely so a panicking worker cannot poison every
    /// later lock and take the manager down with it. Nothing exercised that
    /// recovery: every other test locks a healthy mutex, so deleting the
    /// `unwrap_or_else` would leave the whole suite green.
    #[test]
    fn recover_lock_recovers_a_poisoned_mutex() {
        let mutex = Arc::new(Mutex::new(41i32));
        let poisoner = Arc::clone(&mutex);

        // Silence the default hook so the deliberate panic does not spray the
        // output, and restore it before asserting so a genuine failure here is
        // still reported.
        let previous_hook = std::panic::take_hook();
        std::panic::set_hook(Box::new(|_| {}));
        let handle = std::thread::spawn(move || {
            let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                let _guard = poisoner.lock().expect("the first lock is clean");
                panic!("poison the mutex on purpose");
            }));
        });
        handle
            .join()
            .expect("the poisoning thread itself does not panic");
        std::panic::set_hook(previous_hook);

        // The mutex is now poisoned, so a plain `lock` fails...
        assert!(mutex.lock().is_err(), "the mutex should be poisoned");
        // ...but `recover_lock` still hands back the guard, value intact.
        let recovered = recover_lock(&mutex, "test");
        assert_eq!(*recovered, 41i32, "recovery must not lose the value");
    }

    /// The state a fresh manager exposes before any session exists. Every other
    /// test builds on `default()` but none pins what "fresh" means, so a change
    /// (a pre-armed generation, a stale handle) would only surface as a flake.
    #[test]
    fn manager_defaults_to_idle_with_the_first_generation() {
        let manager = SessionManager::default();

        assert!(matches!(
            *recover_lock(&manager.state, "state"),
            SessionState::Idle
        ));
        assert!(recover_lock(&manager.worker, "worker").is_none());
        assert!(recover_lock(&manager.stop, "stop").is_none());
        assert!(recover_lock(&manager.auto_repeat_plan, "auto_repeat_plan").is_none());
        assert!(
            manager.result_for(1).is_err(),
            "no results before any session"
        );
        assert_eq!(
            manager.auto_repeat_generation(),
            1,
            "generations start at 1"
        );
    }

    /// `emitted_at_ms` is compared against the webview's `Date.now()`, so the
    /// unit is a contract: epoch *milliseconds*, not seconds. Pin the magnitude
    /// (which distinguishes the two) and that it never runs backwards.
    #[test]
    fn now_epoch_ms_is_epoch_milliseconds_and_non_decreasing() {
        let first = now_epoch_ms();
        // The window 2020-01-01 .. 2100-01-01, in epoch milliseconds.
        assert!(
            (1_577_836_800_000..4_102_444_800_000).contains(&first),
            "epoch millis are out of range: {first}",
        );
        assert!(
            now_epoch_ms() >= first,
            "wall-clock time must not run backwards",
        );
    }

    /// The beep is the audible tick of each flash. Nothing pinned that it fires
    /// once per *number* and never for a clear or a countdown tick.
    #[test]
    fn run_session_plan_beeps_once_per_flash_and_never_for_a_clear() {
        let emitter = TestEmitter::new();
        let beeps = Arc::new(Mutex::new(0u32));
        let beep_counter = Arc::clone(&beeps);

        // Zero delays make the cadence observable without any real waiting.
        run_session_plan(
            &emitter,
            timed_plan(7, 0, 0),
            Arc::new(Mutex::new(SessionState::Idle)),
            stop_signal(),
            Arc::new(Mutex::new(VecDeque::new())),
            Arc::new(Mutex::new(None)),
            move || {
                *recover_lock(&beep_counter, "beeps") += 1;
            },
        );

        let calls = recover_lock(&emitter.calls, "calls").clone();
        let flashes = calls
            .iter()
            .filter(|call| call.starts_with("show_number"))
            .count();
        assert_eq!(flashes, 2, "timed_plan flashes two numbers");
        assert_eq!(
            *recover_lock(&beeps, "beeps"),
            u32::try_from(flashes).expect("flashes fit u32"),
            "exactly one beep per flash, none for the clears or the countdown",
        );
    }

    /// A repeat whose delay has already elapsed (a slow start, or a wake-up that
    /// lands late) must still report a single "0 seconds" tick and be allowed to
    /// restart, without entering the poll loop.
    #[test]
    fn auto_repeat_wait_with_no_time_left_emits_only_the_final_tick() {
        let mut ticks: Vec<u64> = Vec::new();
        let may_start = drive_auto_repeat_wait(
            7,
            &mut || 0,
            &mut || 7,
            |seconds_left| ticks.push(seconds_left),
            || panic!("no poll step should run once the delay has elapsed"),
        );

        assert_eq!(ticks, vec![0], "an elapsed delay still reports a zero tick");
        assert!(may_start);
    }

    /// The very first generation check must fire *before* any tick or poll, or a
    /// cancelled repeat would flash once before noticing it was cancelled.
    #[test]
    fn auto_repeat_wait_abandons_before_the_first_poll_when_already_stale() {
        let mut ticks: Vec<u64> = Vec::new();
        let may_start = drive_auto_repeat_wait(
            7,
            // A stale generation must abandon before the clock is even read.
            &mut || panic!("the clock must not be read after an early abandon"),
            &mut || 8,
            |seconds_left| ticks.push(seconds_left),
            || panic!("no polling after an early abandon"),
        );

        assert!(ticks.is_empty(), "a cancelled repeat must emit no ticks");
        assert!(!may_start);
    }

    /// The end-to-end wiring through a real worker thread. Every other executor
    /// test calls `run_session_plan` directly, so nothing pins that
    /// `start_with_emitter` connects the manager, the spawned worker and the
    /// emitter — a broken spawn, a dropped result or a wrong terminal state
    /// would leave all of them green.
    #[test]
    fn start_with_emitter_runs_a_real_session_end_to_end() {
        struct ChannelEmitter {
            complete: std::sync::mpsc::Sender<SessionComplete>,
            events: Arc<Mutex<Vec<String>>>,
        }

        impl SessionEmitter for ChannelEmitter {
            fn clear_screen(&self, payload: ClearScreen) {
                let label = match payload.index {
                    Some(index) => format!("clear({index})"),
                    None => "clear".to_string(),
                };
                recover_lock(&self.events, "events").push(label);
            }
            fn countdown_tick(&self, value: String) {
                recover_lock(&self.events, "events").push(format!("countdown({value})"));
            }
            fn show_number(&self, payload: ShowNumber) {
                recover_lock(&self.events, "events").push(format!("show({})", payload.index));
            }
            fn session_complete(&self, payload: SessionComplete) {
                recover_lock(&self.events, "events").push("complete".to_string());
                let _ = self.complete.send(payload);
            }
        }

        let manager = SessionManager::default();
        let (config, _effective) = normalize_session_config(&SessionConfigInput {
            digits_per_number: 1,
            number_duration_s: 0.1,
            total_numbers: 2,
            allow_negative_numbers: false,
        });

        let events = Arc::new(Mutex::new(Vec::new()));
        let (tx, rx) = std::sync::mpsc::channel();
        let session_id = manager
            .start_with_emitter(
                ChannelEmitter {
                    complete: tx,
                    events: Arc::clone(&events),
                },
                config,
            )
            .expect("a valid config must start");

        let complete = rx
            .recv_timeout(Duration::from_secs(10))
            .expect("the session must complete within the timeout");
        assert_eq!(complete.session_id, session_id);
        assert_eq!(complete.numbers.len(), 2);
        assert_eq!(complete.sum, complete.numbers.iter().sum::<i64>());

        // The manager stores exactly the result it announced, under the id it
        // handed back.
        let stored = manager
            .result_for(session_id)
            .expect("the completion must be stored");
        assert_eq!(stored.numbers, complete.numbers);
        assert_eq!(stored.sum, complete.sum);

        // Wait for the worker itself to exit: `session_complete` is emitted from
        // *inside* the worker, before it publishes the terminal `Complete` state.
        let mut finished = false;
        for _ in 0..500 {
            finished = recover_lock(&manager.worker, "worker")
                .as_ref()
                .is_some_and(JoinHandle::is_finished);
            if finished {
                break;
            }
            std::thread::sleep(Duration::from_millis(2));
        }
        assert!(finished, "the worker should exit after completing");
        assert!(matches!(
            *recover_lock(&manager.state, "state"),
            SessionState::Complete
        ));

        // The emitted sequence is exactly the plan: a leading global clear, the
        // countdown, one flash-then-clear per number, a trailing clear and the
        // completion.
        assert_eq!(
            *recover_lock(&events, "events"),
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
    }

    /// The eviction boundary itself: with exactly `MAX_RECENT_RESULTS` sessions
    /// nothing is dropped, and the next session drops only the oldest. The other
    /// eviction test runs *past* the boundary and checks the survivors; this
    /// pins the edge just below it, where an off-by-one that evicted early would
    /// otherwise go unnoticed. Completion-only plans keep it instant.
    #[test]
    fn recent_results_does_not_evict_before_the_boundary() {
        fn completion_only(session_id: u64) -> SessionPlan {
            let numbers = vec![i64::try_from(session_id).expect("session ids fit i64")];
            let sum = numbers.iter().sum();
            SessionPlan {
                session_id,
                config_snapshot: crate::core::types::SessionConfigEffective {
                    digits_per_number: 1,
                    number_duration_s: 0.1,
                    total_numbers: 1,
                    allow_negative_numbers: false,
                },
                steps: vec![SessionStep::Complete {
                    session_id,
                    numbers: numbers.clone(),
                    sum,
                }],
                total_duration_ms: 0,
                numbers_generated: numbers,
                expected_sum: sum,
            }
        }

        let manager = SessionManager::default();
        let emitter = TestEmitter::new();
        let max = SessionManager::MAX_RECENT_RESULTS;
        let max_u64 = u64::try_from(max).expect("the result cap fits u64");

        let run = |session_id: u64| {
            run_session_plan(
                &emitter,
                completion_only(session_id),
                Arc::new(Mutex::new(SessionState::Idle)),
                stop_signal(),
                Arc::clone(&manager.recent_results),
                Arc::new(Mutex::new(None)),
                || {},
            );
        };

        for session_id in 1..=max_u64 {
            run(session_id);
        }

        let stored_ids = |manager: &SessionManager| -> Vec<u64> {
            recover_lock(&manager.recent_results, "recent_results")
                .iter()
                .map(|result| result.session_id)
                .collect()
        };

        assert_eq!(
            stored_ids(&manager),
            (1..=max_u64).collect::<Vec<_>>(),
            "exactly {max} sessions must all be retained",
        );

        run(max_u64 + 1);
        assert_eq!(
            stored_ids(&manager),
            (2..=max_u64 + 1).collect::<Vec<_>>(),
            "the next session must drop only the oldest",
        );
    }
}
