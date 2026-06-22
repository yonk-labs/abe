//! Command-line interface (clap) and output rendering.

use crate::config::{
    apply_persona_overrides, parse_context_scope, parse_protocol, CliKind, Config, ModelKind,
};
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
    /// List the bundled debate personas (use with `debate --persona`).
    Personas,
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
    /// Comma-separated files to attach as context for the debate (secret-scanned
    /// first). Which rounds see them is governed by --context-scope.
    #[arg(long)]
    pub files: Option<String>,
    /// Proceed even if a context file looks risky or contains secrets.
    #[arg(long)]
    pub allow_secrets: bool,
    /// Which stages see the attached files: off | first | chair-first | full
    /// (default: config, then full).
    #[arg(long)]
    pub context_scope: Option<String>,
    /// Assign personas to models: `model=persona,model2=persona2` (overrides
    /// the YAML `persona:` field). See `abe personas` for the available names.
    #[arg(long)]
    pub persona: Option<String>,
    /// If the attached files exceed context_max_tokens, summarize them to fit
    /// with the `lede` tool (fast, extractive) instead of truncating. Falls back
    /// to truncation with a warning if `lede` is not on PATH.
    #[arg(long)]
    pub lede: bool,
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
    if let Some(cs) = &args.context_scope {
        cfg.debate.context_scope = parse_context_scope(cs)?;
    }
    if let Some(p) = &args.persona {
        apply_persona_overrides(&mut cfg, p)?;
    }
    let mut context = gather_context(args.files.as_deref(), args.allow_secrets)?;
    if args.lede {
        context = maybe_lede(context, cfg.debate.context_max_tokens);
    }

    eprintln!(
        "[abe] {} models \u{b7} {} round(s) \u{b7} {} \u{b7} ~{} model calls",
        cfg.models.len(),
        cfg.debate.rounds,
        format!("{:?}", cfg.debate.protocol).to_lowercase(),
        crate::debate::estimate_calls(cfg.models.len(), cfg.debate.rounds, cfg.debate.protocol),
    );

    let result = debate_from_config(&cfg, &args.prompt, context.as_deref()).await?;

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
    /// Your current take, so the reviewer gives a counter-perspective on your
    /// reasoning rather than judging the statement cold.
    #[arg(long)]
    pub prior_reasoning: Option<String>,
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
        args.prior_reasoning.as_deref(),
        context.as_deref(),
    )
    .await?;

    if args.json {
        println!("{}", serde_json::to_string_pretty(&res)?);
    } else {
        println!("\n=== {}'s take ===\n\n{}\n", res.reviewer, res.take);
        if let Some(note) = &res.note {
            println!("note: {note}\n");
        }
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

/// When `--lede` is set and the gathered context exceeds the token budget,
/// summarize it with the external `lede` tool to fit. Best-effort: if `lede`
/// is missing or fails, warn and return the context unchanged so the engine's
/// truncation backstop takes over. Under budget (or no files) is a no-op.
fn maybe_lede(context: Option<String>, max_tokens: u32) -> Option<String> {
    let c = context?;
    if crate::debate::est_tokens(&c) <= max_tokens as usize {
        return Some(c);
    }
    match run_lede(&c, max_tokens as usize * 4) {
        Ok(summary) => {
            eprintln!(
                "[abe] lede compressed context ~{} \u{2192} ~{} tokens",
                crate::debate::est_tokens(&c),
                crate::debate::est_tokens(&summary)
            );
            Some(summary)
        }
        Err(e) => {
            eprintln!("warning: --lede unavailable ({e}); falling back to truncation");
            Some(c)
        }
    }
}

/// Pipe `text` through `lede --max-chars N` (stdin → stdout) for fast extractive
/// summarization. lede drains all stdin before emitting its (small) summary, so
/// a single write-then-read can't deadlock for any realistic document size.
// ponytail: std::process is fine — lede is a sub-5ms one-shot; no async needed.
fn run_lede(text: &str, max_chars: usize) -> anyhow::Result<String> {
    use std::io::Write;
    use std::process::{Command, Stdio};
    let mut child = Command::new("lede")
        .args(["--max-chars", &max_chars.to_string()])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .context("spawning `lede` (is it installed and on PATH?)")?;
    child
        .stdin
        .take()
        .expect("stdin was piped")
        .write_all(text.as_bytes())
        .context("writing to lede stdin")?;
    let out = child.wait_with_output().context("waiting for lede")?;
    if !out.status.success() {
        anyhow::bail!("lede exited with {}: {}", out.status, String::from_utf8_lossy(&out.stderr).trim());
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
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

pub fn run_personas_cmd() {
    let names = crate::persona::names();
    println!(
        "{} bundled personas \u{2014} assign with `debate --persona model=NAME` or the YAML `persona:` field:\n",
        names.len()
    );
    for (name, system) in crate::persona::PERSONAS {
        let gist = crate::persona::gist(system);
        let gist = if gist.chars().count() > 96 {
            format!("{}\u{2026}", gist.chars().take(95).collect::<String>())
        } else {
            gist.to_string()
        };
        println!("  {name:22} {gist}");
    }
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
