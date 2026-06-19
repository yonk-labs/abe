//! Provider abstraction: one trait, two transports (HTTP via genai, CLI subprocess).
//! Keep this trait small — never leak `genai` types upward.

use async_trait::async_trait;

pub mod cli;

/// A single request to a model.
#[derive(Debug, Clone)]
pub struct Prompt {
    pub system: Option<String>,
    pub user: String,
    pub temperature: f32,
    pub max_tokens: u32,
    /// Request structured (JSON) output where the backend supports it.
    pub json_mode: bool,
}

impl Prompt {
    /// Convenience constructor with sane defaults.
    pub fn user(text: impl Into<String>) -> Self {
        Prompt {
            system: None,
            user: text.into(),
            temperature: 0.7,
            max_tokens: 1024,
            json_mode: false,
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct Usage {
    pub input_tokens: u64,
    pub output_tokens: u64,
}

/// A model's reply.
#[derive(Debug, Clone)]
pub struct Answer {
    pub model_name: String,
    pub text: String,
    pub usage: Option<Usage>,
    pub elapsed_ms: u64,
}

#[derive(Debug, thiserror::Error)]
pub enum ProviderError {
    #[error("provider {name} timed out after {ms}ms")]
    Timeout { name: String, ms: u64 },
    #[error("provider {name} failed: {source}")]
    Backend {
        name: String,
        #[source]
        source: anyhow::Error,
    },
}

/// The core seam. HTTP and CLI providers both implement this so the debate
/// engine never branches on transport.
#[async_trait]
pub trait Provider: Send + Sync {
    fn name(&self) -> &str;
    async fn complete(&self, prompt: &Prompt) -> Result<Answer, ProviderError>;
}

/// Deterministic provider for tests: returns scripted answers in order
/// (repeats the last once exhausted).
#[cfg(test)]
pub struct MockProvider {
    name: String,
    answers: Vec<String>,
    idx: std::sync::atomic::AtomicUsize,
    log: std::sync::Arc<std::sync::Mutex<Vec<String>>>,
}

#[cfg(test)]
impl MockProvider {
    pub fn new<I, S>(name: &str, answers: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        MockProvider {
            name: name.to_string(),
            answers: answers.into_iter().map(Into::into).collect(),
            idx: std::sync::atomic::AtomicUsize::new(0),
            log: std::sync::Arc::new(std::sync::Mutex::new(Vec::new())),
        }
    }

    /// Shared handle to the user-prompt text this provider has received, in order.
    pub fn log_handle(&self) -> std::sync::Arc<std::sync::Mutex<Vec<String>>> {
        self.log.clone()
    }
}

#[cfg(test)]
#[async_trait]
impl Provider for MockProvider {
    fn name(&self) -> &str {
        &self.name
    }
    async fn complete(&self, prompt: &Prompt) -> Result<Answer, ProviderError> {
        use std::sync::atomic::Ordering;
        self.log.lock().unwrap().push(prompt.user.clone());
        let i = self.idx.fetch_add(1, Ordering::SeqCst);
        let text = self
            .answers
            .get(i)
            .or_else(|| self.answers.last())
            .cloned()
            .unwrap_or_default();
        Ok(Answer {
            model_name: self.name.clone(),
            text,
            usage: None,
            elapsed_ms: 0,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn mock_returns_canned() {
        let p = MockProvider::new("m1", ["hello"]);
        let a = p.complete(&Prompt::user("hi")).await.unwrap();
        assert_eq!(a.text, "hello");
        assert_eq!(a.model_name, "m1");
    }

    #[tokio::test]
    async fn mock_advances_per_call() {
        let p = MockProvider::new("m1", ["round0", "round1"]);
        assert_eq!(p.complete(&Prompt::user("x")).await.unwrap().text, "round0");
        assert_eq!(p.complete(&Prompt::user("x")).await.unwrap().text, "round1");
    }
}
