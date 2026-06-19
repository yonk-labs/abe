# llm-debator Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:executing-plans (inline) to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking. Executed inline by the host in this session.

**Goal:** Build a single Rust binary that broadcasts a prompt to multiple LLMs (HTTP via `genai` + CLI subprocesses codex/claude/opencode), runs N rounds of debate, and returns a final answer + agreement/disagreement report; plus a lightweight `validate` mode over CLI + MCP.

**Architecture:** One crate. A small `Provider` trait abstracts two transports (HTTP, CLI). A `debate` engine fans out per round and applies a decision protocol. A `validate` engine does single-shot second opinion. Surfaces: clap subcommands (`debate`, `validate`, `models`, `mcp`).

**Tech Stack:** Rust 1.93, tokio, genai (HTTP LLM), serde + serde_yaml + serde_json, clap (derive), async-trait, thiserror/anyhow, regex, rmcp (MCP, v0.3).

## Global Constraints (verbatim from spec)
- One crate, focused modules: `config`, `provider/{mod,http,cli}`, `debate`, `validate`, `report`, `safety`, `mcp`, `cli`/`main`.
- `Provider` trait stays small; never leak `genai` types upward.
- CLI providers in v1: best-effort reviewers — no retries, per-call timing + failures surfaced, hard timeout default. HTTP providers keep retries.
- Agree/disagree report = synthesized interpretation; raw per-model answers always preserved.
- Cost bounds first-class: caps on models, rounds, max_tokens; context size warning.
- Per-model fault tolerance: a failed model is dropped from a round; abort if live < `min_models`.
- Tests deterministic via a mock provider — no network.
- Environment note: no API keys / no Ollama here → live smoke tests use codex/claude CLIs.

## File Structure
| File | Responsibility |
|---|---|
| `Cargo.toml` | crate + deps |
| `src/main.rs` | clap entry, dispatch to subcommands |
| `src/cli.rs` | clap arg structs, output rendering (pretty/json) |
| `src/config.rs` | YAML structs + load + validation |
| `src/provider/mod.rs` | `Provider` trait, `Prompt`/`Answer`/`Usage`/`ProviderError`, `build_provider` factory, `MockProvider` (cfg test or always-compiled) |
| `src/provider/http.rs` | genai-backed provider |
| `src/provider/cli.rs` | subprocess provider (codex/claude/opencode) |
| `src/debate.rs` | round loop, protocols, `DebateResult` |
| `src/report.rs` | report structs + synthesis/analysis prompt builders + JSON parse |
| `src/validate.rs` | single-shot second opinion |
| `src/safety.rs` | secret scan, filename checks, size warning |
| `src/mcp.rs` | MCP stdio server (v0.3) |
| `tests/*.rs` | integration tests |

---

## VERSION 0.1 — Engine spine (synthesis only)

### Task 1: Scaffold + core provider types
**Files:** Create `Cargo.toml`, `src/main.rs`, `src/provider/mod.rs`
**Produces:** `Provider` trait, `Prompt`, `Answer`, `Usage`, `ProviderError`.

- [ ] Step 1: `cargo init --name llm-debator` (bin). Add deps: tokio (rt-multi-thread,macros,process,time), serde (derive), serde_yaml, serde_json, clap (derive), async-trait, thiserror, anyhow, regex, futures.
- [ ] Step 2: Write `src/provider/mod.rs` with types + trait:
```rust
use async_trait::async_trait;

#[derive(Debug, Clone)]
pub struct Prompt { pub system: Option<String>, pub user: String,
    pub temperature: f32, pub max_tokens: u32, pub json_mode: bool }

#[derive(Debug, Clone, Default)]
pub struct Usage { pub input_tokens: u64, pub output_tokens: u64 }

#[derive(Debug, Clone)]
pub struct Answer { pub model_name: String, pub text: String,
    pub usage: Option<Usage>, pub elapsed_ms: u64 }

#[derive(Debug, thiserror::Error)]
pub enum ProviderError {
    #[error("timeout after {0}ms")] Timeout(u64),
    #[error("provider {name} failed: {source}")]
    Backend { name: String, #[source] source: anyhow::Error },
}

#[async_trait]
pub trait Provider: Send + Sync {
    fn name(&self) -> &str;
    async fn complete(&self, p: &Prompt) -> Result<Answer, ProviderError>;
}
```
- [ ] Step 3: Add a `MockProvider` (compiled always, used by tests + offline runs) returning a scripted answer (optionally keyed by round). Test: `mock_returns_canned`.
- [ ] Step 4: `cargo test` → passes. Commit `feat: provider trait + mock provider`.

### Task 2: Config
**Files:** Create `src/config.rs`
**Produces:** `Config`, `ModelCfg`, `Defaults`, `DebateCfg`, `ValidateCfg`, `Config::load(path)`, `Config::validate()`.

- [ ] Step 1: Write failing test `parses_sample_yaml` + `rejects_unknown_chairman`.
- [ ] Step 2: Implement serde structs mirroring spec §4 (`kind` enum: openai|anthropic|openai-compatible|cli; cli enum: codex|claude|opencode). `load` reads file → serde_yaml. `validate` checks: unique names, chairman/reviewers reference defined models, rounds>=0, min_models>=1.
- [ ] Step 3: `cargo test` passes. Commit `feat: yaml config load + validation`.

### Task 3: Provider factory
**Files:** Modify `src/provider/mod.rs`
**Produces:** `build_provider(&ModelCfg, &Defaults) -> anyhow::Result<Box<dyn Provider>>` (http.rs/cli.rs impls stubbed to compile; cli wired in Task 6, http in Task 7).

- [ ] Step 1: Factory dispatches on `kind` → cli vs http constructor.
- [ ] Step 2: Commit `feat: provider factory`.

### Task 4: Debate engine — broadcast + synthesis (rounds 0)
**Files:** Create `src/debate.rs`, `src/report.rs`
**Produces:** `run_debate(cfg, providers, prompt) -> DebateResult`; `DebateResult`, `Report{agreements,disagreements}`, synthesis prompt builder + JSON parser.

- [ ] Step 1: Failing test `broadcast_collects_all_answers` (3 mock providers → 3 answers) and `synthesis_produces_final_and_report` (mock chairman returns JSON `{final_answer, agreements, disagreements}`).
- [ ] Step 2: Implement round-0 broadcast via `tokio::task::JoinSet` (concurrency cap). Implement `synthesis`: build chairman prompt from all answers, call chairman provider, parse JSON into `Report` + final answer. Robust JSON parse (extract first `{...}` block; on parse failure, fall back to raw text as final_answer with empty report + a `warnings` note).
- [ ] Step 3: `cargo test` passes. Commit `feat: broadcast + synthesis protocol`.

### Task 5: 1-round debate (critique)
**Files:** Modify `src/debate.rs`
- [ ] Step 1: Failing test `one_round_feeds_others_answers` (mock records the prompt it received; assert it contains other models' answers, anonymized as "Solution A/B").
- [ ] Step 2: Implement round 1: build critique prompt per model from others' latest answers (anonymized if cfg). Loop generalized to `rounds` (here 0|1).
- [ ] Step 3: `cargo test` passes. Commit `feat: single critique round`.

### Task 6: CLI provider (live-testable path)
**Files:** Create `src/provider/cli.rs`
**Produces:** `CliProvider` implementing `Provider` for codex/claude/opencode using spec §3.2 recipes via `tokio::process`.

- [ ] Step 1: Failing test `cli_parses_codex_output` using a fake executable stub (a shell script on PATH/temp that emits a known answer file) — verify parsing. Also `claude` (.result JSON) and `opencode` (JSONL) parse paths via stubs.
- [ ] Step 2: Implement subprocess invocation per backend (stdin handling, `-o` file for codex, jq-equivalent JSON parse in Rust via serde_json for claude/opencode), hard timeout, elapsed_ms, no retries.
- [ ] Step 3: `cargo test` passes. Commit `feat: cli subprocess provider`.
- [ ] Step 4: **Live smoke:** tiny config with codex-cli + claude-cli, run a 0-round synthesis debate; eyeball output.

### Task 7: HTTP provider (genai)
**Files:** Create `src/provider/http.rs`
- [ ] Step 1: Test `http_maps_config_to_adapter` (construct provider from ModelCfg; assert name + that openai-compatible sets base_url) — no network.
- [ ] Step 2: Implement via `genai::Client`; map kind→adapter; custom base_url via ServiceTargetResolver; key from api_key_env. **Verify genai version + resolver API against crates.io/docs before coding.**
- [ ] Step 3: `cargo test` passes (network tests `#[ignore]`). Commit `feat: genai http provider`.

### Task 8: CLI `debate` subcommand
**Files:** Create `src/cli.rs`; modify `src/main.rs`
- [ ] Step 1: Test `renders_json_result` (serialize DebateResult → JSON has expected keys).
- [ ] Step 2: clap: `debate [--config] [--rounds] [--protocol] [--json] <prompt>`; load config, build providers, run, render pretty or `--json`. Default config path `./llm-debator.yaml` then `~/.config/llm-debator/config.yaml`.
- [ ] Step 3: `cargo test` + `cargo build` pass. Commit `feat: debate CLI subcommand`.
- [ ] Step 4: **Live smoke:** `llm-debator debate --config sample.yaml "Is Rust a good choice for a CLI proxy?"` with codex+claude CLIs. **v0.1 DONE = testable.**

---

## VERSION 0.2 — Full debate

### Task 9: N-round loop
- [ ] Generalize loop to arbitrary `rounds`. Test `three_rounds_iterates` with mock. Commit.

### Task 10: majority protocol
- [ ] `report.rs`: analyzer prompt → cluster answers, pick dominant, report split. Test with mock analyzer. Commit.

### Task 11: judge protocol
- [ ] judge prompt → score each, pick best verbatim + report. Test with mock. Commit.
- [ ] Wire `protocol` config/flag to select synthesis|majority|judge.

### Task 12: fault tolerance + retries
- [ ] Drop model on error past retries; abort if live<min_models. HTTP retries w/ backoff; CLI no retries. Tests: `failed_model_dropped`, `aborts_below_min_models`. Commit.

### Task 13: cost bounds + size warning
- [ ] Enforce max models/rounds/max_tokens; warn when assembled prompt > max_context_kb. Tests. Commit. **v0.2 DONE.**

---

## VERSION 0.3 — validate + MCP + safety

### Task 14: validate engine
**Files:** Create `src/validate.rs`
- [ ] `run_validate(cfg, text, context, reviewer) -> ValidateResult{reviewer,verdict,take,top_risk}`. Port second-opinion prompt template. Reviewer selection + degenerate-opinion warn. Test with mock. Commit.

### Task 15: validate CLI subcommand
- [ ] `validate [--reviewer] [--files] [--json] <text>`. Live smoke with codex. Commit.

### Task 16: safety (secret scan)
**Files:** Create `src/safety.rs`
- [ ] Regex scan (spec §7 patterns) over file contents added to prompts; filename pre-check; abort unless `--allow-secrets`. Tests: positive/negative. Wire into validate `--files` and any file inclusion. Commit.

### Task 17: models connectivity check
- [ ] `models` subcommand: list configured models; for each, a cheap reachability probe (CLI: `--version`/help; HTTP: tiny request, skipped if no key). Commit.

### Task 18: MCP server
**Files:** Create `src/mcp.rs`
- [ ] **Verify `rmcp` crate name/version + stdio server API first.** Expose tools `debate(prompt,rounds?,protocol?)` and `validate(text,context?,reviewer?)` returning JSON. `mcp` subcommand runs stdio server. Manual smoke via an MCP client / `tools/list`. Commit. **v0.3 DONE = all features testable.**

---

## Self-Review
- **Spec coverage:** §3 providers (T1,3,6,7) · §4 config (T2) · §5 debate+protocols (T4,5,9,10,11) · §5.3 result/report (T4,10,11) · §6 validate (T14,15) · §7 safety (T16) · §8 errors/concurrency (T4,12) · §9 modules (all) · §11 milestones (version blocks) · MCP (T18). Covered.
- **Placeholders:** none (genai/rmcp API marked "verify" deliberately per cite-your-sources).
- **Type consistency:** `Provider`/`Prompt`/`Answer`/`DebateResult`/`Report`/`ValidateResult` names used consistently across tasks.
