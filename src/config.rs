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
fn d_ctx_tokens() -> u32 { 12000 }

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

/// Which debate stages see the attached file context. Off = none; First = the
/// round-0 broadcast only; ChairFirst = round 0 + the chairman's synthesis/judge;
/// Full = round 0, every critique round, and the chairman.
#[derive(Debug, Clone, Copy, Default, PartialEq, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ContextScope {
    Off,
    First,
    ChairFirst,
    #[default]
    Full,
}

impl ContextScope {
    /// Does the round-0 broadcast receive the context?
    pub fn round0(self) -> bool {
        !matches!(self, ContextScope::Off)
    }
    /// Do critique rounds receive the context?
    pub fn critique(self) -> bool {
        matches!(self, ContextScope::Full)
    }
    /// Does the chairman's synthesis/judge prompt receive the context?
    pub fn chairman(self) -> bool {
        matches!(self, ContextScope::ChairFirst | ContextScope::Full)
    }
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
    /// Bundled persona this model adopts in debates (see `abe personas`).
    /// None = no persona. Overridable per call with `debate --persona`.
    #[serde(default)]
    pub persona: Option<String>,
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
    /// Overall wall-clock budget (seconds) for the whole debate. When set and
    /// exceeded, remaining critique rounds are skipped and the debate proceeds
    /// straight to the decision step with the answers gathered so far. None = no
    /// overall cap (per-model `timeout_secs` still applies). Use it to stay under
    /// a caller's tool-call timeout (e.g. an MCP client).
    #[serde(default)]
    pub max_secs: Option<u64>,
    /// Which debate stages see attached file context (off|first|chair-first|full).
    /// Default full. Inert when no files are attached.
    #[serde(default)]
    pub context_scope: ContextScope,
    /// Token budget (estimated, ~4 chars/token) for attached file context.
    /// Over this, the context is truncated and a warning is emitted. Default
    /// 12000. `debate --lede` summarizes oversized files to fit instead.
    #[serde(default = "d_ctx_tokens")]
    pub context_max_tokens: u32,
}

impl Default for DebateCfg {
    fn default() -> Self {
        DebateCfg {
            rounds: d_rounds(),
            protocol: Protocol::default(),
            chairman: None,
            anonymize: true,
            min_models: d_min_models(),
            max_secs: None,
            context_scope: ContextScope::default(),
            context_max_tokens: d_ctx_tokens(),
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
            if let Some(p) = &m.persona {
                crate::persona::resolve(p)
                    .with_context(|| format!("model `{}` persona", m.name))?;
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

/// Parse a context-scope value (case-insensitive). Accepts both the readable
/// names and shorthands: off|0, first|1, chair-first|chair-1, full. Shared by
/// CLI, MCP, and HTTP overrides; config YAML uses the readable names.
pub fn parse_context_scope(s: &str) -> anyhow::Result<ContextScope> {
    match s.to_lowercase().as_str() {
        "off" | "0" => Ok(ContextScope::Off),
        "first" | "1" => Ok(ContextScope::First),
        "chair-first" | "chair-1" => Ok(ContextScope::ChairFirst),
        "full" => Ok(ContextScope::Full),
        other => anyhow::bail!("unknown context scope `{other}` (expected off|first|chair-first|full)"),
    }
}

/// Apply a `model=persona,model2=persona2` override spec onto the config's
/// models. Each pair sets that model's persona, overriding any YAML value.
/// Errors on a malformed entry, an unknown model, or an unknown persona.
/// Shared by the CLI `--persona` flag and the MCP/HTTP `personas` field.
pub fn apply_persona_overrides(cfg: &mut Config, spec: &str) -> anyhow::Result<()> {
    for pair in spec.split(',').map(str::trim).filter(|s| !s.is_empty()) {
        let (model, persona) = pair
            .split_once('=')
            .map(|(m, p)| (m.trim(), p.trim()))
            .with_context(|| format!("bad --persona entry `{pair}` (expected model=persona)"))?;
        crate::persona::resolve(persona)
            .with_context(|| format!("--persona {model}={persona}"))?;
        let m = cfg
            .models
            .iter_mut()
            .find(|m| m.name == model)
            .with_context(|| format!("unknown model `{model}` in --persona"))?;
        m.persona = Some(persona.to_string());
    }
    Ok(())
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
    fn accepts_known_persona_rejects_unknown() {
        let ok = Config::from_yaml(
            "models: [{name: a, kind: cli, cli: codex, persona: the-challenger}]",
        )
        .unwrap();
        assert_eq!(ok.models[0].persona.as_deref(), Some("the-challenger"));
        ok.validate().unwrap();

        let bad = Config::from_yaml(
            "models: [{name: a, kind: cli, cli: codex, persona: not-a-persona}]",
        )
        .unwrap();
        assert!(bad.validate().is_err(), "unknown persona must fail validation");
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

    #[test]
    fn context_scope_defaults_to_full() {
        let c = Config::from_yaml("models: [{name: a, kind: cli, cli: codex}]").unwrap();
        assert!(matches!(c.debate.context_scope, ContextScope::Full));
    }

    #[test]
    fn context_max_tokens_defaults_and_overrides() {
        let c = Config::from_yaml("models: [{name: a, kind: cli, cli: codex}]").unwrap();
        assert_eq!(c.debate.context_max_tokens, 12000);
        let c = Config::from_yaml(
            "models: [{name: a, kind: cli, cli: codex}]\ndebate: {context_max_tokens: 500}",
        )
        .unwrap();
        assert_eq!(c.debate.context_max_tokens, 500);
    }

    #[test]
    fn context_scope_parses_from_yaml() {
        let c = Config::from_yaml(
            "models: [{name: a, kind: cli, cli: codex}]\ndebate: {context_scope: chair-first}",
        )
        .unwrap();
        assert!(matches!(c.debate.context_scope, ContextScope::ChairFirst));
    }

    #[test]
    fn context_scope_stage_gates() {
        // off: nothing gets the doc.
        assert!(!ContextScope::Off.round0());
        assert!(!ContextScope::Off.critique());
        assert!(!ContextScope::Off.chairman());
        // first: round 0 only.
        assert!(ContextScope::First.round0());
        assert!(!ContextScope::First.critique());
        assert!(!ContextScope::First.chairman());
        // chair-first: round 0 + chairman, not critique.
        assert!(ContextScope::ChairFirst.round0());
        assert!(!ContextScope::ChairFirst.critique());
        assert!(ContextScope::ChairFirst.chairman());
        // full: everyone, every stage.
        assert!(ContextScope::Full.round0());
        assert!(ContextScope::Full.critique());
        assert!(ContextScope::Full.chairman());
    }

    #[test]
    fn apply_persona_overrides_sets_validates_and_errors() {
        let yaml = "models: [{name: a, kind: cli, cli: codex}, {name: b, kind: cli, cli: claude}]";
        let mut c = Config::from_yaml(yaml).unwrap();
        apply_persona_overrides(&mut c, "a=the-challenger, b=the-engineer").unwrap();
        assert_eq!(c.models[0].persona.as_deref(), Some("the-challenger"));
        assert_eq!(c.models[1].persona.as_deref(), Some("the-engineer"));

        let mut c = Config::from_yaml(yaml).unwrap();
        assert!(apply_persona_overrides(&mut c, "a=not-real").is_err(), "unknown persona");
        assert!(apply_persona_overrides(&mut c, "ghost=the-yonk").is_err(), "unknown model");
        assert!(apply_persona_overrides(&mut c, "a").is_err(), "missing = separator");
    }

    #[test]
    fn parse_context_scope_names_and_shorthands() {
        assert!(matches!(parse_context_scope("off").unwrap(), ContextScope::Off));
        assert!(matches!(parse_context_scope("0").unwrap(), ContextScope::Off));
        assert!(matches!(parse_context_scope("first").unwrap(), ContextScope::First));
        assert!(matches!(parse_context_scope("1").unwrap(), ContextScope::First));
        assert!(matches!(parse_context_scope("chair-first").unwrap(), ContextScope::ChairFirst));
        assert!(matches!(parse_context_scope("chair-1").unwrap(), ContextScope::ChairFirst));
        assert!(matches!(parse_context_scope("FULL").unwrap(), ContextScope::Full));
        assert!(parse_context_scope("bogus").is_err());
    }
}
