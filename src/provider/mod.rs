//! Provider abstraction: one trait, two transports (HTTP via genai, CLI subprocess).
//! Keep this trait small — never leak `genai` types upward.

use async_trait::async_trait;

pub mod cli;
pub mod http;

/// A single request to a model.
#[derive(Debug, Clone)]
pub struct Prompt {
    pub system: Option<String>,
    pub user: String,
    pub temperature: f32,
    pub max_tokens: u32,
}

#[cfg(test)]
impl Prompt {
    /// Convenience constructor with sane defaults (test helper).
    pub fn user(text: impl Into<String>) -> Self {
        Prompt {
            system: None,
            user: text.into(),
            temperature: 0.7,
            max_tokens: 1024,
        }
    }
}

/// A model's reply.
#[derive(Debug, Clone)]
pub struct Answer {
    pub model_name: String,
    pub text: String,
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

/// Build a live provider from config: CLI subprocess or HTTP (genai).
pub fn build_provider(
    cfg: &crate::config::ModelCfg,
    defaults: &crate::config::Defaults,
) -> anyhow::Result<Box<dyn Provider>> {
    use anyhow::Context;
    use crate::config::ModelKind;
    match cfg.kind {
        ModelKind::Cli => {
            let cli = cfg
                .cli
                .with_context(|| format!("model `{}` has kind=cli but no `cli`", cfg.name))?;
            Ok(Box::new(cli::CliProvider::new(
                &cfg.name,
                cli,
                cfg.model.clone(),
                cfg.fast,
                cfg.extra_args.clone(),
                defaults.timeout_secs,
            )))
        }
        _ => Ok(Box::new(http::HttpProvider::new(cfg, defaults)?)),
    }
}

/// Run `f` up to `attempts` times (minimum 1) with exponential backoff between
/// tries. Returns the first success, or the last error if all attempts fail.
pub async fn retry_async<T, E, F, Fut>(attempts: u32, mut f: F) -> Result<T, E>
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = Result<T, E>>,
{
    let attempts = attempts.max(1);
    let mut last: Option<E> = None;
    for i in 0..attempts {
        match f().await {
            Ok(v) => return Ok(v),
            Err(e) => {
                last = Some(e);
                if i + 1 < attempts {
                    tokio::time::sleep(std::time::Duration::from_millis(50u64 << i)).await;
                }
            }
        }
    }
    Err(last.expect("attempts >= 1 guarantees an error on failure"))
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
            elapsed_ms: 0,
        })
    }
}

/// Always-failing provider for fault-tolerance tests.
#[cfg(test)]
pub struct FailProvider {
    name: String,
}

#[cfg(test)]
impl FailProvider {
    pub fn new(name: &str) -> Self {
        FailProvider {
            name: name.to_string(),
        }
    }
}

#[cfg(test)]
#[async_trait]
impl Provider for FailProvider {
    fn name(&self) -> &str {
        &self.name
    }
    async fn complete(&self, _prompt: &Prompt) -> Result<Answer, ProviderError> {
        Err(ProviderError::Backend {
            name: self.name.clone(),
            source: anyhow::anyhow!("simulated failure"),
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

    #[test]
    fn factory_builds_cli_and_http() {
        use crate::config::Config;
        let c = Config::from_yaml(
            "models: [{name: cx, kind: cli, cli: codex}, {name: gpt, kind: openai, model: gpt-5.1}]",
        )
        .unwrap();
        assert_eq!(build_provider(&c.models[0], &c.defaults).unwrap().name(), "cx");
        assert_eq!(build_provider(&c.models[1], &c.defaults).unwrap().name(), "gpt");
    }

    #[tokio::test]
    async fn retry_succeeds_after_failures() {
        use std::sync::atomic::{AtomicU32, Ordering};
        let calls = AtomicU32::new(0);
        let r: Result<u32, &str> = retry_async(3, || {
            let n = calls.fetch_add(1, Ordering::SeqCst);
            async move {
                if n < 2 {
                    Err("transient")
                } else {
                    Ok(n)
                }
            }
        })
        .await;
        assert_eq!(r, Ok(2));
    }

    #[tokio::test]
    async fn retry_gives_up_after_attempts() {
        let r: Result<u32, &str> = retry_async(2, || async { Err::<u32, &str>("nope") }).await;
        assert_eq!(r, Err("nope"));
    }
}
