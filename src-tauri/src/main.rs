#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod session;

use serde::Deserialize;
use session::{
    normalize_session_config, AutoRepeatPlan, SessionConfigEffective, SessionConfigInput,
    SessionManager,
};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Instant;
use std::time::{SystemTime, UNIX_EPOCH};
use tauri::Emitter;

#[tauri::command]
fn ping() -> String {
    "pong".to_string()
}

#[tauri::command]
fn stop_session(manager: tauri::State<'_, Arc<SessionManager>>) {
    manager.stop();
}

#[tauri::command]
fn cancel_auto_repeat(manager: tauri::State<'_, Arc<SessionManager>>) {
    manager.configure_auto_repeat(None);
}

#[derive(Debug, Clone, Deserialize)]
struct AutoRepeatConfigInput {
    enabled: bool,
    repeats: i64,
    delay_s: f64,
}

#[derive(Debug, Clone, serde::Serialize)]
struct AutoRepeatEffective {
    enabled: bool,
    repeats: u32,
    delay_s: f64,
}

#[derive(Debug, Clone, serde::Serialize)]
struct StartSessionResponse {
    session_id: u64,
    effective_config: SessionConfigEffective,
    effective_auto_repeat: Option<AutoRepeatEffective>,
}

#[derive(Debug, Clone, serde::Serialize)]
struct AutoRepeatWaitingPayload {
    session_id: u64,
    next_start_at_ms: u64,
    remaining: u32,
}

#[derive(Debug, Clone, serde::Serialize)]
struct AutoRepeatTickPayload {
    session_id: u64,
    seconds_left: u64,
    remaining: u32,
}

#[derive(Debug, Clone, Copy, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
enum ColorScheme {
    Midnight,
    Ivory,
    Crimson,
    Aqua,
    Violet,
    Amber,
}

#[derive(Debug, Clone, Copy, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
enum ThemeMode {
    Dark,
    Light,
}

#[derive(Debug, Clone, serde::Serialize)]
struct AppSettings {
    color_scheme: ColorScheme,
    theme_mode: ThemeMode,
}

#[derive(Default)]
struct SettingsState(Mutex<AppSettings>);

impl Default for AppSettings {
    fn default() -> Self {
        Self {
            color_scheme: ColorScheme::Midnight,
            theme_mode: ThemeMode::Dark,
        }
    }
}

#[tauri::command]
fn get_app_settings(settings: tauri::State<'_, SettingsState>) -> AppSettings {
    settings.0.lock().expect("settings lock poisoned").clone()
}

#[tauri::command]
fn set_color_scheme(
    app: tauri::AppHandle,
    settings: tauri::State<'_, SettingsState>,
    color_scheme: ColorScheme,
) -> Result<AppSettings, String> {
    let updated = {
        let mut guard = settings.0.lock().expect("settings lock poisoned");
        guard.color_scheme = color_scheme;
        guard.clone()
    };
    let _ = app.emit("app_settings_changed", updated.clone());
    Ok(updated)
}

#[tauri::command]
fn set_theme_mode(
    app: tauri::AppHandle,
    settings: tauri::State<'_, SettingsState>,
    theme_mode: ThemeMode,
) -> Result<AppSettings, String> {
    let updated = {
        let mut guard = settings.0.lock().expect("settings lock poisoned");
        guard.theme_mode = theme_mode;
        guard.clone()
    };

    let _ = app.emit("app_settings_changed", updated.clone());
    Ok(updated)
}

#[derive(Debug, Clone, serde::Serialize)]
struct ValidationResult {
    expected_sum: i64,
    provided_sum: i64,
    correct: bool,
    delta: i64,
}

#[derive(Debug, Clone, serde::Serialize)]
struct SubmitAnswerResponse {
    validation: ValidationResult,
    auto_repeat_waiting: Option<AutoRepeatWaitingPayload>,
}

fn parse_answer_text(input: &str) -> Result<i64, String> {
    let cleaned = input.trim().replace(',', "");
    if cleaned.is_empty() {
        return Err("Enter a single integer answer (e.g. 42 or -17).".to_string());
    }

    // Defensive bound: avoid absurd payload sizes.
    if cleaned.len() > 64 {
        return Err("Enter a single integer answer (e.g. 42 or -17).".to_string());
    }

    cleaned
        .parse::<i64>()
        .map_err(|_| "Enter a single integer answer (e.g. 42 or -17).".to_string())
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

fn schedule_auto_repeat_if_needed(
    app: tauri::AppHandle,
    manager: Arc<SessionManager>,
    session_id: u64,
) -> Result<Option<AutoRepeatWaitingPayload>, String> {
    let Some((delay_ms, remaining, config, generation)) =
        manager.mark_validated_and_schedule_info(session_id)?
    else {
        return Ok(None);
    };

    let next_start_at_ms = now_ms().saturating_add(delay_ms);
    let payload = AutoRepeatWaitingPayload {
        session_id,
        next_start_at_ms,
        remaining,
    };

    let _ = app.emit("auto_repeat_waiting", payload.clone());

    let manager_arc = Arc::clone(&manager);
    let app_for_thread = app.clone();
    let remaining_repeats = remaining;
    thread::spawn(move || {
        let end_at = Instant::now() + std::time::Duration::from_millis(delay_ms);
        let mut last_sent: Option<u64> = None;

        loop {
            if manager_arc.auto_repeat_generation() != generation {
                return;
            }

            let now = Instant::now();
            if now >= end_at {
                break;
            }

            let remaining_duration = end_at.saturating_duration_since(now);
            let seconds_left = (remaining_duration.as_millis() as u64).div_ceil(1000);

            if last_sent != Some(seconds_left) {
                last_sent = Some(seconds_left);
                let _ = app_for_thread.emit(
                    "auto_repeat_tick",
                    AutoRepeatTickPayload {
                        session_id,
                        seconds_left,
                        remaining: remaining_repeats,
                    },
                );
            }

            let step = remaining_duration.min(std::time::Duration::from_millis(120));
            thread::sleep(step);
        }

        if manager_arc.auto_repeat_generation() != generation {
            return;
        }

        let _ = app_for_thread.emit(
            "auto_repeat_tick",
            AutoRepeatTickPayload {
                session_id,
                seconds_left: 0,
                remaining: remaining_repeats,
            },
        );

        let _ = manager_arc.start(app_for_thread, config);
    });

    Ok(Some(payload))
}

#[tauri::command]
fn start_session(
    app: tauri::AppHandle,
    manager: tauri::State<'_, Arc<SessionManager>>,
    config: SessionConfigInput,
    auto_repeat: Option<AutoRepeatConfigInput>,
) -> Result<StartSessionResponse, String> {
    let (config, effective_config) = normalize_session_config(config);

    // Configure auto-repeat plan for this run (or clear it).
    let effective_auto_repeat = if let Some(ar) = auto_repeat {
        if ar.enabled {
            let repeats = ar.repeats.clamp(1, 20) as u32;
            let delay_s = if ar.delay_s.is_finite() {
                ar.delay_s.clamp(5.0, 120.0)
            } else {
                5.0
            };
            let delay_ms = ((delay_s * 1000.0).round() as u64).max(5_000);

            manager.configure_auto_repeat(Some(AutoRepeatPlan {
                remaining: repeats,
                delay_ms,
                config: config.clone(),
                awaiting_validation_session_id: None,
            }));

            Some(AutoRepeatEffective {
                enabled: true,
                repeats,
                delay_s: (delay_ms as f64 / 1000.0),
            })
        } else {
            manager.configure_auto_repeat(None);
            None
        }
    } else {
        manager.configure_auto_repeat(None);
        None
    };

    let session_id = manager.start(app, config)?;
    Ok(StartSessionResponse {
        session_id,
        effective_config,
        effective_auto_repeat,
    })
}

#[tauri::command]
fn mark_validated(
    app: tauri::AppHandle,
    manager: tauri::State<'_, Arc<SessionManager>>,
    session_id: u64,
) -> Result<Option<AutoRepeatWaitingPayload>, String> {
    schedule_auto_repeat_if_needed(app, Arc::clone(&*manager), session_id)
}

#[tauri::command]
fn acknowledge_complete(
    app: tauri::AppHandle,
    manager: tauri::State<'_, Arc<SessionManager>>,
    session_id: u64,
) -> Result<Option<AutoRepeatWaitingPayload>, String> {
    schedule_auto_repeat_if_needed(app, Arc::clone(&*manager), session_id)
}

#[tauri::command]
fn submit_answer(
    app: tauri::AppHandle,
    manager: tauri::State<'_, Arc<SessionManager>>,
    session_id: u64,
    provided_sum: i64,
) -> Result<SubmitAnswerResponse, String> {
    let result = manager.result_for(session_id)?;
    let expected_sum = result.sum;

    let delta = provided_sum.saturating_sub(expected_sum);
    let correct = delta == 0;

    let validation = ValidationResult {
        expected_sum,
        provided_sum,
        correct,
        delta,
    };

    let waiting = schedule_auto_repeat_if_needed(app, Arc::clone(&*manager), session_id)?;
    Ok(SubmitAnswerResponse {
        validation,
        auto_repeat_waiting: waiting,
    })
}

#[tauri::command]
fn submit_answer_text(
    app: tauri::AppHandle,
    manager: tauri::State<'_, Arc<SessionManager>>,
    session_id: u64,
    provided_text: String,
) -> Result<SubmitAnswerResponse, String> {
    let provided_sum = parse_answer_text(&provided_text)?;
    submit_answer(app, manager, session_id, provided_sum)
}

fn main() {
    tauri::Builder::default()
        .manage(Arc::new(SessionManager::default()))
        .manage(SettingsState::default())
        .invoke_handler(tauri::generate_handler![
            ping,
            get_app_settings,
            set_color_scheme,
            set_theme_mode,
            start_session,
            stop_session,
            cancel_auto_repeat,
            mark_validated,
            acknowledge_complete,
            submit_answer,
            submit_answer_text
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
