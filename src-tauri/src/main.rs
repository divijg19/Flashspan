#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod session;

use session::{SessionConfig, SessionManager};

#[tauri::command]
fn ping() -> String {
    "pong".to_string()
}

#[tauri::command]
fn start_session(
    app: tauri::AppHandle,
    manager: tauri::State<'_, SessionManager>,
    config: SessionConfig,
) -> Result<(), String> {
    manager.start(app, config)
}

#[tauri::command]
fn stop_session(manager: tauri::State<'_, SessionManager>) {
    manager.stop();
}

fn main() {
    tauri::Builder::default()
        .manage(SessionManager::default())
        .invoke_handler(tauri::generate_handler![ping, start_session, stop_session])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
