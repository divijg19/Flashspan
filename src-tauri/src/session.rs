use rand::Rng;
use serde::Deserialize;
use std::{
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
    thread,
    thread::JoinHandle,
    time::{Duration, Instant},
};
use tauri::{AppHandle, Emitter};

#[derive(Debug, Clone, Deserialize)]
pub struct SessionConfig {
    pub digits_per_number: u32,
    pub number_duration_ms: u64,
    pub delay_between_numbers_ms: u64,
    pub total_numbers: u32,

    #[serde(default)]
    pub allow_negative_numbers: bool,
}

#[derive(Debug, Clone)]
pub enum SessionState {
    Idle,
    ShowingNumbers { current: u32, total: u32 },
    Complete,
}

pub struct SessionManager {
    state: Arc<Mutex<SessionState>>,
    worker: Mutex<Option<JoinHandle<()>>>,
    stop: Mutex<Option<Arc<AtomicBool>>>,
}

impl Default for SessionManager {
    fn default() -> Self {
        Self {
            state: Arc::new(Mutex::new(SessionState::Idle)),
            worker: Mutex::new(None),
            stop: Mutex::new(None),
        }
    }
}

impl SessionManager {
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

    pub fn start(&self, app: AppHandle, config: SessionConfig) -> Result<(), String> {
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

        let state_arc = Arc::clone(&self.state);
        let handle = thread::spawn(move || {
            run_session_loop(app, config, state_arc, stop_flag);
        });

        *self.worker.lock().expect("worker lock poisoned") = Some(handle);
        Ok(())
    }

    pub fn stop(&self) {
        self.cleanup_finished_worker();

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

    Ok(())
}

fn run_session_loop(
    app: AppHandle,
    config: SessionConfig,
    state: Arc<Mutex<SessionState>>,
    stop: Arc<AtomicBool>,
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
        let _ = app.emit("show_number", payload);

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
    let _ = app.emit("session_complete", ());

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
