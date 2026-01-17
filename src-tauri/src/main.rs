#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod session;

use serde::Deserialize;
use session::{AutoRepeatPlan, SessionConfig, SessionManager};
use std::sync::Arc;
use std::thread;
use std::time::{SystemTime, UNIX_EPOCH};
use tauri::Emitter;

#[tauri::command]
fn ping() -> String {
    "pong".to_string()
}

#[tauri::command]
fn start_session(
    app: tauri::AppHandle,
    manager: tauri::State<'_, Arc<SessionManager>>,
    config: SessionConfig,
) -> Result<(), String> {
    let _ = manager.start(app, config)?;
    Ok(())
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
struct AutoRepeatConfig {
    enabled: bool,
    repeats: u32,
    delay_ms: u64,
}

#[derive(Debug, Clone, serde::Serialize)]
struct AutoRepeatWaitingPayload {
    session_id: u64,
    next_start_at_ms: u64,
    remaining: u32,
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
    thread::spawn(move || {
        thread::sleep(std::time::Duration::from_millis(delay_ms));
        if manager_arc.auto_repeat_generation() != generation {
            return;
        }
        let _ = manager_arc.start(app, config);
    });

    Ok(Some(payload))
}

#[tauri::command]
fn start_session_v2(
    app: tauri::AppHandle,
    manager: tauri::State<'_, Arc<SessionManager>>,
    config: SessionConfig,
    auto_repeat: Option<AutoRepeatConfig>,
) -> Result<u64, String> {
    // Configure auto-repeat plan for this run (or clear it).
    if let Some(ar) = auto_repeat {
        if ar.enabled {
            let repeats = ar.repeats.clamp(1, 20);
            let delay_ms = ar.delay_ms.max(5_000);
            manager.configure_auto_repeat(Some(AutoRepeatPlan {
                remaining: repeats,
                delay_ms,
                config: config.clone(),
                awaiting_validation_session_id: None,
            }));
        } else {
            manager.configure_auto_repeat(None);
        }
    } else {
        manager.configure_auto_repeat(None);
    }

    manager.start(app, config)
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

fn main() {
    tauri::Builder::default()
        .manage(Arc::new(SessionManager::default()))
        .invoke_handler(tauri::generate_handler![
            ping,
            start_session,
            start_session_v2,
            stop_session,
            cancel_auto_repeat,
            mark_validated,
            acknowledge_complete,
            submit_answer
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
