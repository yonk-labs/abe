//! MCP stdio server exposing `debate` and `validate` as tools so agents can
//! call abe inline. Thin wrapper over the already-tested engines.

use crate::config::Config;
use rmcp::handler::server::router::tool::ToolRouter;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::{ServerCapabilities, ServerInfo};
use rmcp::transport::stdio;
use rmcp::{schemars, tool, tool_handler, tool_router, ServerHandler, ServiceExt};
use serde::Deserialize;

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
        Config::load_default(self.config_path.as_deref())
    }

    async fn do_debate(&self, p: DebateParams) -> anyhow::Result<String> {
        let mut cfg = self.load()?;
        if let Some(r) = p.rounds {
            cfg.debate.rounds = r;
        }
        if let Some(proto) = &p.protocol {
            cfg.debate.protocol = crate::config::parse_protocol(proto)?;
        }
        let res = crate::debate::debate_from_config(&cfg, &p.prompt).await?;
        Ok(serde_json::to_string(&res)?)
    }

    async fn do_validate(&self, p: ValidateParams) -> anyhow::Result<String> {
        let cfg = self.load()?;
        let res = crate::validate::validate_from_config(
            &cfg,
            &p.statement,
            p.reviewer.as_deref(),
            p.context.as_deref(),
        )
        .await?;
        Ok(serde_json::to_string(&res)?)
    }
}

#[tool_handler]
impl ServerHandler for DebatorServer {
    fn get_info(&self) -> ServerInfo {
        ServerInfo {
            instructions: Some(
                "abe: run a multi-model debate or get a second-opinion validation.".into(),
            ),
            capabilities: ServerCapabilities::builder().enable_tools().build(),
            ..Default::default()
        }
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
