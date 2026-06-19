//! Quick-validate ("second opinion") mode: send a statement/decision to one
//! reviewer model and return its independent take. A lightweight cousin of the
//! full debate — ported from the `second-opinion` skill's prompt template.

use crate::config::Config;
use crate::provider::{build_provider, Prompt, Provider};
use anyhow::Context;
use serde::Serialize;

#[derive(Debug, Clone, Serialize)]
pub struct ValidateResult {
    pub reviewer: String,
    pub take: String,
}

/// Send a statement/decision to one reviewer model for an independent take.
pub async fn run_validate(
    reviewer: &dyn Provider,
    statement: &str,
    context: Option<&str>,
) -> anyhow::Result<ValidateResult> {
    let prompt = Prompt {
        system: None,
        user: validate_prompt(statement, context),
        temperature: 0.7,
        max_tokens: 1024,
    };
    let ans = reviewer.complete(&prompt).await?;
    Ok(ValidateResult {
        reviewer: reviewer.name().to_string(),
        take: ans.text,
    })
}

/// Resolve a reviewer from config (explicit name → validate.reviewers[0] →
/// first model), build it, and run a single-shot validation. Shared by CLI,
/// MCP, and HTTP surfaces.
pub async fn validate_from_config(
    cfg: &Config,
    statement: &str,
    reviewer: Option<&str>,
    context: Option<&str>,
) -> anyhow::Result<ValidateResult> {
    let name = reviewer
        .map(|s| s.to_string())
        .or_else(|| cfg.validate.reviewers.first().cloned())
        .or_else(|| cfg.models.first().map(|m| m.name.clone()))
        .context("no reviewer configured and no models defined")?;
    let rcfg = cfg
        .models
        .iter()
        .find(|m| m.name == name)
        .with_context(|| format!("reviewer `{name}` is not a defined model"))?;
    let provider = build_provider(rcfg, &cfg.defaults)?;
    run_validate(provider.as_ref(), statement, context).await
}

fn validate_prompt(statement: &str, context: Option<&str>) -> String {
    let ctx = context
        .map(|c| format!("\n\n# Context\n{c}"))
        .unwrap_or_default();
    format!(
        "You are being consulted for a SECOND OPINION / quick validation.\n\
Give a clear, opinionated, INDEPENDENT perspective. Disagree freely if you see things differently. \
Reason from first principles; you have only what is written here.\n\n\
# What to validate\n{statement}{ctx}\n\n\
# What I want from you\n\
1. Your direct verdict (is it sound? do you agree or disagree?).\n\
2. The single biggest risk or blind spot.\n\
3. One thing it gets right, and one thing it might get wrong.\n\
Be concise. Skip preamble."
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::provider::MockProvider;

    #[tokio::test]
    async fn validate_returns_reviewer_take() {
        let reviewer = MockProvider::new("codex", ["Looks sound, but watch concurrency."]);
        let res = run_validate(&reviewer, "We should use Rust.", None)
            .await
            .unwrap();
        assert_eq!(res.reviewer, "codex");
        assert!(res.take.contains("watch concurrency"));
    }

    #[tokio::test]
    async fn validate_includes_statement_and_context() {
        let reviewer = MockProvider::new("codex", ["ok"]);
        let log = reviewer.log_handle();
        run_validate(&reviewer, "STMT-TEXT", Some("CTX-DATA"))
            .await
            .unwrap();
        let prompt = &log.lock().unwrap()[0];
        assert!(prompt.contains("STMT-TEXT"));
        assert!(prompt.contains("CTX-DATA"));
    }
}
