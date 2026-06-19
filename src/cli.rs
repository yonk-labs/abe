//! Command-line interface (clap) and output rendering.

use crate::config::{CliKind, Config, ModelKind, Protocol};
use crate::debate::{run_debate, DebateResult};
use crate::provider::{build_provider, Provider};
use crate::validate::run_validate;
use anyhow::Context;
use clap::{Args, Parser, Subcommand};
use std::path::PathBuf;

#[derive(Parser)]
#[command(
    name = "llm-debator",
    version,
    about = "Multi-provider LLM debate/consensus proxy (HTTP + CLI providers)"
)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Command,
}

#[derive(Subcommand)]
pub enum Command {
    /// Broadcast a prompt to all models, debate, return a synthesized answer.
    Debate(DebateArgs),
    /// Get one model's independent second opinion on a statement/decision.
    Validate(ValidateArgs),
    /// List configured models and check basic reachability (CLI on PATH, keys set).
    Models(ModelsArgs),
}

#[derive(Args)]
pub struct DebateArgs {
    /// The question/prompt to debate.
    pub prompt: String,
    /// Config path (default: ./llm-debator.yaml then ~/.config/llm-debator/config.yaml).
    #[arg(short, long)]
    pub config: Option<String>,
    /// Override number of debate rounds.
    #[arg(long)]
    pub rounds: Option<u32>,
    /// Override decision protocol: synthesis | majority | judge.
    #[arg(long)]
    pub protocol: Option<String>,
    /// Emit JSON instead of pretty text.
    #[arg(long)]
    pub json: bool,
}

pub async fn run_debate_cmd(args: DebateArgs) -> anyhow::Result<()> {
    let mut cfg = load_config(args.config.as_deref())?;
    if let Some(r) = args.rounds {
        cfg.debate.rounds = r;
    }
    if let Some(p) = &args.protocol {
        cfg.debate.protocol = parse_protocol(p)?;
    }

    let providers: Vec<Box<dyn Provider>> = cfg
        .models
        .iter()
        .map(|m| build_provider(m, &cfg.defaults))
        .collect::<anyhow::Result<_>>()?;

    eprintln!(
        "[llm-debator] {} models \u{b7} {} round(s) \u{b7} {} \u{b7} ~{} model calls",
        cfg.models.len(),
        cfg.debate.rounds,
        format!("{:?}", cfg.debate.protocol).to_lowercase(),
        crate::debate::estimate_calls(cfg.models.len(), cfg.debate.rounds, cfg.debate.protocol),
    );

    let chair_name = cfg
        .resolved_chairman()
        .context("no chairman and no models to fall back to")?
        .to_string();
    let chairman: &dyn Provider = providers
        .iter()
        .find(|p| p.name() == chair_name)
        .map(|b| b.as_ref())
        .with_context(|| format!("chairman `{chair_name}` not found among providers"))?;

    let result = run_debate(&cfg, &providers, chairman, &args.prompt).await?;

    if args.json {
        println!("{}", serde_json::to_string_pretty(&result)?);
    } else {
        print_pretty(&result);
    }
    Ok(())
}

#[derive(Args)]
pub struct ValidateArgs {
    /// The statement / decision / answer to validate.
    pub statement: String,
    /// Config path (default: ./llm-debator.yaml then ~/.config/llm-debator/config.yaml).
    #[arg(short, long)]
    pub config: Option<String>,
    /// Reviewer model name (default: validate.reviewers[0], else first model).
    #[arg(short, long)]
    pub reviewer: Option<String>,
    /// Comma-separated files to include as context (secret-scanned first).
    #[arg(long)]
    pub files: Option<String>,
    /// Proceed even if a context file looks risky or contains secrets.
    #[arg(long)]
    pub allow_secrets: bool,
    /// Emit JSON instead of pretty text.
    #[arg(long)]
    pub json: bool,
}

pub async fn run_validate_cmd(args: ValidateArgs) -> anyhow::Result<()> {
    let cfg = load_config(args.config.as_deref())?;

    let reviewer_name = args
        .reviewer
        .clone()
        .or_else(|| cfg.validate.reviewers.first().cloned())
        .or_else(|| cfg.models.first().map(|m| m.name.clone()))
        .context("no reviewer configured and no models defined")?;
    let rcfg = cfg
        .models
        .iter()
        .find(|m| m.name == reviewer_name)
        .with_context(|| format!("reviewer `{reviewer_name}` is not a defined model"))?;
    let reviewer = build_provider(rcfg, &cfg.defaults)?;

    let context = gather_context(args.files.as_deref(), args.allow_secrets)?;
    let res = run_validate(reviewer.as_ref(), &args.statement, context.as_deref()).await?;

    if args.json {
        println!("{}", serde_json::to_string_pretty(&res)?);
    } else {
        println!("\n=== {}'s take ===\n\n{}\n", res.reviewer, res.take);
    }
    Ok(())
}

/// Read --files into a single context blob, secret-scanning each first.
fn gather_context(files: Option<&str>, allow_secrets: bool) -> anyhow::Result<Option<String>> {
    let Some(files) = files else {
        return Ok(None);
    };
    let mut buf = String::new();
    for path in files.split(',').map(|s| s.trim()).filter(|s| !s.is_empty()) {
        if crate::safety::risky_filename(path) && !allow_secrets {
            anyhow::bail!("refusing risky filename `{path}` (pass --allow-secrets to override)");
        }
        let content =
            std::fs::read_to_string(path).with_context(|| format!("reading context file {path}"))?;
        let hits = crate::safety::scan_secrets(&content);
        if !hits.is_empty() && !allow_secrets {
            anyhow::bail!(
                "possible secret(s) in `{path}`: {} match(es), e.g. {} \u{2014} pass --allow-secrets to override",
                hits.len(),
                hits[0]
            );
        }
        buf.push_str(&format!("\n--- {path} ---\n{content}\n"));
    }
    Ok(Some(buf))
}

#[derive(Args)]
pub struct ModelsArgs {
    /// Config path (default: ./llm-debator.yaml then ~/.config/llm-debator/config.yaml).
    #[arg(short, long)]
    pub config: Option<String>,
}

pub async fn run_models_cmd(args: ModelsArgs) -> anyhow::Result<()> {
    let cfg = load_config(args.config.as_deref())?;
    println!("{} model(s):", cfg.models.len());
    for m in &cfg.models {
        let status = match m.kind {
            ModelKind::Cli => match m.cli {
                Some(c) => {
                    let bin = cli_bin(c);
                    if which(bin) {
                        format!("\u{2713} {bin} on PATH")
                    } else {
                        format!("\u{2717} {bin} NOT on PATH")
                    }
                }
                None => "\u{2717} no cli specified".to_string(),
            },
            _ => match &m.api_key_env {
                Some(env) if std::env::var(env).is_ok() => format!("\u{2713} {env} set"),
                Some(env) => format!("\u{2717} {env} not set"),
                None => "(no api_key_env \u{2014} ok for local/no-auth)".to_string(),
            },
        };
        println!("  {:14} {:24} {}", m.name, kind_label(m), status);
    }
    Ok(())
}

fn cli_bin(c: CliKind) -> &'static str {
    match c {
        CliKind::Codex => "codex",
        CliKind::Claude => "claude",
        CliKind::Opencode => "opencode",
    }
}

fn kind_label(m: &crate::config::ModelCfg) -> String {
    match m.kind {
        ModelKind::Cli => format!("cli:{}", m.cli.map(cli_bin).unwrap_or("?")),
        ModelKind::Openai => format!("openai {}", m.model.as_deref().unwrap_or("?")),
        ModelKind::Anthropic => format!("anthropic {}", m.model.as_deref().unwrap_or("?")),
        ModelKind::OpenaiCompatible => format!("oai-compat {}", m.model.as_deref().unwrap_or("?")),
    }
}

fn which(bin: &str) -> bool {
    std::env::var_os("PATH")
        .map(|paths| std::env::split_paths(&paths).any(|dir| dir.join(bin).is_file()))
        .unwrap_or(false)
}

fn parse_protocol(s: &str) -> anyhow::Result<Protocol> {
    match s.to_lowercase().as_str() {
        "synthesis" => Ok(Protocol::Synthesis),
        "majority" => Ok(Protocol::Majority),
        "judge" => Ok(Protocol::Judge),
        other => anyhow::bail!("unknown protocol `{other}` (expected synthesis|majority|judge)"),
    }
}

fn load_config(explicit: Option<&str>) -> anyhow::Result<Config> {
    let candidates: Vec<PathBuf> = match explicit {
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
    anyhow::bail!(
        "no config found (looked for: {})",
        candidates
            .iter()
            .map(|p| p.display().to_string())
            .collect::<Vec<_>>()
            .join(", ")
    )
}

fn print_pretty(r: &DebateResult) {
    println!(
        "\n=== FINAL ANSWER ({} · {}) ===\n",
        r.protocol,
        r.models_used.join(", ")
    );
    println!("{}\n", r.final_answer);
    if !r.report.agreements.is_empty() {
        println!("--- Agreements ---");
        for a in &r.report.agreements {
            println!("  \u{2713} {a}");
        }
        println!();
    }
    if !r.report.disagreements.is_empty() {
        println!("--- Disagreements ---");
        for d in &r.report.disagreements {
            println!("  \u{26a0} {d}");
        }
        println!();
    }
    for w in &r.warnings {
        eprintln!("warning: {w}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_protocol_variants() {
        assert!(matches!(parse_protocol("synthesis").unwrap(), Protocol::Synthesis));
        assert!(matches!(parse_protocol("Majority").unwrap(), Protocol::Majority));
        assert!(matches!(parse_protocol("judge").unwrap(), Protocol::Judge));
        assert!(parse_protocol("nope").is_err());
    }

    #[test]
    fn which_finds_real_binaries_only() {
        assert!(which("sh"));
        assert!(!which("definitely-not-a-real-binary-xyzzy"));
    }
}
