//! CLI subprocess provider (codex / claude / opencode).
//!
//! Each CLI is "prompt in → text out" as a read-only subprocess. Invocation
//! recipes (stdin handling, sandbox flags, output shapes) are ported from the
//! battle-tested `second-opinion` skill. CLI providers are best-effort: hard
//! timeout, no retries (the engine handles HTTP retries instead).

use crate::config::CliKind;
use crate::provider::{Answer, Prompt, Provider, ProviderError};
use anyhow::Context;
use async_trait::async_trait;
use std::process::Stdio;
use std::time::{Duration, Instant};
use tokio::io::AsyncWriteExt;
use tokio::process::Command;

const INDEP: &str =
    "You are giving an INDEPENDENT answer. Ignore any local project conventions; reason from first principles.";

pub struct CliProvider {
    name: String,
    cli: CliKind,
    model: Option<String>,
    fast: bool,
    extra_args: Vec<String>,
    timeout_secs: u64,
}

impl CliProvider {
    pub fn new(
        name: &str,
        cli: CliKind,
        model: Option<String>,
        fast: bool,
        extra_args: Vec<String>,
        timeout_secs: u64,
    ) -> Self {
        CliProvider {
            name: name.to_string(),
            cli,
            model,
            fast,
            extra_args,
            timeout_secs,
        }
    }

    async fn run(&self, text: &str) -> anyhow::Result<String> {
        match self.cli {
            CliKind::Codex => self.run_codex(text).await,
            CliKind::Claude => self.run_claude(text).await,
            CliKind::Opencode => self.run_opencode(text).await,
        }
    }

    async fn run_codex(&self, text: &str) -> anyhow::Result<String> {
        let outfile = temp_path("codex-out");
        let args = codex_args(&self.model, self.fast, &outfile, &self.extra_args);
        let mut child = Command::new("codex")
            .args(&args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .context("spawning codex (is it on PATH?)")?;
        if let Some(mut sin) = child.stdin.take() {
            sin.write_all(text.as_bytes()).await?;
            sin.shutdown().await?;
        }
        let output = child.wait_with_output().await?;
        let answer = tokio::fs::read_to_string(&outfile).await.unwrap_or_default();
        let _ = tokio::fs::remove_file(&outfile).await;
        if !answer.trim().is_empty() {
            return Ok(answer.trim().to_string());
        }
        if !output.status.success() {
            anyhow::bail!(
                "codex failed: {}",
                String::from_utf8_lossy(&output.stderr).trim()
            );
        }
        Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
    }

    async fn run_claude(&self, text: &str) -> anyhow::Result<String> {
        let args = claude_args(&self.model, self.fast, text, &self.extra_args);
        let cwd = temp_path("claude-cwd");
        tokio::fs::create_dir_all(&cwd).await.ok();
        let output = Command::new("claude")
            .args(&args)
            .current_dir(&cwd)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .context("spawning claude (is it on PATH?)")?
            .wait_with_output()
            .await?;
        let _ = tokio::fs::remove_dir_all(&cwd).await;
        let stdout = String::from_utf8_lossy(&output.stdout);
        if let Some(r) = parse_claude_result(&stdout) {
            return Ok(r);
        }
        if !output.status.success() {
            anyhow::bail!(
                "claude failed: {}",
                String::from_utf8_lossy(&output.stderr).trim()
            );
        }
        Ok(stdout.trim().to_string())
    }

    async fn run_opencode(&self, text: &str) -> anyhow::Result<String> {
        let model = self
            .model
            .clone()
            .ok_or_else(|| anyhow::anyhow!("opencode requires a `model` (provider/model)"))?;
        let args = opencode_args(&Some(model), text, &self.extra_args);
        let output = Command::new("opencode")
            .args(&args)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .context("spawning opencode (is it on PATH?)")?
            .wait_with_output()
            .await?;
        let stdout = String::from_utf8_lossy(&output.stdout);
        let parsed = parse_opencode_text(&stdout);
        if !parsed.trim().is_empty() {
            return Ok(parsed.trim().to_string());
        }
        if !output.status.success() {
            anyhow::bail!(
                "opencode failed: {}",
                String::from_utf8_lossy(&output.stderr).trim()
            );
        }
        Ok(stdout.trim().to_string())
    }
}

fn compose(prompt: &Prompt) -> String {
    match &prompt.system {
        Some(s) => format!("{s}\n\n{}", prompt.user),
        None => prompt.user.clone(),
    }
}

fn temp_path(tag: &str) -> String {
    use std::sync::atomic::{AtomicU64, Ordering};
    static C: AtomicU64 = AtomicU64::new(0);
    let n = C.fetch_add(1, Ordering::Relaxed);
    std::env::temp_dir()
        .join(format!("llm-debator-{}-{}-{}", tag, std::process::id(), n))
        .to_string_lossy()
        .into_owned()
}

fn codex_args(model: &Option<String>, fast: bool, outfile: &str, extra: &[String]) -> Vec<String> {
    let mut a: Vec<String> = [
        "exec",
        "-s",
        "read-only",
        "--skip-git-repo-check",
        "--ephemeral",
        "--color",
        "never",
        "-o",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect();
    a.push(outfile.to_string());
    if let Some(m) = model {
        a.push("-m".into());
        a.push(m.clone());
    }
    if fast {
        a.push("-c".into());
        a.push("model_reasoning_effort=low".into());
    }
    a.extend(extra.iter().cloned());
    a.push("-".into());
    a
}

fn claude_args(model: &Option<String>, fast: bool, prompt: &str, extra: &[String]) -> Vec<String> {
    let mut a: Vec<String> = vec![
        "-p".into(),
        prompt.into(),
        "--permission-mode".into(),
        "plan".into(),
        "--append-system-prompt".into(),
        INDEP.into(),
        "--output-format".into(),
        "json".into(),
    ];
    let m = model
        .clone()
        .or_else(|| if fast { Some("sonnet".into()) } else { None });
    if let Some(m) = m {
        a.push("--model".into());
        a.push(m);
    }
    a.extend(extra.iter().cloned());
    a
}

fn opencode_args(model: &Option<String>, prompt: &str, extra: &[String]) -> Vec<String> {
    let mut a: Vec<String> = vec!["run".into(), prompt.into(), "--format".into(), "json".into()];
    if let Some(m) = model {
        a.push("-m".into());
        a.push(m.clone());
    }
    a.extend(extra.iter().cloned());
    a
}

fn parse_claude_result(stdout: &str) -> Option<String> {
    let v: serde_json::Value = serde_json::from_str(stdout.trim()).ok()?;
    v.get("result")
        .and_then(|r| r.as_str())
        .map(|s| s.to_string())
}

fn parse_opencode_text(stdout: &str) -> String {
    let mut out = String::new();
    for line in stdout.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        if let Ok(v) = serde_json::from_str::<serde_json::Value>(line) {
            if v.get("type").and_then(|t| t.as_str()) == Some("text") {
                if let Some(t) = v
                    .get("part")
                    .and_then(|p| p.get("text"))
                    .and_then(|t| t.as_str())
                {
                    out.push_str(t);
                }
            }
        }
    }
    out
}

#[async_trait]
impl Provider for CliProvider {
    fn name(&self) -> &str {
        &self.name
    }

    async fn complete(&self, prompt: &Prompt) -> Result<Answer, ProviderError> {
        let text = compose(prompt);
        let start = Instant::now();
        let res =
            tokio::time::timeout(Duration::from_secs(self.timeout_secs), self.run(&text)).await;
        let elapsed_ms = start.elapsed().as_millis() as u64;
        match res {
            Err(_) => Err(ProviderError::Timeout {
                name: self.name.clone(),
                ms: self.timeout_secs * 1000,
            }),
            Ok(Err(e)) => Err(ProviderError::Backend {
                name: self.name.clone(),
                source: e,
            }),
            Ok(Ok(text)) => Ok(Answer {
                model_name: self.name.clone(),
                text,
                elapsed_ms,
            }),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::CliKind;
    use crate::provider::Provider;

    #[test]
    fn parse_claude_extracts_result() {
        assert_eq!(
            parse_claude_result(r#"{"result":"hello","cost":1}"#).as_deref(),
            Some("hello")
        );
    }

    #[test]
    fn parse_claude_none_on_non_json() {
        assert!(parse_claude_result("not json").is_none());
    }

    #[test]
    fn parse_opencode_concats_text_parts() {
        let jsonl = "{\"type\":\"text\",\"part\":{\"text\":\"foo \"}}\n\
                     {\"type\":\"step-start\",\"part\":{}}\n\
                     {\"type\":\"text\",\"part\":{\"text\":\"bar\"}}";
        assert_eq!(parse_opencode_text(jsonl), "foo bar");
    }

    #[test]
    fn codex_args_are_read_only() {
        let a = codex_args(&None, false, "/tmp/out.txt", &[]);
        assert!(a.iter().any(|x| x == "read-only"));
        assert!(a.iter().any(|x| x == "--ephemeral"));
        assert_eq!(a.last().unwrap(), "-");
        assert!(a.iter().any(|x| x == "/tmp/out.txt"));
    }

    #[test]
    fn claude_args_plan_mode_json_and_fast_model() {
        let a = claude_args(&None, true, "PROMPT", &[]);
        assert!(a.windows(2).any(|w| w[0] == "--permission-mode" && w[1] == "plan"));
        assert!(a.windows(2).any(|w| w[0] == "--output-format" && w[1] == "json"));
        assert!(a.windows(2).any(|w| w[0] == "--model" && w[1] == "sonnet"));
        assert!(a.contains(&"PROMPT".to_string()));
    }

    #[test]
    fn opencode_args_include_model() {
        let a = opencode_args(&Some("ollama/x".into()), "P", &[]);
        assert!(a.windows(2).any(|w| w[0] == "-m" && w[1] == "ollama/x"));
        assert!(a.windows(2).any(|w| w[0] == "--format" && w[1] == "json"));
    }

    #[test]
    fn cli_provider_constructs() {
        let p = CliProvider::new("codex-cli", CliKind::Codex, None, false, vec![], 120);
        assert_eq!(p.name(), "codex-cli");
    }
}
