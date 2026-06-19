//! YAML configuration: models, defaults, debate + validate settings.

use anyhow::Context;
use serde::Deserialize;
use std::collections::HashSet;
use std::path::Path;

#[derive(Debug, Clone, Deserialize)]
pub struct Config {
    #[serde(default)]
    pub defaults: Defaults,
    pub models: Vec<ModelCfg>,
    #[serde(default)]
    pub debate: DebateCfg,
    #[serde(default)]
    pub validate: ValidateCfg,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Defaults {
    #[serde(default = "d_temp")]
    pub temperature: f32,
    #[serde(default = "d_max_tokens")]
    pub max_tokens: u32,
    #[serde(default = "d_timeout")]
    pub timeout_secs: u64,
    #[serde(default = "d_retries")]
    pub retries: u32,
    #[serde(default = "d_ctx_kb")]
    pub max_context_kb: u64,
}

impl Default for Defaults {
    fn default() -> Self {
        Defaults {
            temperature: d_temp(),
            max_tokens: d_max_tokens(),
            timeout_secs: d_timeout(),
            retries: d_retries(),
            max_context_kb: d_ctx_kb(),
        }
    }
}

fn d_temp() -> f32 { 0.7 }
fn d_max_tokens() -> u32 { 1024 }
fn d_timeout() -> u64 { 120 }
fn d_retries() -> u32 { 2 }
fn d_ctx_kb() -> u64 { 50 }
fn d_rounds() -> u32 { 2 }
fn d_true() -> bool { true }
fn d_min_models() -> u32 { 2 }

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ModelKind {
    Openai,
    Anthropic,
    OpenaiCompatible,
    Cli,
}

#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CliKind {
    Codex,
    Claude,
    Opencode,
}

#[derive(Debug, Clone, Copy, Default, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Protocol {
    #[default]
    Synthesis,
    Majority,
    Judge,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ModelCfg {
    pub name: String,
    pub kind: ModelKind,
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub base_url: Option<String>,
    #[serde(default)]
    pub api_key_env: Option<String>,
    #[serde(default)]
    pub cli: Option<CliKind>,
    #[serde(default)]
    pub fast: bool,
    #[serde(default)]
    pub extra_args: Vec<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct DebateCfg {
    #[serde(default = "d_rounds")]
    pub rounds: u32,
    #[serde(default)]
    pub protocol: Protocol,
    #[serde(default)]
    pub chairman: Option<String>,
    #[serde(default = "d_true")]
    pub anonymize: bool,
    #[serde(default = "d_min_models")]
    pub min_models: u32,
}

impl Default for DebateCfg {
    fn default() -> Self {
        DebateCfg {
            rounds: d_rounds(),
            protocol: Protocol::default(),
            chairman: None,
            anonymize: true,
            min_models: d_min_models(),
        }
    }
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct ValidateCfg {
    #[serde(default)]
    pub reviewers: Vec<String>,
}

impl Config {
    pub fn from_yaml(s: &str) -> anyhow::Result<Config> {
        serde_yaml::from_str(s).context("parsing config YAML")
    }

    pub fn load(path: &Path) -> anyhow::Result<Config> {
        let s = std::fs::read_to_string(path)
            .with_context(|| format!("reading config {}", path.display()))?;
        let c = Self::from_yaml(&s)?;
        c.validate()?;
        Ok(c)
    }

    pub fn validate(&self) -> anyhow::Result<()> {
        if self.models.is_empty() {
            anyhow::bail!("config has no models");
        }
        let mut seen = HashSet::new();
        for m in &self.models {
            if !seen.insert(m.name.as_str()) {
                anyhow::bail!("duplicate model name: {}", m.name);
            }
            match m.kind {
                ModelKind::Cli => {
                    if m.cli.is_none() {
                        anyhow::bail!("model `{}` has kind=cli but no `cli` field", m.name);
                    }
                }
                _ => {
                    if m.model.is_none() {
                        anyhow::bail!("model `{}` requires a `model` field", m.name);
                    }
                }
            }
        }
        let names: HashSet<&str> = self.models.iter().map(|m| m.name.as_str()).collect();
        if let Some(ch) = &self.debate.chairman {
            if !names.contains(ch.as_str()) {
                anyhow::bail!("debate.chairman `{}` is not a defined model", ch);
            }
        }
        for r in &self.validate.reviewers {
            if !names.contains(r.as_str()) {
                anyhow::bail!("validate reviewer `{}` is not a defined model", r);
            }
        }
        Ok(())
    }

    /// Chairman if set, else the first model (lazy-friendly default).
    pub fn resolved_chairman(&self) -> Option<&str> {
        self.debate
            .chairman
            .as_deref()
            .or_else(|| self.models.first().map(|m| m.name.as_str()))
    }

    /// Load from an explicit path, or the default search path
    /// (./abe.yaml then ~/.config/abe/config.yaml).
    pub fn load_default(explicit: Option<&str>) -> anyhow::Result<Config> {
        let candidates: Vec<std::path::PathBuf> = match explicit {
            Some(p) => vec![std::path::PathBuf::from(p)],
            None => {
                let mut v = vec![std::path::PathBuf::from("abe.yaml")];
                if let Some(home) = std::env::var_os("HOME") {
                    v.push(std::path::PathBuf::from(home).join(".config/abe/config.yaml"));
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
}

/// Parse a protocol name (case-insensitive). Shared by CLI, MCP, and HTTP.
pub fn parse_protocol(s: &str) -> anyhow::Result<Protocol> {
    match s.to_lowercase().as_str() {
        "synthesis" => Ok(Protocol::Synthesis),
        "majority" => Ok(Protocol::Majority),
        "judge" => Ok(Protocol::Judge),
        other => anyhow::bail!("unknown protocol `{other}` (expected synthesis|majority|judge)"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"
defaults:
  temperature: 0.5
  max_tokens: 512
models:
  - { name: gpt, kind: openai, model: gpt-5.1, api_key_env: OPENAI_API_KEY }
  - { name: local, kind: openai-compatible, model: q, base_url: "http://localhost:11434/v1" }
  - { name: codex-cli, kind: cli, cli: codex }
debate: { rounds: 1, protocol: synthesis, chairman: gpt }
validate: { reviewers: [codex-cli] }
"#;

    #[test]
    fn parses_sample_yaml() {
        let c = Config::from_yaml(SAMPLE).unwrap();
        assert_eq!(c.models.len(), 3);
        assert_eq!(c.models[0].name, "gpt");
        assert!(matches!(c.models[0].kind, ModelKind::Openai));
        assert!(matches!(c.models[2].kind, ModelKind::Cli));
        assert!(matches!(c.models[2].cli, Some(CliKind::Codex)));
        assert_eq!(c.defaults.temperature, 0.5);
        assert_eq!(c.debate.rounds, 1);
        c.validate().unwrap();
    }

    #[test]
    fn applies_defaults_when_omitted() {
        let c = Config::from_yaml("models: [{name: a, kind: cli, cli: codex}]").unwrap();
        assert_eq!(c.defaults.temperature, 0.7);
        assert_eq!(c.defaults.max_tokens, 1024);
        assert_eq!(c.debate.rounds, 2);
        assert!(c.debate.anonymize);
        assert_eq!(c.debate.min_models, 2);
        assert!(matches!(c.debate.protocol, Protocol::Synthesis));
    }

    #[test]
    fn rejects_unknown_chairman() {
        let c = Config::from_yaml(
            "models: [{name: a, kind: cli, cli: codex}]\ndebate: {chairman: ghost}",
        )
        .unwrap();
        assert!(c.validate().is_err());
    }

    #[test]
    fn rejects_duplicate_model_names() {
        let c = Config::from_yaml(
            "models: [{name: a, kind: cli, cli: codex}, {name: a, kind: cli, cli: claude}]",
        )
        .unwrap();
        assert!(c.validate().is_err());
    }

    #[test]
    fn cli_kind_requires_cli_field() {
        let c = Config::from_yaml("models: [{name: a, kind: cli}]").unwrap();
        assert!(c.validate().is_err());
    }

    #[test]
    fn http_kind_requires_model() {
        let c = Config::from_yaml("models: [{name: a, kind: openai}]").unwrap();
        assert!(c.validate().is_err());
    }

    #[test]
    fn parse_protocol_names() {
        assert!(matches!(parse_protocol("synthesis").unwrap(), Protocol::Synthesis));
        assert!(matches!(parse_protocol("MAJORITY").unwrap(), Protocol::Majority));
        assert!(matches!(parse_protocol("judge").unwrap(), Protocol::Judge));
        assert!(parse_protocol("bogus").is_err());
    }
}
