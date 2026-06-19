//! Quick-validate ("second opinion") mode: send a statement/decision to one
//! reviewer model and return its independent take. A lightweight cousin of the
//! full debate — ported from the `second-opinion` skill's prompt template.

use crate::provider::{Prompt, Provider};
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
