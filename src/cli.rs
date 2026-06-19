//! Command-line interface (clap) and output rendering.

use crate::config::{Config, Protocol};
use crate::debate::{run_debate, DebateResult};
use crate::provider::{build_provider, Provider};
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
}
