// Prevents additional console window on Windows in release, DO NOT REMOVE!!
#![cfg_attr(
    all(not(debug_assertions), target_os = "windows"),
    windows_subsystem = "windows"
)]
use aiknife::{chat_completion, Message, check_api_key};
use tauri::Manager;

#[tauri::command]
async fn send_message(messages: Vec<Message>) -> Result<String, String> {
    chat_completion(messages).await.map_err(|e| e.to_string())
}

#[tauri::command]
fn check_api_key_command() -> Result<(), String> {
    check_api_key().map_err(|e| e.to_string())
}

fn main() {
    tauri::Builder::default()
        .setup(|app| {
            #[cfg(debug_assertions)]
            {
                let window = app.get_webview_window("main").unwrap();
                window.open_devtools();
            }
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![send_message, check_api_key_command])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
