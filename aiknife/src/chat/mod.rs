use crate::Result;
use anyhow::Context;
use reqwest::Client;
use serde::{Deserialize, Serialize};
#[cfg(feature = "tauri")]
use specta::Type;
use std::collections::HashMap;
use std::env;
use std::sync::{Arc, Mutex};
use uuid::Uuid;

pub type SessionId = Uuid;

#[derive(Serialize)]
#[cfg_attr(feature = "tauri", derive(Type))]
struct ChatCompletionRequest {
    model: String,
    messages: Vec<Message>,
}

#[derive(Serialize, Deserialize, Clone)]
#[cfg_attr(feature = "tauri", derive(Type))]
pub struct Message {
    role: String,
    content: String,
}

impl Message {
    pub fn new_user_message(content: String) -> Self {
        Self {
            role: "user".to_string(),
            content,
        }
    }

    pub fn new_assistant_message(content: String) -> Self {
        Self {
            role: "assistant".to_string(),
            content,
        }
    }
}

#[derive(Deserialize)]
#[cfg_attr(feature = "tauri", derive(Type))]
struct ChatCompletionResponse {
    choices: Vec<Choice>,
}

#[derive(Deserialize)]
#[cfg_attr(feature = "tauri", derive(Type))]
struct Choice {
    message: Message,
}

#[derive(Clone)]
pub struct Session {
    id: Uuid,
    api_key: String,
    inner: Arc<Mutex<SessionInner>>,
}

struct SessionInner {
    messages: Vec<Message>,
}

impl Session {
    pub fn new() -> Result<Self> {
        Ok(Self {
            id: Uuid::now_v7(),
            api_key: env::var("OPENAI_API_KEY").context("OPENAI_API_KEY must be set")?,
            inner: Arc::new(Mutex::new(SessionInner {
                messages: Vec::new(),
            })),
        })
    }

    pub fn id(&self) -> Uuid {
        self.id
    }

    pub async fn chat_completion(&self, message: Message) -> Result<String> {
        let mut messages = self.inner.lock().unwrap().messages.clone();
        messages.push(message);

        let client = Client::new();
        let request = ChatCompletionRequest {
            model: "gpt-3.5-turbo".to_string(),
            messages: messages.clone(),
        };

        let response = client
            .post("https://api.openai.com/v1/chat/completions")
            .header("Authorization", format!("Bearer {}", &self.api_key))
            .json(&request)
            .send()
            .await
            .context("Failed to send request")?
            .json::<ChatCompletionResponse>()
            .await
            .context("Failed to parse JSON response")?;

        // Update the session with the message we just sent to chat, and also the response
        let mut inner = self.inner.lock().unwrap();
        inner.messages = messages;
        inner.messages.push(response.choices[0].message.clone());

        Ok(inner.messages.last().as_ref().unwrap().content.clone())
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
