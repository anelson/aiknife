//! Implementation of the GPT-like chat interface backend.
use std::sync::Arc;

use crate::{Result, UiError};
use aiknife::chat;
use serde::{Deserialize, Serialize};
use tauri_specta::Event;
use tracing::*;

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
        pub(crate) role: chat::Role,
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
    use std::sync::Arc;

    use super::*;
    use messages::*;

    #[derive(Serialize, Clone, specta::Type, tauri_specta::Event)]
    pub struct MessagePending {
        pub(crate) message: ChatMessage,
    }
    #[derive(Serialize, Clone, specta::Type, tauri_specta::Event)]
    pub struct MessageResponse {
        pub(crate) message: ChatMessage,
    }

    #[derive(Serialize, Clone, specta::Type, tauri_specta::Event)]
    pub struct MessageError {
        pub(crate) message_id: uuid::Uuid,
        #[specta(type = UiError)]
        pub(crate) error: Arc<UiError>,
    }
}

pub struct AppState {
    session_manager: chat::SessionManager,
}

impl AppState {
    pub fn new() -> Self {
        Self {
            session_manager: chat::SessionManager::load().expect("Failed to load sessions"),
        }
    }
}

#[tauri::command]
#[specta::specta]
pub async fn new_session(state: tauri::State<'_, AppState>) -> Result<messages::SessionHandle> {
    let session = state.session_manager.new_session()?;
    Ok(messages::SessionHandle { id: session.id() })
}

#[tauri::command]
#[specta::specta]
#[tracing::instrument(skip_all, err, fields(session = %session.id))]
pub async fn send_message(
    state: tauri::State<'_, AppState>,
    window: tauri::Window,
    session: messages::SessionHandle,
    message: String,
) -> Result<uuid::Uuid> {
    debug!(%message, "Sending chat message");

    let session = state.session_manager.get_session(session.id)?;

    // Create user message
    let message_id = uuid::Uuid::now_v7();
    let user_message = messages::ChatMessage {
        id: message_id,
        role: chat::Role::User,
        content: message.clone(),
        status: messages::MessageStatus::Pending,
    };

    // Emit event that message is being processed
    events::MessagePending {
        message: user_message.clone(),
    }
    .emit(&window)?;

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
                        role: chat::Role::Assistant,
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
                        error: Arc::new(UiError::from(e)),
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
pub fn check_api_key() -> Result<()> {
    chat::check_api_key()?;

    Ok(())
}