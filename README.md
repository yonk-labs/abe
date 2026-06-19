# llm-debator

A small Rust binary that broadcasts a prompt to several LLMs, has them debate over N rounds, and returns a synthesized final answer plus an **agreement / disagreement** report. Models can be HTTP providers (OpenAI, Anthropic, OpenAI-compatible, local) **or** local CLIs (`codex`, `claude`, `opencode`) — mixed freely in one debate.

No API keys? Use the CLI providers — a debate between `codex` and `claude` runs with zero cloud config.

## Build

```bash
cargo build --release   # binary at target/release/llm-debator
```

## Quick start

```bash
# Debate (uses ./llm-debator.yaml or ~/.config/llm-debator/config.yaml by default)
llm-debator debate --config examples/cli-debate.yaml "Is Postgres a good default database?"

# Second opinion (single model)
llm-debator validate --reviewer codex "We should rewrite this service in Rust."

# Inspect configured models + reachability
llm-debator models --config examples/cli-debate.yaml

# Run as an MCP server over stdio (exposes `debate` and `validate` tools)
llm-debator mcp --config examples/cli-debate.yaml

# Serve the web UI + JSON API at http://127.0.0.1:8080  (--port to change)
llm-debator serve --config examples/cli-debate.yaml
```

The web UI is a single page with a debate/validate toggle. The JSON API: `POST /api/debate {prompt, rounds?, protocol?}` and `POST /api/validate {statement, reviewer?, context?}`.

Add `--json` to `debate`/`validate` for machine-readable output. `debate` flags: `--rounds N`, `--protocol synthesis|majority|judge`.

## Config (YAML)

```yaml
defaults:
  temperature: 0.7
  max_tokens: 1024
  timeout_secs: 120
  retries: 2            # HTTP providers only (CLI providers are not retried)
  max_context_kb: 50    # warn when assembled context exceeds this

models:
  - { name: gpt,        kind: openai,            model: gpt-5.1,         api_key_env: OPENAI_API_KEY }
  - { name: claude-api, kind: anthropic,         model: claude-opus-4-8, api_key_env: ANTHROPIC_API_KEY }
  - { name: local,      kind: openai-compatible, model: qwen3,           base_url: "http://localhost:11434/v1" }  # no key = no auth
  - { name: codex,      kind: cli, cli: codex,  fast: true }
  - { name: claude,     kind: cli, cli: claude }

debate:
  rounds: 2             # 0 = broadcast + decide only
  protocol: synthesis   # synthesis | majority | judge
  chairman: gpt         # model used for synthesis/judge (defaults to first model)
  anonymize: true       # hide model identities during cross-review
  min_models: 2         # abort if fewer than this respond

validate:
  reviewers: [codex]    # default reviewer(s) for `validate`
```

## Decision protocols

- **synthesis** — a chairman model merges all answers into one, with the agree/disagree report.
- **judge** — a judge model scores each answer and picks the single best, verbatim.
- **majority** — deterministic clustering of equal answers (no extra model call); best for short/factual answers.

## How it works

1. Broadcast the prompt to every model concurrently.
2. For each critique round, show each model the *other* models' latest answers (anonymized) and ask it to critique and revise.
3. Apply the decision protocol; always produce an agreement/disagreement report.

The report is a *synthesized interpretation* — raw per-model answers are always preserved in the result (`--json` / `rounds`).

## Safety

- CLI providers run read-only (`codex -s read-only --ephemeral`, `claude --permission-mode plan`).
- `validate --files` secret-scans file contents before sending; pass `--allow-secrets` to override.

## Status

v0.x. Surfaces: CLI (`debate` / `validate` / `models`), MCP server (`mcp`), and web UI + JSON API (`serve`). See `docs/specs/` and `docs/superpowers/plans/` for the design + plan.
