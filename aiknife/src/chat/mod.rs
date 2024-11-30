use crate::Result;
use anyhow::Context;
use async_openai as oai;
use futures::{Stream, StreamExt};
use serde::{Deserialize, Serialize};
#[cfg(feature = "tauri")]
use specta::Type;
use std::collections::HashMap;
use std::env;
use std::sync::{Arc, Mutex};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
use tracing::*;
use uuid::Uuid;

pub type SessionId = Uuid;

#[derive(Serialize, Deserialize, Clone)]
#[cfg_attr(feature = "tauri", derive(Type))]
#[serde(rename_all = "lowercase")]
pub enum Role {
    User,
    Assistant,
}

impl Into<oai::types::Role> for Role {
    fn into(self) -> oai::types::Role {
        match self {
            Role::User => oai::types::Role::User,
            Role::Assistant => oai::types::Role::Assistant,
        }
    }
}

#[derive(Serialize, Deserialize, Clone)]
#[cfg_attr(feature = "tauri", derive(Type))]
pub struct Message {
    role: Role,
    content: String,
}

impl Message {
    pub fn new_user_message(content: String) -> Self {
        Self {
            role: Role::User,
            content,
        }
    }

    pub fn new_assistant_message(content: String) -> Self {
        Self {
            role: Role::Assistant,
            content,
        }
    }

    fn to_oai_chat_completion_message(&self) -> oai::types::ChatCompletionRequestMessage {
        match self.role {
            Role::User => oai::types::ChatCompletionRequestMessage::User(
                oai::types::ChatCompletionRequestUserMessage {
                    content: oai::types::ChatCompletionRequestUserMessageContent::Text(
                        self.content.clone(),
                    ),
                    ..Default::default()
                },
            ),
            Role::Assistant => oai::types::ChatCompletionRequestMessage::Assistant(
                oai::types::ChatCompletionRequestAssistantMessage {
                    content: Some(
                        oai::types::ChatCompletionRequestAssistantMessageContent::Text(
                            self.content.clone(),
                        ),
                    ),
                    ..Default::default()
                },
            ),
        }
    }
}

#[derive(Clone)]
pub struct Session {
    id: Uuid,
    client: oai::Client<oai::config::OpenAIConfig>,
    inner: Arc<Mutex<SessionInner>>,
}

struct SessionInner {
    messages: Vec<Message>,
}

impl Session {
    pub fn new() -> Result<Self> {
        let api_key = env::var("OPENAI_API_KEY").context("OPENAI_API_KEY must be set")?;

        let config = oai::config::OpenAIConfig::new().with_api_key(api_key);

        Ok(Self {
            id: Uuid::now_v7(),
            client: oai::Client::with_config(config),
            inner: Arc::new(Mutex::new(SessionInner {
                messages: Vec::new(),
            })),
        })
    }

    pub fn id(&self) -> Uuid {
        self.id
    }

    pub async fn chat_completion_stream(
        &self,
        message: String,
        cancel_token: CancellationToken,
    ) -> Result<impl Stream<Item = Result<String>>> {
        let (tx, rx) = mpsc::channel(32);
        let request = {
            let mut guard = self.inner.lock().unwrap();
            let messages = &mut guard.messages;

            messages.push(Message::new_user_message(message.clone()));

            oai::types::CreateChatCompletionRequestArgs::default()
                .model("gpt-3.5-turbo")
                .messages(
                    messages
                        .iter()
                        .map(|m| m.to_oai_chat_completion_message())
                        .collect::<Vec<_>>(),
                )
                .stream(true)
                .build()?
        };

        debug!(session_id = %self.id, %message, "Starting chat stream");

        let mut stream = self.client.chat().create_stream(request).await?;

        tokio::spawn({
            let me = self.clone();

            async move {
                // Put an initial empty assistant message in the session; we'll gradually fill it
                // with the assistant's responses as they come in
                let response_message_index = {
                    let mut guard = me.inner.lock().unwrap();
                    let response_message_index = guard.messages.len();
                    guard
                        .messages
                        .push(Message::new_assistant_message("".to_string()));
                    response_message_index
                };

                debug!(response_message_index, "Created new empty response message");

                loop {
                    tokio::select! {
                        // Check for cancellation
                        _ = cancel_token.cancelled() => {
                            debug!("Chat aborted by user request");
                            let _ = tx.send(Err(anyhow::anyhow!("Chat aborted by user request"))).await;
                            break;
                        }
                        // Process next stream item
                        next = stream.next() => {
                            match next {
                                Some(result) => {
                                    match result {
                                        Ok(response) => {
                                            debug!(?response, "Received chat stream response");
                                            if let Some(content) = &response.choices[0].delta.content {
                                                // Update session with assistant message, and also push it to the
                                                // requestor's stream
                                                {
                                                    let mut guard = me.inner.lock().unwrap();
                                                    guard.messages[response_message_index]
                                                        .content
                                                        .push_str(content);
                                                }
                                                if tx.send(Ok(content.clone())).await.is_err() {
                                                    // Send fails only when the receiver is
                                                    // dropped.  If the receiver is dropped then no
                                                    // one is listening for completions, so we
                                                    // should abort.
                                                    warn!("Chat completion receiver dropped; aborting chat stream");
                                                    break;
                                                }
                                            } else {
                                                // Content is None.  This seems likely to be at the
                                                // end of the response, but the async-openai
                                                // example doesn't conclude until the response
                                                // stream itself returns None.
                                                debug!("Received content value of None; ignoring");
                                            }
                                        }
                                        Err(e) => {
                                            error!("Stream error: {}", e);
                                            let _ = tx.send(Err(e.into())).await;
                                            break;
                                        }
                                    }
                                }
                                None => break,
                            }
                        }
                    }
                }

                debug!("Chat stream ended");
            }
        });

        Ok(tokio_stream::wrappers::ReceiverStream::new(rx))
    }
}

#[derive(Clone)]
pub struct SessionManager {
    sessions: Arc<Mutex<HashMap<SessionId, Session>>>,
}

impl SessionManager {
    // TODO: Implement session persistence
    pub fn load() -> Result<Self> {
        Ok(Self {
            sessions: Arc::new(Mutex::new(HashMap::new())),
        })
    }

    pub fn session_ids(&self) -> Vec<SessionId> {
        self.sessions.lock().unwrap().keys().cloned().collect()
    }

    pub fn new_session(&self) -> Result<Session> {
        let mut sessions = self.sessions.lock().unwrap();
        let session = Session::new()?;
        sessions.insert(session.id, session.clone());
        Ok(session)
    }

    pub fn get_session(&self, id: SessionId) -> Result<Session> {
        self.sessions
            .lock()
            .unwrap()
            .get(&id)
            .cloned()
            .context("Session not found")
    }
}

pub fn check_api_key() -> Result<()> {
    env::var("OPENAI_API_KEY").context(
        "OpenAI API key is missing. Please set the OPENAI_API_KEY environment variable.",
    )?;
    Ok(())
}
