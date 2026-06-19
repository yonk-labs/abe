//! HTTP provider via the `genai` crate (OpenAI, Anthropic, OpenAI-compatible,
//! local). `genai` types never leak past this module — the rest of the crate
//! sees only `Provider`/`Prompt`/`Answer`.

use crate::config::{Defaults, ModelCfg, ModelKind};
use crate::provider::{Answer, Prompt, Provider, ProviderError};
use anyhow::Context;
use async_trait::async_trait;
use genai::adapter::AdapterKind;
use genai::chat::{ChatMessage, ChatRequest};
use genai::resolver::{AuthData, Endpoint, ServiceTargetResolver};
use genai::{Client, ModelIden, ServiceTarget};
use std::time::Instant;

pub struct HttpProvider {
    name: String,
    model: String,
    client: Client,
}

impl HttpProvider {
    pub fn new(cfg: &ModelCfg, _defaults: &Defaults) -> anyhow::Result<Self> {
        let model = cfg
            .model
            .clone()
            .with_context(|| format!("http model `{}` requires a `model` field", cfg.name))?;
        let client = build_client(cfg)?;
        Ok(HttpProvider {
            name: cfg.name.clone(),
            model,
            client,
        })
    }
}

/// Build a genai client. Standard providers use env-based auth defaults; a
/// custom `base_url` or `api_key_env` (or kind=openai-compatible) installs a
/// ServiceTargetResolver to override endpoint/auth/adapter.
fn build_client(cfg: &ModelCfg) -> anyhow::Result<Client> {
    let needs_resolver = matches!(cfg.kind, ModelKind::OpenaiCompatible)
        || cfg.base_url.is_some()
        || cfg.api_key_env.is_some();
    if !needs_resolver {
        return Ok(Client::default());
    }

    let adapter = match cfg.kind {
        ModelKind::Anthropic => AdapterKind::Anthropic,
        _ => AdapterKind::OpenAI,
    };
    let base_url = cfg.base_url.clone();
    let api_key_env = cfg.api_key_env.clone();

    let resolver = ServiceTargetResolver::from_resolver_fn(
        move |st: ServiceTarget| -> Result<ServiceTarget, genai::resolver::Error> {
            let ServiceTarget { endpoint, auth, model } = st;
            let endpoint = match &base_url {
                Some(u) => endpoint_from(u),
                None => endpoint,
            };
            let auth = match &api_key_env {
                // ponytail: leak the env-var name to satisfy from_static-style
                // 'static bound; one tiny one-time leak per client build.
                Some(name) => AuthData::from_env(Box::leak(name.clone().into_boxed_str())),
                None => auth,
            };
            let model = ModelIden::new(adapter, model.model_name);
            Ok(ServiceTarget { endpoint, auth, model })
        },
    );

    Ok(Client::builder()
        .with_service_target_resolver(resolver)
        .build())
}

fn endpoint_from(base_url: &str) -> Endpoint {
    let mut u = base_url.to_string();
    if !u.ends_with('/') {
        u.push('/');
    }
    // ponytail: Endpoint::from_static needs 'static; leak the config URL once.
    Endpoint::from_static(Box::leak(u.into_boxed_str()))
}

#[async_trait]
impl Provider for HttpProvider {
    fn name(&self) -> &str {
        &self.name
    }

    async fn complete(&self, prompt: &Prompt) -> Result<Answer, ProviderError> {
        let mut msgs = Vec::new();
        if let Some(s) = &prompt.system {
            msgs.push(ChatMessage::system(s.clone()));
        }
        msgs.push(ChatMessage::user(prompt.user.clone()));
        let req = ChatRequest::new(msgs);

        let start = Instant::now();
        let res = self
            .client
            .exec_chat(&self.model, req, None)
            .await
            .map_err(|e| ProviderError::Backend {
                name: self.name.clone(),
                source: anyhow::anyhow!(e.to_string()),
            })?;
        let elapsed_ms = start.elapsed().as_millis() as u64;
        let text = res
            .first_text()
            .map(|s| s.to_string())
            .unwrap_or_default();
        Ok(Answer {
            model_name: self.name.clone(),
            text,
            usage: None,
            elapsed_ms,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;
    use crate::provider::Provider;

    #[test]
    fn builds_for_openai() {
        let c = Config::from_yaml(
            "models: [{name: gpt, kind: openai, model: gpt-5.1, api_key_env: OPENAI_API_KEY}]",
        )
        .unwrap();
        let p = HttpProvider::new(&c.models[0], &c.defaults).unwrap();
        assert_eq!(p.name(), "gpt");
    }

    #[test]
    fn builds_for_openai_compatible_with_base_url() {
        let c = Config::from_yaml(
            "models: [{name: local, kind: openai-compatible, model: q, base_url: \"http://localhost:11434/v1\"}]",
        )
        .unwrap();
        assert!(HttpProvider::new(&c.models[0], &c.defaults).is_ok());
    }
}
