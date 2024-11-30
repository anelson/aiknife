//! Implementation of the GPT-like chat interface backend.
use crate::{Result, UiError};
use aiknife::chat;
use futures::StreamExt;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use tauri_specta::Event;
use tracing::*;

pub mod messages {
    use super::*;

    #[derive(Serialize, Clone, specta::Type)]
    #[serde(tag = "type")]
    pub enum MessageStatus {
        Pending,
        Processing,
        Streaming,
        Complete,
        Error {
            #[specta(type = UiError)]
            error: Arc<UiError>,
        },
    }

    #[derive(Serialize, Clone, specta::Type)]
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
}

pub mod events {
    use super::*;
    use messages::*;

    #[derive(Serialize, Clone, specta::Type, tauri_specta::Event)]
    pub struct NewMessage {
        pub(crate) message: ChatMessage,
    }

    #[derive(Serialize, Clone, specta::Type, tauri_specta::Event)]
    pub struct MessageStatusChanged {
        pub(crate) message_id: uuid::Uuid,
        pub(crate) status: MessageStatus,
    }

    #[derive(Serialize, Clone, specta::Type, tauri_specta::Event)]
    pub struct MessageStream {
        pub(crate) message_id: uuid::Uuid,
        pub(crate) message_fragment: String,
    }
}

pub struct AppState {
    session_manager: chat::SessionManager,
    message_stream_cancel_tokens:
        Arc<Mutex<HashMap<uuid::Uuid, tokio_util::sync::CancellationToken>>>,
}

impl AppState {
    pub fn new() -> Self {
        Self {
            session_manager: chat::SessionManager::load().expect("Failed to load sessions"),
            message_stream_cancel_tokens: Arc::new(Mutex::new(HashMap::new())),
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
    let session = state.session_manager.get_session(session.id)?;
    let message_id = uuid::Uuid::now_v7();

    debug!(session_id = %session.id(), %message_id, %message, "Sending message");

    let cancel_token = tokio_util::sync::CancellationToken::new();
    state
        .message_stream_cancel_tokens
        .lock()
        .unwrap()
        .insert(message_id, cancel_token.clone());

    // Emit new user message event
    let user_message = messages::ChatMessage {
        id: message_id,
        role: chat::Role::User,
        content: message.clone(),
        status: messages::MessageStatus::Pending,
    };
    events::NewMessage {
        message: user_message,
    }
    .emit(&window)?;

    // Spawn streaming task
    tauri::async_runtime::spawn({
        let window = window.clone();
        let message = message.clone();

        async move {
            match session.chat_completion_stream(message, cancel_token).await {
                Ok(mut stream) => {
                    // Update status to Processing
                    let _ = events::MessageStatusChanged {
                        message_id,
                        status: messages::MessageStatus::Processing,
                    }
                    .emit(&window);

                    // Create assistant message and emit NewMessage event
                    let mut assistant_message_id: Option<uuid::Uuid> = None;

                    while let Some(result) = stream.next().await {
                        match result {
                            Ok(message_fragment) => {
                                let msg_id = if let Some(id) = &assistant_message_id {
                                    id.clone()
                                } else {
                                    // Create an assistant message to contain the response tokens
                                    let new_message = messages::ChatMessage {
                                        id: uuid::Uuid::now_v7(),
                                        role: chat::Role::Assistant,
                                        content: String::new(),
                                        status: messages::MessageStatus::Streaming,
                                    };

                                    let new_id = new_message.id;
                                    assistant_message_id = Some(new_id);

                                    let _ = events::NewMessage {
                                        message: new_message,
                                    }
                                    .emit(&window);
                                    new_id
                                };

                                let _ = events::MessageStream {
                                    message_id: msg_id,
                                    message_fragment,
                                }
                                .emit(&window);
                            }
                            Err(e) => {
                                // If we get an error while streaming, associate that error with
                                // the user's message, since that's the one that the user can
                                // retry.
                                let _ = events::MessageStatusChanged {
                                    message_id,
                                    status: messages::MessageStatus::Error {
                                        error: Arc::new(UiError::from(e)),
                                    },
                                }
                                .emit(&window);
                                if let Some(id) = assistant_message_id {
                                    // If there is any assistant message, it's not going to get any
                                    // more updates to mark it as complete.
                                    let _ = events::MessageStatusChanged {
                                        message_id: id,
                                        status: messages::MessageStatus::Complete,
                                    }
                                    .emit(&window);
                                }
                                break;
                            }
                        }
                    }

                    // Stream complete, update status
                    let _ = events::MessageStatusChanged {
                        message_id,
                        status: messages::MessageStatus::Complete,
                    }
                    .emit(&window);

                    if let Some(message_id) = assistant_message_id {
                        let _ = events::MessageStatusChanged {
                            message_id,
                            status: messages::MessageStatus::Complete,
                        }
                        .emit(&window);
                    }
                }
                Err(e) => {
                    let _ = events::MessageStatusChanged {
                        message_id,
                        status: messages::MessageStatus::Error {
                            error: Arc::new(UiError::from(e)),
                        },
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

#[tauri::command]
#[specta::specta]
pub async fn abort_message(
    state: tauri::State<'_, AppState>,
    message_id: uuid::Uuid,
) -> Result<()> {
    if let Some(cancel_token) = state
        .message_stream_cancel_tokens
        .lock()
        .unwrap()
        .get(&message_id)
    {
        cancel_token.cancel();
    }
    Ok(())
}

#[tauri::command]
#[specta::specta]
pub async fn retry_message(
    state: tauri::State<'_, AppState>,
    window: tauri::Window,
    session: messages::SessionHandle,
    original_message_id: uuid::Uuid,
    message: String,
) -> Result<uuid::Uuid> {
    // Reuse send_message logic but with a different message ID
    send_message(state, window, session, message).await
}
