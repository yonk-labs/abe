# llm-debator — Design Spec

_Date: 2026-06-19 · Status: draft for review · Author: matt@theyonk.com (with Claude Code)_

## TL;DR
llm-debator is a single Rust binary that broadcasts a prompt to 1–5 LLMs — across HTTP providers (OpenAI, Anthropic, OpenAI-compatible, local) **and** local CLI subprocesses (`codex`, `claude`, `opencode`) — runs N rounds of mutual critique, and returns a final answer plus a structured **agreement/disagreement** report. It ships with two modes: a full **debate** and a lightweight **validate** ("second opinion") exposed over both CLI and MCP. v1 is CLI-first; web app and HTTP proxy are deferred.

---

## 1. Goals & Non-Goals

### Goals (v1)
- One static binary, one YAML config, bring-your-own-keys (incl. local + CLI models).
- Mixed providers in a single debate: HTTP and CLI models are interchangeable.
- N-round debate → final answer + explicit agreement/disagreement report.
- Configurable decision protocol: `synthesis | majority | judge`.
- Lightweight `validate` mode (single-shot second opinion), over CLI **and** MCP.
- First-class cost bounds and per-model fault tolerance.
- Deterministic, network-free tests via a mock provider.

### Non-Goals (v1 — explicitly deferred)
- Web app / dashboard UI.
- HTTP reverse-proxy / server endpoint for the *debate* mode.
- Conversation history / persistence / multi-turn sessions.
- Streaming token output.
- Adversarial-agent robustness safeguards beyond the basics (noted as v2 in §11).
- Auth/multi-tenant concerns (it's a local/self-host tool).

---

## 2. Architecture Overview

```
                ┌──────────── llm-debator (single binary) ────────────┐
   YAML config ─┤                                                      │
                │  cli/main ──> {debate | validate | mcp | models}     │
                │      │                                               │
                │      ▼                                               │
                │   engine (debate.rs / validate.rs)                   │
                │      │  uses                                         │
                │      ▼                                               │
                │   Provider trait ──┬── HttpProvider (genai)          │
                │                    └── CliProvider (tokio::process)  │
                │   report.rs (agree/disagree)   safety.rs (scan/caps) │
                └──────────────────────────────────────────────────────┘
```

- **Surfaces:** `debate`, `validate`, `models` are one-shot CLI subcommands. `mcp` is a long-running stdio MCP server exposing `debate` + `validate` as tools. All share the same engine.
- **One crate**, focused modules (§9). Not a workspace — it's small.

---

## 3. Provider Layer (the core seam)

A single trait abstracts the two transports so the debate engine never branches on provider type:

```rust
#[async_trait]
pub trait Provider: Send + Sync {
    fn name(&self) -> &str;
    async fn complete(&self, prompt: &Prompt) -> Result<Answer, ProviderError>;
}

pub struct Prompt {
    pub system: Option<String>,
    pub user: String,
    pub temperature: f32,
    pub max_tokens: u32,
    pub json_mode: bool,        // request structured output where supported
}

pub struct Answer {
    pub text: String,
    pub model_name: String,
    pub usage: Option<Usage>,   // tokens in/out when the backend reports them
    pub elapsed_ms: u64,
}
```

### 3.1 HttpProvider (via `genai`)
- Wraps a `genai::Client`. Maps config `kind` → genai adapter:
  - `openai` → OpenAI adapter
  - `anthropic` → Anthropic adapter
  - `openai-compatible` → OpenAI adapter + custom `base_url` via genai's `ServiceTargetResolver` (covers local Ollama/vLLM/LM Studio and any OAI-compatible endpoint)
- API key resolved from `api_key_env`; absent key ⇒ no-auth (valid for local endpoints).
- Honors `defaults` (temperature/max_tokens/timeout/retries), overridable per model.

### 3.2 CliProvider (subprocess — ported from `/second-opinion`)
Spawns the CLI per request via `tokio::process::Command`, prompt delivered via stdin/temp file, output parsed to text. Read-only postures and JSON shapes are taken from the battle-tested `/second-opinion` recipes:

| cli | invocation (read-only) | output parse |
|---|---|---|
| `codex` | `codex exec -s read-only --skip-git-repo-check --ephemeral --color never -o OUT - < PROMPT` | `OUT` = final message |
| `claude` | `(cd $TMP && claude -p "$PROMPT" --permission-mode plan --append-system-prompt "<independence>" --output-format json) < /dev/null` | `jq -r .result` |
| `opencode` | `opencode run "$PROMPT" -m <provider/model> --format json < /dev/null` ⚠️ no read-only flag | concat `.part.text` where `.type=="text"` |

Notes carried over from the skill (must be honored to avoid hangs/corruption):
- stdin must be satisfied (`< PROMPT` for codex, `< /dev/null` for claude/opencode) or the process blocks.
- keep stderr separate from stdout so banner/log noise never corrupts JSON.
- `opencode` requires an explicit `-m provider/model` (no reliable default) and has **no** read-only sandbox — the secret scan (§7) is its only safeguard.
- optional `fast` per model → codex `-c model_reasoning_effort=low`, claude `--model sonnet`, opencode faster configured model.
- hard per-call timeout (default from `timeout_secs`, ceiling 600s).

**CLI provider v1 contract (per second-opinion review — CLI calls are brittle and slow):** treat CLI models as *best-effort reviewers*, not equal-quality backends. v1: show per-call elapsed time, surface which CLI providers failed, hard timeout on by default, and **no retries for CLI providers in v1** (HTTP providers keep retries). The primary CLI-subprocess use case is `validate`, not full multi-round debate.

### 3.3 Provider construction
A `build_provider(model_cfg, defaults) -> Box<dyn Provider>` factory dispatches on `kind` (`cli` → CliProvider, else HttpProvider). Mirrors `pg-synapse`'s factory pattern.

---

## 4. Configuration (YAML)

Field shape borrowed from `llm-judge` (`examples/llm_config.sample.yaml`) and `pg-synapse` (`bench/models.toml`).

```yaml
defaults:
  temperature: 0.7
  max_tokens: 1024
  timeout_secs: 120
  retries: 2
  max_context_kb: 50        # warn/trim threshold for assembled prompts

models:
  - name: gpt
    kind: openai
    model: gpt-5.1
    api_key_env: OPENAI_API_KEY
  - name: claude-api
    kind: anthropic
    model: claude-opus-4-8
    api_key_env: ANTHROPIC_API_KEY
  - name: local-qwen
    kind: openai-compatible
    model: Qwen3.5-2B
    base_url: http://localhost:11434/v1   # no api_key_env => no auth
  - name: codex-cli
    kind: cli
    cli: codex                            # codex | claude | opencode
    # optional: model, fast: true, extra_args: [...]
  - name: claude-cli
    kind: cli
    cli: claude

debate:
  rounds: 2                 # 0 = broadcast only, no debate
  protocol: synthesis       # synthesis | majority | judge
  chairman: gpt             # model used by synthesis/judge protocols
  anonymize: true           # hide identities during cross-review (reduce bias)
  min_models: 2             # abort if fewer than this remain alive

validate:
  reviewers: [codex-cli]    # default reviewer(s); see §6 for selection rules
```

**Validation at load:** unknown `kind`/`cli` → error; `chairman`/`reviewers` must reference defined model names; `rounds >= 0`; `min_models >= 1`; duplicate `name` → error. Config errors are fatal and reported with the offending field.

---

## 5. Debate Engine

```
load config -> build providers
Round 0: broadcast prompt to ALL models concurrently (tokio JoinSet, capped) -> answers[0]
for k in 1..=rounds:
    for each live model (concurrently):
        critique_prompt = render(original_prompt, others_latest_answers)   # anonymized if configured
        answers[k][model] = model.complete(critique_prompt)
    drop models that errored past `retries`; abort if live < min_models
decision = apply(protocol, answers[last])
result = DebateResult {
    final_answer, agreements[], disagreements[],
    rounds[], per_model[], protocol, models_used, total_usage
}
```

### 5.1 Critique prompt (per round)
Each model receives: the original question, its own previous answer, and the other models' latest answers (labeled `Solution A/B/…` when `anonymize`). Instruction: identify errors/disagreements in the others, then produce a revised best answer. (Pattern from Du et al. + LLM Council.)

### 5.2 Decision protocols
- **synthesis** — `chairman` model receives all final answers and returns JSON `{final_answer, agreements[], disagreements[]}` in one call.
- **majority** — an analyzer call groups semantically-equivalent answers, picks the largest cluster as `final_answer`, reports cluster split as agreement strength; disagreements = minority positions.
- **judge** — `judge` model (= `chairman`) scores each final answer (rubric), selects the best verbatim as `final_answer`, and emits agreements/disagreements.

In all protocols the **agreement/disagreement report is mandatory output**. Where a protocol's model call can produce it inline (synthesis, judge), it does; `majority` adds one analyzer call.

**Trust framing (per second-opinion review):** the report is a *synthesized interpretation*, not ground truth — a reader aid downstream of another model call, which can invent agreement or omit minority objections. Raw per-model answers are always preserved in `per_model`/`rounds` and shown, so false consensus stays inspectable.

### 5.3 Result shape (JSON)
```json
{
  "final_answer": "…",
  "protocol": "synthesis",
  "agreements": ["point all models agreed on", "…"],
  "disagreements": [
    {"topic": "…", "positions": [{"model": "Solution A", "stance": "…"}]}
  ],
  "models_used": ["gpt", "claude-api", "codex-cli"],
  "rounds": [{"round": 0, "answers": [{"model": "gpt", "text": "…", "error": null}]}],
  "total_usage": {"input_tokens": 0, "output_tokens": 0}
}
```
Pretty (human) output renders final answer, then agreements (✓) and disagreements (⚠) sections, then a per-model/per-round appendix.

---

## 6. Quick-Validate Mode ("second opinion" as a service)

Single-shot. Input: a statement / answer / decision (+ optional context files). It is `/second-opinion` reimplemented in Rust.

- **Reviewer selection:** use `validate.reviewers` from config, or `--reviewer <name>`. The skill's "genuinely different model" principle applies: warn (degenerate-opinion guard) if the only reviewer is the same model family as whatever produced the thing being validated. For MCP callers, the calling agent names what it is so we can pick a different backend.
- **Prompt:** port the second-opinion template (direct answer, biggest risk/blind spot, what's right/wrong, would-you-differ). Blank-slate contract — reviewer sees only the prompt.
- **Output:** `{reviewer, verdict, take, top_risk}` (+ optional synthesis when multiple reviewers).

### Surfaces
- **CLI:** `llm-debator validate [--reviewer X] [--files a,b] [--json] "<text>"`
- **MCP:** tool `validate(text, context?, reviewer?)` returning the same JSON. The `mcp` subcommand also exposes `debate(prompt, rounds?, protocol?)`.

---

## 7. Safety (ported from `/second-opinion`)

- **Pre-flight secret scan** (mandatory, all backends): regex over the *contents* of any file inlined into a prompt (AWS keys, `sk-`/`ghp_`/`gho_` tokens, Slack `xox*`, PEM private keys, `password=`/`api_key=`). On match → abort with redacted line; CLI `--allow-secrets` / MCP explicit flag required to override.
- **Filename pre-check:** warn on `.env*`, `*.pem`, `*.key`, `*credentials*`, `*secret*`, `id_rsa*`.
- **CLI read-only:** codex `-s read-only --ephemeral`; claude `--permission-mode plan` (never `bypassPermissions`/`acceptEdits`); opencode warned (no sandbox) — never `--dangerously-skip-permissions`.
- **Cost bounds:** caps on model count, `rounds`, `max_tokens`; warn when assembled context exceeds `max_context_kb` (default 50KB) and trim least-relevant, never silently dropping question-central content.

---

## 8. Error Handling & Concurrency

- **Per-model isolation:** a model that errors or times out past `retries` is dropped from that round, its error recorded in `per_model`. Debate proceeds while live models `>= min_models`; otherwise abort with a clear message.
- **Retries:** exponential backoff honoring `Retry-After` (llm-judge pattern), bounded by `retries`.
- **Concurrency:** `tokio` runtime; per-round fan-out via `JoinSet` with a concurrency cap (default = model count, configurable). CLI subprocesses count against the same cap.
- **Timeouts:** per-call `timeout_secs`, hard ceiling 600s.

---

## 9. Module Layout

| Module | Responsibility |
|---|---|
| `main.rs` / `cli.rs` | clap subcommands, arg parsing, output rendering |
| `config.rs` | YAML load + validation |
| `provider/mod.rs` | `Provider` trait, `Prompt`/`Answer`/`ProviderError`, factory |
| `provider/http.rs` | genai-backed HTTP provider |
| `provider/cli.rs` | subprocess provider (codex/claude/opencode) |
| `debate.rs` | round loop, protocols, `DebateResult` |
| `validate.rs` | single-shot second-opinion logic |
| `report.rs` | agree/disagree structs + synthesis/analysis prompts |
| `safety.rs` | secret scan, filename checks, size warnings |
| `mcp.rs` | MCP stdio server exposing `debate` + `validate` |

---

## 10. Dependencies (minimal)

| Crate | Purpose | Note |
|---|---|---|
| `tokio` | async runtime, process, time | features: rt-multi-thread, macros, process, time |
| `genai` | multi-provider HTTP LLM client | `ServiceTargetResolver` for custom base URLs |
| `serde`, `serde_yaml`, `serde_json` | config + structured I/O | |
| `clap` | CLI subcommands | derive |
| `async-trait` | trait async methods | |
| `thiserror` / `anyhow` | error types | lib vs bin boundary |
| `futures` | JoinSet helpers | (or tokio::task::JoinSet directly) |
| `rmcp` | MCP server | **UNVERIFIED** — confirm official Rust MCP SDK crate + version at impl time |
| `regex` | secret scan | |

---

## 11. Milestones

- **v0.1 (spike):** config load → broadcast → `synthesis` protocol **only**, `rounds: 0|1` → CLI `debate`, pretty + `--json`. HTTP provider + **one** CLI provider path working well. Mock-provider tests. (Scope narrowed per second-opinion review: ship the engine spine first; the `protocol` config field accepts only `synthesis` until v0.2.)
- **v0.2:** N-round debate loop; agreement/disagreement report; cost bounds; per-model fault tolerance. `majority` + `judge` protocols added here — **only once real cases show `synthesis` is insufficient** (per review; the approved design keeps them, this phases them).
- **v0.3:** `validate` mode (CLI first, then MCP); `mcp` server exposing both tools — **gated on a stable CLI JSON contract** (MCP demands stable schemas + stdout purity, so the CLI output shape must settle first); secret scan; `models` connectivity check.
- **Post-v1 (watch/strategic):** HTTP proxy endpoint; small web app; adversarial-agent safeguard (skeptic/judge guard); streaming.

---

## 12. Testing

- **Mock provider** returning canned/scripted answers → deterministic, network-free tests of: round loop, all 3 protocols, agree/disagree assembly, `min_models` fallback, anonymization labeling.
- **CLI provider parsing** tested against fake executable stubs emitting each CLI's real output shape (codex `-o` file, claude JSON `.result`, opencode JSONL).
- **Config** parse + validation (good + malformed fixtures).
- **Safety** secret-scan unit tests (positive/negative patterns).
- Minimum bar per ponytail: every non-trivial module ships one runnable check.

---

## 13. Open Questions / To Verify at Implementation
1. `rmcp` crate name/version + its stdio server API (cite-your-sources: verify against the official MCP Rust SDK before coding `mcp.rs`).
2. `genai` `ServiceTargetResolver` exact API for per-model base_url + adapter override (check `examples/c06-target-resolver.rs`).
3. Whether `majority` clustering should be LLM-based (default) or add an embedding option later.
4. Repo not yet under git — needs `git init` before the spec/commits land.

---

## 14. References (borrow sources)
- Local: `../pg-synapse` (Rust provider trait/factory + model config), `../llm-judge` (YAML config + retry/backoff + score aggregation), `~/.claude/skills/second-opinion/SKILL.md` (CLI recipes, secret scan, prompt template).
- External: [karpathy/llm-council](https://github.com/karpathy/llm-council) (3-stage flow), [Du et al. ICML 2024](https://composable-models.github.io/llm_debate/) (debate loop), [MALLM](https://github.com/Multi-Agent-LLMs/mallm) (decision-protocol menu), [genai](https://github.com/jeremychone/rust-genai).
- Full landscape: `skill-output/research-base/` and `skill-output/market-intel/`.

---

## 15. Second-Opinion Review (codex, 2026-06-19)

External review via `/second-opinion` (backend: codex, since host is Claude). **Verdict: architecture sound; the v1 scope was too ambitious.** Adopted into the spec above:
- Ship `synthesis` only in v0.1; defer `majority`/`judge` until synthesis demonstrably fails (§5.2, §11).
- CLI providers = best-effort reviewers, not peer backends: no retries in v1, per-call timing + failures surfaced, `validate` is their primary use case (§3.2).
- Agree/disagree report framed as a synthesized interpretation; raw answers always inspectable (§5.2).
- Keep the `Provider` trait small; never leak `genai` types upward (already §3).
- Defer MCP until the CLI JSON contract is stable (§11).

**Open decision for the maintainer:** codex argued for cutting MCP and multi-protocol from v1 entirely. The current plan *keeps* them (they were approved design choices) but *phases* them into v0.2/v0.3 rather than front-loading them — preserving the approved design while building the engine spine first. Revisit if the v0.1 spike shows the engine is harder than expected.
