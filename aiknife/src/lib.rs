mod error;
mod tokenize;

use anyhow::Context;
pub use error::Result;
pub use tokenize::*;

#[cfg(test)]
mod tests {
    use super::*;
}

use reqwest::Client;
use serde::{Deserialize, Serialize};
use specta::Type;
use std::env;

#[derive(Serialize, Type)]
struct ChatCompletionRequest {
    model: String,
    messages: Vec<Message>,
}

#[derive(Serialize, Deserialize, Clone, Type)]
pub struct Message {
    role: String,
    content: String,
}

#[derive(Deserialize, Type)]
struct ChatCompletionResponse {
    choices: Vec<Choice>,
}

#[derive(Deserialize, Type)]
struct Choice {
    message: Message,
}

pub fn check_api_key() -> Result<()> {
    env::var("OPENAI_API_KEY").context(
        "OpenAI API key is missing. Please set the OPENAI_API_KEY environment variable.",
    )?;
    Ok(())
}

pub async fn chat_completion(messages: Vec<Message>) -> Result<String> {
    let api_key = env::var("OPENAI_API_KEY").context("OPENAI_API_KEY must be set")?;
    let client = Client::new();
    let request = ChatCompletionRequest {
        model: "gpt-3.5-turbo".to_string(),
        messages,
    };

    let response = client
        .post("https://api.openai.com/v1/chat/completions")
        .header("Authorization", format!("Bearer {}", api_key))
        .json(&request)
        .send()
        .await
        .context("Failed to send request")?
        .json::<ChatCompletionResponse>()
        .await
        .context("Failed to parse JSON response")?;

    Ok(response.choices[0].message.content.clone())
}
