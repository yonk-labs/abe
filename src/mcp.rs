//! MCP stdio server exposing `debate` and `validate` as tools so agents can
//! call llm-debator inline. Thin wrapper over the already-tested engines.

use crate::config::{Config, Protocol};
use crate::debate::run_debate;
use crate::provider::{build_provider, Provider};
use crate::validate::run_validate;
use anyhow::Context;
use rmcp::handler::server::router::tool::ToolRouter;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::{ServerCapabilities, ServerInfo};
use rmcp::transport::stdio;
use rmcp::{schemars, tool, tool_handler, tool_router, ServerHandler, ServiceExt};
use serde::Deserialize;
use std::path::PathBuf;

#[derive(Clone)]
pub struct DebatorServer {
    config_path: Option<String>,
    tool_router: ToolRouter<Self>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct DebateParams {
    /// The question/prompt to debate across all configured models.
    pub prompt: String,
    /// Number of debate rounds (overrides config).
    #[serde(default)]
    pub rounds: Option<u32>,
    /// Decision protocol: synthesis | majority | judge.
    #[serde(default)]
    pub protocol: Option<String>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct ValidateParams {
    /// The statement/decision/answer to validate.
    pub statement: String,
    /// Reviewer model name (defaults to validate.reviewers[0], else first model).
    #[serde(default)]
    pub reviewer: Option<String>,
    /// Optional extra context to include in the review.
    #[serde(default)]
    pub context: Option<String>,
}

#[tool_router]
impl DebatorServer {
    #[tool(
        description = "Run a multi-model debate; returns final answer + agreement/disagreement report as JSON."
    )]
    pub async fn debate(&self, Parameters(p): Parameters<DebateParams>) -> String {
        json_or_error(self.do_debate(p).await)
    }

    #[tool(
        description = "Get one model's independent second opinion on a statement/decision; returns JSON."
    )]
    pub async fn validate(&self, Parameters(p): Parameters<ValidateParams>) -> String {
        json_or_error(self.do_validate(p).await)
    }
}

impl DebatorServer {
    pub fn new(config_path: Option<String>) -> Self {
        Self {
            config_path,
            tool_router: Self::tool_router(),
        }
    }

    fn load(&self) -> anyhow::Result<Config> {
        let candidates: Vec<PathBuf> = match &self.config_path {
            Some(p) => vec![PathBuf::from(p)],
            None => {
                let mut v = vec![PathBuf::from("llm-debator.yaml")];
                if let Some(home) = std::env::var_os("HOME") {
                    v.push(PathBuf::from(home).join(".config/llm-debator/config.yaml"));
                }
                v
            }
        };
        for c in &candidates {
            if c.exists() {
                return Config::load(c);
            }
        }
        anyhow::bail!("no config found")
    }

    async fn do_debate(&self, p: DebateParams) -> anyhow::Result<String> {
        let mut cfg = self.load()?;
        if let Some(r) = p.rounds {
            cfg.debate.rounds = r;
        }
        if let Some(proto) = &p.protocol {
            cfg.debate.protocol = parse_proto(proto)?;
        }
        let providers: Vec<Box<dyn Provider>> = cfg
            .models
            .iter()
            .map(|m| build_provider(m, &cfg.defaults))
            .collect::<anyhow::Result<_>>()?;
        let chair = cfg.resolved_chairman().map(|s| s.to_string());
        let chairman: &dyn Provider = providers
            .iter()
            .find(|pr| Some(pr.name()) == chair.as_deref())
            .map(|b| b.as_ref())
            .context("chairman model not found")?;
        let res = run_debate(&cfg, &providers, chairman, &p.prompt).await?;
        Ok(serde_json::to_string(&res)?)
    }

    async fn do_validate(&self, p: ValidateParams) -> anyhow::Result<String> {
        let cfg = self.load()?;
        let name = p
            .reviewer
            .clone()
            .or_else(|| cfg.validate.reviewers.first().cloned())
            .or_else(|| cfg.models.first().map(|m| m.name.clone()))
            .context("no reviewer configured")?;
        let rcfg = cfg
            .models
            .iter()
            .find(|m| m.name == name)
            .context("reviewer not found")?;
        let reviewer = build_provider(rcfg, &cfg.defaults)?;
        let res = run_validate(reviewer.as_ref(), &p.statement, p.context.as_deref()).await?;
        Ok(serde_json::to_string(&res)?)
    }
}

#[tool_handler]
impl ServerHandler for DebatorServer {
    fn get_info(&self) -> ServerInfo {
        ServerInfo {
            instructions: Some(
                "llm-debator: run a multi-model debate or get a second-opinion validation.".into(),
            ),
            capabilities: ServerCapabilities::builder().enable_tools().build(),
            ..Default::default()
        }
    }
}

fn parse_proto(s: &str) -> anyhow::Result<Protocol> {
    match s.to_lowercase().as_str() {
        "synthesis" => Ok(Protocol::Synthesis),
        "majority" => Ok(Protocol::Majority),
        "judge" => Ok(Protocol::Judge),
        o => anyhow::bail!("unknown protocol `{o}`"),
    }
}

fn json_or_error(r: anyhow::Result<String>) -> String {
    match r {
        Ok(s) => s,
        Err(e) => format!(
            "{{\"error\":{}}}",
            serde_json::to_string(&e.to_string()).unwrap_or_else(|_| "\"error\"".into())
        ),
    }
}

/// Run the MCP server over stdio until shutdown.
pub async fn serve(config_path: Option<String>) -> anyhow::Result<()> {
    let server = DebatorServer::new(config_path);
    let service = server.serve(stdio()).await?;
    service.waiting().await?;
    Ok(())
}
