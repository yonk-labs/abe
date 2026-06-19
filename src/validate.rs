//! Quick-validate ("second opinion") mode: send a statement/decision to one
//! reviewer model and return its independent take. A lightweight cousin of the
//! full debate — ported from the `second-opinion` skill's prompt template.

use crate::config::Config;
use crate::provider::{build_provider, Prompt, Provider};
use serde::Serialize;

#[derive(Debug, Clone, Serialize)]
pub struct ValidateResult {
    pub reviewer: String,
    pub take: String,
    /// Set when the preferred reviewer was unavailable and we fell back to
    /// another model — names what was skipped and why.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

/// Send a statement/decision to one reviewer model for an independent take.
/// `prior_reasoning` is the host model's current take — supplying it turns this
/// from generic validation into a true second opinion on the host's reasoning.
pub async fn run_validate(
    reviewer: &dyn Provider,
    statement: &str,
    prior_reasoning: Option<&str>,
    context: Option<&str>,
) -> anyhow::Result<ValidateResult> {
    let prompt = Prompt {
        system: None,
        user: validate_prompt(statement, prior_reasoning, context),
        temperature: 0.7,
        max_tokens: 1024,
    };
    let ans = reviewer.complete(&prompt).await?;
    Ok(ValidateResult {
        reviewer: reviewer.name().to_string(),
        take: ans.text,
        note: None,
    })
}

/// Try reviewers in order; return the first that answers. A reviewer that errors
/// (missing key, unreachable endpoint, dead CLI) is skipped, not fatal — only an
/// all-down list bails. On fallback, the chosen result carries a `note` naming
/// what was skipped, so the degradation is visible rather than silent.
async fn first_reviewer_take(
    reviewers: &[Box<dyn Provider>],
    statement: &str,
    prior_reasoning: Option<&str>,
    context: Option<&str>,
) -> anyhow::Result<ValidateResult> {
    let mut skipped: Vec<String> = Vec::new();
    for (i, p) in reviewers.iter().enumerate() {
        match run_validate(p.as_ref(), statement, prior_reasoning, context).await {
            Ok(mut res) => {
                if i > 0 {
                    res.note = Some(format!("fell back to `{}` — skipped: {}", p.name(), skipped.join("; ")));
                }
                return Ok(res);
            }
            Err(e) => skipped.push(format!("`{}` unavailable: {e}", p.name())),
        }
    }
    anyhow::bail!("no reviewer could be reached:\n  - {}", skipped.join("\n  - "))
}

/// Resolve a reviewer from config (explicit name → validate.reviewers[0] →
/// first model), build it, and run a single-shot validation. Shared by CLI,
/// MCP, and HTTP surfaces.
pub async fn validate_from_config(
    cfg: &Config,
    statement: &str,
    reviewer: Option<&str>,
    prior_reasoning: Option<&str>,
    context: Option<&str>,
) -> anyhow::Result<ValidateResult> {
    // Preference order: explicit reviewer → configured reviewers → all models,
    // deduped. Building a provider is cheap and side-effect-free (no network),
    // so building the whole candidate list up front keeps the cascade simple.
    let mut candidates: Vec<Box<dyn Provider>> = Vec::new();
    let mut seen: Vec<String> = Vec::new();
    let mut skipped: Vec<String> = Vec::new();
    for name in reviewer
        .map(str::to_string)
        .into_iter()
        .chain(cfg.validate.reviewers.iter().cloned())
        .chain(cfg.models.iter().map(|m| m.name.clone()))
    {
        if name.is_empty() || seen.contains(&name) {
            continue;
        }
        seen.push(name.clone());
        match cfg.models.iter().find(|m| m.name == name) {
            Some(rcfg) => match build_provider(rcfg, &cfg.defaults) {
                Ok(p) => candidates.push(p),
                Err(e) => skipped.push(format!("`{name}` failed to initialize: {e}")),
            },
            None => skipped.push(format!("`{name}` is not a defined model")),
        }
    }
    if candidates.is_empty() {
        let why = if skipped.is_empty() {
            "no reviewer configured and no models defined".to_string()
        } else {
            format!("no usable reviewer: {}", skipped.join("; "))
        };
        anyhow::bail!(why);
    }
    first_reviewer_take(&candidates, statement, prior_reasoning, context).await
}

/// Mirrors the `second-opinion` skill's prompt: frame the reviewer as a non-host
/// counter-perspective, hard blank-slate clause (it sees only this prompt), and
/// the 4-point ask. `prior_reasoning` (host's take) and `context` are dropped
/// when absent, exactly as the skill drops empty sections.
fn validate_prompt(statement: &str, prior_reasoning: Option<&str>, context: Option<&str>) -> String {
    let prior = prior_reasoning
        .filter(|s| !s.trim().is_empty())
        .map(|p| format!("\n\n# Prior reasoning (the host model's current take)\n{p}"))
        .unwrap_or_default();
    let ctx = context
        .filter(|s| !s.trim().is_empty())
        .map(|c| format!("\n\n# Context\n{c}"))
        .unwrap_or_default();
    format!(
        "You are being consulted for a SECOND OPINION on a technical decision.\n\
Another AI assistant is the primary collaborator and has its own view.\n\
Your job: give a clear, opinionated, non-host perspective. Disagree freely if you see things differently.\n\n\
IMPORTANT: You have NO access to the codebase, the prior conversation, or any tool to inspect anything. \
Everything you know about this situation is in THIS prompt. If something critical to a sound answer is \
missing, state exactly what you'd need and give your best conditional answer — do NOT invent file \
contents, APIs, or behavior you cannot see.\n\n\
# The question\n{statement}{prior}{ctx}\n\n\
# What I want from you\n\
1. Your direct answer or recommendation.\n\
2. The single biggest risk or blind spot you see.\n\
3. One thing the host probably got right, and one thing it might be wrong about.\n\
4. If you'd approach this fundamentally differently, say so plainly.\n\
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
        let res = run_validate(&reviewer, "We should use Rust.", None, None)
            .await
            .unwrap();
        assert_eq!(res.reviewer, "codex");
        assert!(res.take.contains("watch concurrency"));
    }

    #[tokio::test]
    async fn validate_includes_statement_and_context() {
        let reviewer = MockProvider::new("codex", ["ok"]);
        let log = reviewer.log_handle();
        run_validate(&reviewer, "STMT-TEXT", None, Some("CTX-DATA"))
            .await
            .unwrap();
        let prompt = &log.lock().unwrap()[0];
        assert!(prompt.contains("STMT-TEXT"));
        assert!(prompt.contains("CTX-DATA"));
    }

    #[tokio::test]
    async fn validate_includes_prior_reasoning() {
        let reviewer = MockProvider::new("codex", ["ok"]);
        let log = reviewer.log_handle();
        run_validate(&reviewer, "Use Postgres", Some("HOST-TAKE-XYZ"), None)
            .await
            .unwrap();
        let prompt = &log.lock().unwrap()[0];
        assert!(prompt.contains("HOST-TAKE-XYZ"), "the host's take must reach the reviewer");
        assert!(prompt.contains("Prior reasoning"), "section header present when a take is given");
    }

    #[tokio::test]
    async fn cascades_past_a_dead_reviewer() {
        let reviewers: Vec<Box<dyn Provider>> = vec![
            Box::new(crate::provider::FailProvider::new("gpt")),
            Box::new(MockProvider::new("qwen", ["second opinion here"])),
        ];
        let res = first_reviewer_take(&reviewers, "ship it?", None, None).await.unwrap();
        assert_eq!(res.reviewer, "qwen", "should fall back to the live model");
        assert!(res.note.as_deref().unwrap_or_default().contains("gpt"), "note must name the skipped reviewer");
    }

    #[tokio::test]
    async fn no_note_when_first_reviewer_answers() {
        let reviewers: Vec<Box<dyn Provider>> = vec![
            Box::new(MockProvider::new("gpt", ["ok"])),
            Box::new(MockProvider::new("qwen", ["unused"])),
        ];
        let res = first_reviewer_take(&reviewers, "x", None, None).await.unwrap();
        assert_eq!(res.reviewer, "gpt");
        assert!(res.note.is_none(), "no fallback → no note");
    }

    #[tokio::test]
    async fn bails_only_when_all_reviewers_are_down() {
        let reviewers: Vec<Box<dyn Provider>> = vec![
            Box::new(crate::provider::FailProvider::new("gpt")),
            Box::new(crate::provider::FailProvider::new("qwen")),
        ];
        let err = first_reviewer_take(&reviewers, "ship it?", None, None).await.unwrap_err();
        assert!(err.to_string().contains("no reviewer could be reached"));
    }
}
