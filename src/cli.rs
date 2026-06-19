//! Command-line interface (clap) and output rendering.

use crate::config::{parse_protocol, CliKind, Config, ModelKind};
use crate::debate::{debate_from_config, DebateResult};
use crate::validate::validate_from_config;
use anyhow::Context;
use clap::{Args, Parser, Subcommand};

#[derive(Parser)]
#[command(
    name = "abe",
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
    /// Run as an MCP server over stdio, exposing `debate` and `validate` tools.
    Mcp(McpArgs),
    /// Serve the web UI + JSON API (POST /api/debate, /api/validate).
    Serve(ServeArgs),
    /// Interactive setup: ask how many models + their details, write a config YAML.
    Init,
}

#[derive(Args)]
pub struct DebateArgs {
    /// The question/prompt to debate.
    pub prompt: String,
    /// Config path (default: ./abe.yaml then ~/.config/abe/config.yaml).
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
    let mut cfg = Config::load_default(args.config.as_deref())?;
    if let Some(r) = args.rounds {
        cfg.debate.rounds = r;
    }
    if let Some(p) = &args.protocol {
        cfg.debate.protocol = parse_protocol(p)?;
    }

    eprintln!(
        "[abe] {} models \u{b7} {} round(s) \u{b7} {} \u{b7} ~{} model calls",
        cfg.models.len(),
        cfg.debate.rounds,
        format!("{:?}", cfg.debate.protocol).to_lowercase(),
        crate::debate::estimate_calls(cfg.models.len(), cfg.debate.rounds, cfg.debate.protocol),
    );

    let result = debate_from_config(&cfg, &args.prompt).await?;

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
    /// Config path (default: ./abe.yaml then ~/.config/abe/config.yaml).
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
    let cfg = Config::load_default(args.config.as_deref())?;
    let context = gather_context(args.files.as_deref(), args.allow_secrets)?;
    let res = validate_from_config(
        &cfg,
        &args.statement,
        args.reviewer.as_deref(),
        context.as_deref(),
    )
    .await?;

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
    /// Config path (default: ./abe.yaml then ~/.config/abe/config.yaml).
    #[arg(short, long)]
    pub config: Option<String>,
}

#[derive(Args)]
pub struct McpArgs {
    /// Config path (default: ./abe.yaml then ~/.config/abe/config.yaml).
    #[arg(short, long)]
    pub config: Option<String>,
}

#[derive(Args)]
pub struct ServeArgs {
    /// Config path (default: ./abe.yaml then ~/.config/abe/config.yaml).
    #[arg(short, long)]
    pub config: Option<String>,
    /// IP to bind. Default 127.0.0.1 (local only). Use 0.0.0.0 to expose on the
    /// network — the UI is UNAUTHENTICATED, so only do this on a trusted LAN.
    #[arg(long, default_value = "127.0.0.1")]
    pub host: String,
    /// Port to listen on.
    #[arg(short, long, default_value_t = 8080)]
    pub port: u16,
}

pub async fn run_models_cmd(args: ModelsArgs) -> anyhow::Result<()> {
    let cfg = Config::load_default(args.config.as_deref())?;
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
    fn which_finds_real_binaries_only() {
        assert!(which("sh"));
        assert!(!which("definitely-not-a-real-binary-xyzzy"));
    }
}
