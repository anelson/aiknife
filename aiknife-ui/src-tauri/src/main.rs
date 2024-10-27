// Prevents additional console window on Windows in release, DO NOT REMOVE!!
#![cfg_attr(
    all(not(debug_assertions), target_os = "windows"),
    windows_subsystem = "windows"
)]
use aiknife::chat;
use serde::{Deserialize, Serialize};
use specta_typescript::Typescript;
use tauri::Manager;

struct AppState {
    session_manager: chat::SessionManager,
}

#[derive(Serialize, Deserialize, specta::Type)]
pub struct SessionHandle {
    id: uuid::Uuid,
}

#[tauri::command]
#[specta::specta]
async fn new_session(state: tauri::State<'_, AppState>) -> Result<SessionHandle, String> {
    let session = state
        .session_manager
        .new_session()
        .map_err(|e| e.to_string())?;
    Ok(SessionHandle { id: session.id() })
}

#[tauri::command]
#[specta::specta]
async fn send_message(
    state: tauri::State<'_, AppState>,
    session: SessionHandle,
    message: String,
) -> Result<String, String> {
    let session = state
        .session_manager
        .get_session(session.id)
        .map_err(|e| e.to_string())?;
    session
        .chat_completion(chat::Message::new_user_message(message))
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
#[specta::specta]
fn check_api_key() -> Result<(), String> {
    chat::check_api_key().map_err(|e| e.to_string())
}

fn main() {
    let builder = tauri_specta::Builder::<tauri::Wry>::new()
        .commands(tauri_specta::collect_commands![
            new_session,
            send_message,
            check_api_key,
        ])
        .error_handling(tauri_specta::ErrorHandlingMode::Throw);

    #[cfg(debug_assertions)] // <- Only export on non-release builds
    builder
        .export(Typescript::default(), "../src/bindings.ts")
        .expect("Failed to export typescript bindings");

    tauri::Builder::default()
        .setup(|app| {
            #[cfg(debug_assertions)]
            {
                let window = app.get_webview_window("main").unwrap();
                window.open_devtools();
            }
            Ok(())
        })
        .manage(AppState {
            session_manager: chat::SessionManager::load().expect("Failed to load sessions"),
        })
        .invoke_handler(builder.invoke_handler())
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
