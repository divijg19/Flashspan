use rand::Rng;
use serde::Deserialize;
use std::{
    collections::VecDeque,
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        Arc, Mutex,
    },
    thread,
    thread::JoinHandle,
    time::{Duration, Instant},
};
use tauri::{AppHandle, Emitter};

#[derive(Debug, Clone, serde::Serialize)]
pub struct SessionComplete {
    pub session_id: u64,
    pub numbers: Vec<i64>,
    pub sum: i64,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct ShowNumber {
    pub session_id: u64,
    pub index: u32,
    pub total: u32,
    pub value: i64,
    pub running_sum: i64,
}

#[derive(Debug, Clone)]
pub struct AutoRepeatPlan {
    pub remaining: u32,
    pub delay_ms: u64,
    pub config: SessionConfig,
    pub awaiting_validation_session_id: Option<u64>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct SessionConfigInput {
    pub digits_per_number: i64,
    pub number_duration_s: f64,
    pub delay_between_numbers_s: f64,
    pub total_numbers: i64,

    #[serde(default)]
    pub allow_negative_numbers: bool,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct SessionConfigEffective {
    pub digits_per_number: u32,
    pub number_duration_s: f64,
    pub delay_between_numbers_s: f64,
    pub total_numbers: u32,
    pub allow_negative_numbers: bool,
}

#[derive(Debug, Clone)]
pub struct SessionConfig {
    pub digits_per_number: u32,
    pub number_duration_ms: u64,
    pub delay_between_numbers_ms: u64,
    pub total_numbers: u32,
    pub allow_negative_numbers: bool,
}

fn round_1_decimal(v: f64) -> f64 {
    (v * 10.0).round() / 10.0
}

fn clamp_f64(v: f64, min: f64, max: f64) -> f64 {
    if !v.is_finite() {
        return min;
    }
    v.max(min).min(max)
}

fn clamp_i64(v: i64, min: i64, max: i64) -> i64 {
    v.max(min).min(max)
}

fn seconds_to_ms_clamped(seconds: f64, min_ms: u64, max_ms: u64) -> u64 {
    let ms = (seconds * 1000.0).round();
    if !ms.is_finite() {
        return min_ms;
    }
    let ms_u64 = if ms <= 0.0 { 0 } else { ms as u64 };
    ms_u64.max(min_ms).min(max_ms)
}

pub fn normalize_session_config(
    input: SessionConfigInput,
) -> (SessionConfig, SessionConfigEffective) {
    let digits = clamp_i64(input.digits_per_number, 1, 18) as u32;
    let total_numbers = clamp_i64(input.total_numbers, 1, 10_000) as u32;

    // UI typically uses 0.1–5s, but we allow up to 60s defensively.
    let duration_s = clamp_f64(input.number_duration_s, 0.1, 60.0);
    let delay_s = clamp_f64(input.delay_between_numbers_s, 0.0, 60.0);

    let number_duration_ms = seconds_to_ms_clamped(duration_s, 1, 60_000);
    let delay_between_numbers_ms = seconds_to_ms_clamped(delay_s, 0, 60_000);

    let config = SessionConfig {
        digits_per_number: digits,
        number_duration_ms,
        delay_between_numbers_ms,
        total_numbers,
        allow_negative_numbers: input.allow_negative_numbers,
    };

    let effective = SessionConfigEffective {
        digits_per_number: config.digits_per_number,
        number_duration_s: round_1_decimal(config.number_duration_ms as f64 / 1000.0),
        delay_between_numbers_s: round_1_decimal(config.delay_between_numbers_ms as f64 / 1000.0),
        total_numbers: config.total_numbers,
        allow_negative_numbers: config.allow_negative_numbers,
    };

    (config, effective)
}

#[derive(Debug, Clone)]
pub enum SessionState {
    Idle,
    #[allow(dead_code)]
    ShowingNumbers {
        current: u32,
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

impl SessionManager {
    const MAX_RECENT_RESULTS: usize = 8;

    fn cleanup_finished_worker(&self) {
        let mut worker = self.worker.lock().expect("worker lock poisoned");
        if let Some(handle) = worker.as_ref() {
            if handle.is_finished() {
                let handle = worker.take().expect("just checked Some");
                let _ = handle.join();
                *self.stop.lock().expect("stop lock poisoned") = None;
            }
        }
    }

    pub fn start(&self, app: AppHandle, config: SessionConfig) -> Result<u64, String> {
        self.cleanup_finished_worker();

        validate_config(&config)?;

        {
            let worker = self.worker.lock().expect("worker lock poisoned");
            if let Some(handle) = worker.as_ref() {
                if !handle.is_finished() {
                    return Err("session already running".to_string());
                }
            }
        }

        let stop_flag = Arc::new(AtomicBool::new(false));
        *self.stop.lock().expect("stop lock poisoned") = Some(stop_flag.clone());

        {
            let mut state = self.state.lock().expect("state lock poisoned");
            *state = SessionState::ShowingNumbers {
                current: 0,
                total: config.total_numbers,
            };
        }

        // New session id for this run.
        let session_id = self.next_session_id.fetch_add(1, Ordering::SeqCst);

        let state_arc = Arc::clone(&self.state);
        let recent_results_arc = Arc::clone(&self.recent_results);
        let plan_arc = Arc::clone(&self.auto_repeat_plan);
        let handle = thread::spawn(move || {
            run_session_loop(
                app,
                config,
                state_arc,
                stop_flag,
                session_id,
                recent_results_arc,
                plan_arc,
            );
        });

        *self.worker.lock().expect("worker lock poisoned") = Some(handle);
        Ok(session_id)
    }

    pub fn configure_auto_repeat(&self, plan: Option<AutoRepeatPlan>) {
        *self
            .auto_repeat_plan
            .lock()
            .expect("auto_repeat_plan lock poisoned") = plan;
        // Bump generation so any previously scheduled starts become no-ops.
        self.auto_repeat_generation.fetch_add(1, Ordering::SeqCst);
    }

    pub fn auto_repeat_generation(&self) -> u64 {
        self.auto_repeat_generation.load(Ordering::SeqCst)
    }

    pub fn result_for(&self, session_id: u64) -> Result<SessionComplete, String> {
        let guard = self
            .recent_results
            .lock()
            .expect("recent_results lock poisoned");

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
            let mut plan_guard = self
                .auto_repeat_plan
                .lock()
                .expect("auto_repeat_plan lock poisoned");
            let Some(plan) = plan_guard.as_mut() else {
                return Ok(None);
            };

            if plan.awaiting_validation_session_id != Some(session_id) {
                return Ok(None);
            }

            if plan.remaining == 0 {
                return Ok(None);
            }

            plan.awaiting_validation_session_id = None;
            plan.remaining = plan.remaining.saturating_sub(1);

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

        // Cancel any pending auto-repeat and forget last result.
        self.configure_auto_repeat(None);
        self.recent_results
            .lock()
            .expect("recent_results lock poisoned")
            .clear();

        let stop_flag = self.stop.lock().expect("stop lock poisoned").take();
        if let Some(flag) = stop_flag {
            flag.store(true, Ordering::SeqCst);
        }

        if let Some(handle) = self.worker.lock().expect("worker lock poisoned").take() {
            let _ = handle.join();
        }

        let mut state = self.state.lock().expect("state lock poisoned");
        *state = SessionState::Idle;
    }
}

fn validate_config(config: &SessionConfig) -> Result<(), String> {
    if config.digits_per_number == 0 || config.number_duration_ms == 0 || config.total_numbers == 0
    {
        return Err(
            "digits_per_number, number_duration_ms, and total_numbers must be > 0".to_string(),
        );
    }

    // Keep generation simple and safe: 10^digits must fit in u64.
    if config.digits_per_number > 18 {
        return Err("digits_per_number must be <= 18".to_string());
    }

    // Defensive caps: UI enforces ranges, but IPC inputs must be treated as untrusted.
    // These limits are generous enough for real use while preventing accidental runaway sessions.
    if config.total_numbers > 10_000 {
        return Err("total_numbers must be <= 10000".to_string());
    }

    if config.number_duration_ms > 60_000 {
        return Err("number_duration_ms must be <= 60000".to_string());
    }

    if config.delay_between_numbers_ms > 60_000 {
        return Err("delay_between_numbers_ms must be <= 60000".to_string());
    }

    Ok(())
}

fn run_session_loop(
    app: AppHandle,
    config: SessionConfig,
    state: Arc<Mutex<SessionState>>,
    stop: Arc<AtomicBool>,
    session_id: u64,
    recent_results: Arc<Mutex<VecDeque<SessionComplete>>>,
    auto_repeat_plan: Arc<Mutex<Option<AutoRepeatPlan>>>,
) {
    let _ = app.emit("clear_screen", ());

    // Phase 4: 3-second countdown before first number.
    let countdown_start = Instant::now();
    for (idx, value) in [3u32, 2u32, 1u32].into_iter().enumerate() {
        if stop.load(Ordering::SeqCst) {
            let _ = app.emit("clear_screen", ());
            let mut st = state.lock().expect("state lock poisoned");
            *st = SessionState::Idle;
            return;
        }

        let show_at = countdown_start + Duration::from_secs(idx as u64);
        sleep_until_interruptible(show_at, &stop);
        if stop.load(Ordering::SeqCst) {
            let _ = app.emit("clear_screen", ());
            let mut st = state.lock().expect("state lock poisoned");
            *st = SessionState::Idle;
            return;
        }

        let _ = app.emit("countdown_tick", value.to_string());
    }

    let begin_at = countdown_start + Duration::from_secs(3);
    sleep_until_interruptible(begin_at, &stop);
    if stop.load(Ordering::SeqCst) {
        let _ = app.emit("clear_screen", ());
        let mut st = state.lock().expect("state lock poisoned");
        *st = SessionState::Idle;
        return;
    }
    let _ = app.emit("clear_screen", ());

    let number_duration = Duration::from_millis(config.number_duration_ms);
    let gap_duration = Duration::from_millis(config.delay_between_numbers_ms);

    // Use sequential scheduling (relative to actual emission times) so we never
    // "catch up" by skipping visibility when the process is delayed.
    let mut next_on_at = Instant::now();

    let mut rng = rand::thread_rng();
    let mut last_payload: Option<String> = None;
    let mut running_sum: i128 = 0;
    let mut numbers: Vec<i64> = Vec::with_capacity(config.total_numbers as usize);
    let mut sum_i128: i128 = 0;

    for i in 0..config.total_numbers {
        if stop.load(Ordering::SeqCst) {
            let _ = app.emit("clear_screen", ());
            let mut st = state.lock().expect("state lock poisoned");
            *st = SessionState::Idle;
            return;
        }

        sleep_until_interruptible(next_on_at, &stop);
        if stop.load(Ordering::SeqCst) {
            let _ = app.emit("clear_screen", ());
            let mut st = state.lock().expect("state lock poisoned");
            *st = SessionState::Idle;
            return;
        }

        let (payload, payload_value) = {
            // Prevent consecutive numbers from being identical.
            // Also enforce:
            // - The first number is never negative.
            // - When negatives are enabled, the running sum never drops below zero.
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
                    // Fall back (should be unreachable with valid configs).
                    break (candidate, candidate_value);
                }
            }
        };

        {
            let mut st = state.lock().expect("state lock poisoned");
            *st = SessionState::ShowingNumbers {
                current: i + 1,
                total: config.total_numbers,
            };
        }

        last_payload = Some(payload.clone());
        running_sum = (running_sum + payload_value).max(0);
        sum_i128 += payload_value;

        let value_i64: i64 = payload_value
            .try_into()
            .expect("payload_value should fit into i64 with current constraints");
        numbers.push(value_i64);

        let _ = app.emit(
            "show_number",
            ShowNumber {
                session_id,
                index: i + 1,
                total: config.total_numbers,
                value: value_i64,
                running_sum: running_sum
                    .try_into()
                    .expect("running_sum should fit into i64 with current constraints"),
            },
        );

        let shown_at = Instant::now();
        sleep_until_interruptible(shown_at + number_duration, &stop);
        let _ = app.emit("clear_screen", ());

        if stop.load(Ordering::SeqCst) {
            let mut st = state.lock().expect("state lock poisoned");
            *st = SessionState::Idle;
            return;
        }

        let cleared_at = Instant::now();
        next_on_at = cleared_at + gap_duration;
    }

    let _ = app.emit("clear_screen", ());

    let sum_i64: i64 = sum_i128
        .try_into()
        .expect("sum should fit into i64 with current constraints");
    let result = SessionComplete {
        session_id,
        numbers,
        sum: sum_i64,
    };

    {
        let mut guard = recent_results.lock().expect("recent_results lock poisoned");
        guard.push_back(result.clone());
        while guard.len() > SessionManager::MAX_RECENT_RESULTS {
            guard.pop_front();
        }
    }
    let _ = app.emit("session_complete", result.clone());

    // If auto-repeat is configured and there are repeats remaining, arm it to wait for validation.
    {
        let mut plan_guard = auto_repeat_plan
            .lock()
            .expect("auto_repeat_plan lock poisoned");
        if let Some(plan) = plan_guard.as_mut() {
            if plan.remaining > 0 {
                plan.awaiting_validation_session_id = Some(session_id);
            }
        }
    }

    let mut st = state.lock().expect("state lock poisoned");
    *st = SessionState::Complete;
}

fn random_fixed_digits_no_leading_zero(rng: &mut impl Rng, digits: u32) -> String {
    if digits <= 1 {
        // No leading zero => 0 is excluded.
        return rng.gen_range(1u32..=9u32).to_string();
    }

    let min = 10u64.pow(digits - 1);
    let max_exclusive = 10u64.pow(digits);
    rng.gen_range(min..max_exclusive).to_string()
}

fn random_fixed_digits_no_leading_zero_capped(
    rng: &mut impl Rng,
    digits: u32,
    max_inclusive: u64,
) -> Option<String> {
    if digits <= 1 {
        if max_inclusive < 1 {
            return None;
        }
        let max = max_inclusive.min(9);
        return Some(rng.gen_range(1u64..=max).to_string());
    }

    let min = 10u64.pow(digits - 1);
    if max_inclusive < min {
        return None;
    }

    let max_exclusive = 10u64.pow(digits);
    let cap_exclusive = (max_inclusive.saturating_add(1)).min(max_exclusive);
    Some(rng.gen_range(min..cap_exclusive).to_string())
}

fn random_number_with_constraints(
    rng: &mut impl Rng,
    digits: u32,
    allow_negative_numbers: bool,
    index: u32,
    running_sum: i128,
) -> (String, i128) {
    // Requirement: first number is never negative.
    let allow_negative_here = allow_negative_numbers && index > 0;

    // Cap for negative magnitudes: cannot exceed current running sum, and cannot exceed
    // the maximum representable magnitude for the requested digit count.
    let max_for_digits = if digits <= 1 {
        9u64
    } else {
        10u64.pow(digits).saturating_sub(1)
    };

    let sum_cap_u64 = if running_sum <= 0 {
        0u64
    } else {
        (running_sum.min(max_for_digits as i128)) as u64
    };

    let can_choose_negative = allow_negative_here && sum_cap_u64 > 0;
    let try_negative = can_choose_negative && rng.gen_bool(0.5);

    if try_negative {
        if let Some(magnitude) =
            random_fixed_digits_no_leading_zero_capped(rng, digits, sum_cap_u64)
        {
            let magnitude_value: i128 = magnitude
                .parse::<i128>()
                .expect("generated magnitude should parse as integer");
            // Enforce non-negative running sum after applying this value.
            if running_sum - magnitude_value >= 0 {
                return (format!("-{magnitude}"), -magnitude_value);
            }
        }
    }

    let magnitude = random_fixed_digits_no_leading_zero(rng, digits);
    let magnitude_value: i128 = magnitude
        .parse::<i128>()
        .expect("generated magnitude should parse as integer");
    (magnitude, magnitude_value)
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
