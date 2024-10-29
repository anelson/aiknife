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
pub enum MessageStatus {
    Pending,
    Complete,
}

#[derive(Serialize, Deserialize, specta::Type)]
pub struct ChatMessage {
    id: uuid::Uuid,
    role: String,
    content: String,
    status: MessageStatus,
}

#[derive(Serialize, Deserialize, specta::Type)]
pub struct SessionHandle {
    id: uuid::Uuid,
}

#[derive(Serialize, Deserialize, specta::Type)]
pub struct MessagePair {
    user_message: ChatMessage,
    assistant_message: ChatMessage,
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
) -> Result<MessagePair, String> {
    let session = state
        .session_manager
        .get_session(session.id)
        .map_err(|e| e.to_string())?;

    // Create user message
    let user_message = ChatMessage {
        id: uuid::Uuid::now_v7(),
        role: "user".to_string(),
        content: message.clone(),
        status: MessageStatus::Complete,
    };

    // Get response from AI
    let response = session
        .chat_completion(chat::Message::new_user_message(message))
        .await
        .map_err(|e| e.to_string())?;

    // Create assistant message
    let assistant_message = ChatMessage {
        id: uuid::Uuid::now_v7(),
        role: "assistant".to_string(),
        content: response,
        status: MessageStatus::Complete,
    };

    Ok(MessagePair {
        user_message,
        assistant_message,
    })
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
