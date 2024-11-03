use aiknife::chat;
use serde::{Deserialize, Serialize};
use specta_typescript::Typescript;
use tauri::Emitter;
use tauri::Manager;
use tauri_specta::Event;

pub mod messages {
    use super::*;

    #[derive(Serialize, Deserialize, Clone, specta::Type)]
    pub enum MessageStatus {
        Pending,
        Complete,
    }

    #[derive(Serialize, Deserialize, Clone, specta::Type)]
    pub struct ChatMessage {
        pub(crate) id: uuid::Uuid,
        pub(crate) role: String,
        pub(crate) content: String,
        pub(crate) status: MessageStatus,
    }

    #[derive(Serialize, Deserialize, specta::Type)]
    pub struct SessionHandle {
        pub(crate) id: uuid::Uuid,
    }

    #[derive(Serialize, Deserialize, specta::Type)]
    pub struct MessagePair {
        pub(crate) user_message: ChatMessage,
        pub(crate) assistant_message: ChatMessage,
    }
}

pub mod events {
    use super::*;
    use messages::*;

    #[derive(Serialize, Deserialize, Clone, specta::Type, tauri_specta::Event)]
    pub struct MessagePending {
        pub(crate) message: ChatMessage,
    }
    #[derive(Serialize, Deserialize, Clone, specta::Type, tauri_specta::Event)]
    pub struct MessageResponse {
        pub(crate) message: ChatMessage,
    }

    #[derive(Serialize, Deserialize, Clone, specta::Type, tauri_specta::Event)]
    pub struct MessageError {
        pub(crate) message_id: uuid::Uuid,
        pub(crate) error: String,
    }
}

pub struct AppState {
    session_manager: chat::SessionManager,
}

#[tauri::command]
#[specta::specta]
async fn new_session(state: tauri::State<'_, AppState>) -> Result<messages::SessionHandle, String> {
    let session = state
        .session_manager
        .new_session()
        .map_err(|e| e.to_string())?;
    Ok(messages::SessionHandle { id: session.id() })
}

#[tauri::command]
#[specta::specta]
async fn send_message(
    state: tauri::State<'_, AppState>,
    window: tauri::Window,
    session: messages::SessionHandle,
    message: String,
) -> Result<uuid::Uuid, String> {
    let session = state
        .session_manager
        .get_session(session.id)
        .map_err(|e| e.to_string())?;

    // Create user message
    let message_id = uuid::Uuid::now_v7();
    let user_message = messages::ChatMessage {
        id: message_id,
        role: "user".to_string(),
        content: message.clone(),
        status: messages::MessageStatus::Pending,
    };

    // Emit event that message is being processed
    events::MessagePending {
        message: user_message.clone(),
    }
    .emit(&window)
    .map_err(|e| e.to_string())?;

    // Spawn a task to handle the API call
    tauri::async_runtime::spawn({
        let window = window.clone();

        async move {
            let outgoing_message_id = user_message.id;
            match session
                .chat_completion(chat::Message::new_user_message(message))
                .await
            {
                Ok(response) => {
                    let assistant_message = messages::ChatMessage {
                        id: uuid::Uuid::now_v7(),
                        role: "assistant".to_string(),
                        content: response,
                        status: messages::MessageStatus::Complete,
                    };

                    // Emit success event with assistant's response
                    let _ = events::MessageResponse {
                        message: assistant_message.clone(),
                    }
                    .emit(&window);
                }
                Err(e) => {
                    // Emit error event
                    let _ = events::MessageError {
                        message_id: outgoing_message_id,
                        error: e.to_string(),
                    }
                    .emit(&window);
                }
            }
        }
    });

    Ok(message_id)
}

#[tauri::command]
#[specta::specta]
fn check_api_key() -> Result<(), String> {
    chat::check_api_key().map_err(|e| e.to_string())
}

pub fn run() {
    let builder = tauri_specta::Builder::<tauri::Wry>::new()
        .commands(tauri_specta::collect_commands![
            new_session,
            send_message,
            check_api_key,
        ])
        .events(tauri_specta::collect_events![
            events::MessagePending,
            events::MessageResponse,
            events::MessageError
        ])
        .error_handling(tauri_specta::ErrorHandlingMode::Throw);

    #[cfg(debug_assertions)] // <- Only export on non-release builds
    builder
        .export(Typescript::default(), "../src/bindings.ts")
        .expect("Failed to export typescript bindings");

    tauri::Builder::default()
        .invoke_handler(builder.invoke_handler())
        .manage(AppState {
            session_manager: chat::SessionManager::load().expect("Failed to load sessions"),
        })
        .setup(move |app| {
            #[cfg(debug_assertions)]
            {
                let window = app.get_webview_window("main").unwrap();
                window.open_devtools();
            }

            // Wire up the tauri-specta strongly-typed event magic
            builder.mount_events(app);

            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
