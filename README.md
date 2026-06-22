# Abe

> Multi-model LLM **debate** & **second-opinion** validation — named for Lincoln, one of history's great debaters.

Abe broadcasts a prompt to several LLMs, has them debate over N rounds, and returns a synthesized final answer plus an **agreement / disagreement** report. Models can be HTTP providers (OpenAI, Anthropic, OpenAI-compatible, local) **or** local CLIs (`codex`, `claude`, `opencode`) — mixed freely in one debate.

No API keys? Use the CLI providers — a debate between `codex` and `claude` runs with zero cloud config. Have a model on your network (vLLM / Ollama / LM Studio)? Point Abe at its `base_url` with no key at all.

Three surfaces: a **CLI**, an **MCP server** (for Claude Code and other MCP clients), and a small **web UI + JSON API**.

## Install

### Option 1 — prebuilt binary (no toolchain)

```bash
curl -fsSL https://raw.githubusercontent.com/yonk-labs/abe/main/install.sh | sh
```

Downloads the right binary for your platform (Linux x86_64, macOS arm64/x86_64) to `~/.local/bin`, and writes a starter config to `~/.config/abe/config.yaml`.

### Option 2 — cargo (any platform with Rust)

```bash
cargo install --git https://github.com/yonk-labs/abe
```

### Option 3 — Docker (web UI / API)

```bash
docker run -p 8080:8080 -v "$PWD/abe.yaml:/config.yaml" ghcr.io/yonk-labs/abe
# browser -> http://localhost:8080
```

The image runs the web UI bound to `0.0.0.0` inside the container. Two caveats:

- **CLI providers** (`codex`/`claude`/`opencode`) are **not** in the image — use HTTP providers for Dockerized debates.
- **Models on your LAN** (e.g. `192.168.1.x`) may be unreachable from a default-bridge container. Docker is the easy path for **cloud** providers (OpenAI/Anthropic). For models on your own network, the native binary (Option 1) runs on your host network and just works — or try `docker run --network host`.

### As a Claude Code plugin

The plugin registers an MCP server that runs the `abe` binary, so the binary must be installed and configured **first**. Full flow:

```bash
# 1. install the binary (also runs the setup wizard)
curl -fsSL https://raw.githubusercontent.com/yonk-labs/abe/main/install.sh | sh
#    (or: cargo install --git https://github.com/yonk-labs/abe  &&  abe init)

# 2. confirm it's on PATH and configured
abe models          # should list your models
```

Then in Claude Code:

```
/plugin marketplace add yonk-labs/abe
/plugin install abe@yonk-labs
```

Reload (`/reload-plugins` or restart Claude Code). You get the `abe` MCP tools (`debate`, `validate`) that Claude can call directly, plus two slash commands:

```
/abe:debate Is Postgres a good default database?
/abe:validate We should rewrite this service in Rust.
```

The `debate` MCP tool also accepts `context` (attach a doc — pass its contents), `context_scope`, and `personas` (`"model=persona,…"`), so Claude can ground a debate in a file you're discussing or run it as a panel of [personas](#personas). The `/abe:debate` command is wired to use them when your request calls for it.

**Prerequisites recap:** `abe` on your `PATH` + a config (`abe init` writes `~/.config/abe/config.yaml`). Without both, the MCP server can't start.

### With Codex or other MCP clients

`abe mcp` is a standard stdio MCP server, so it works with any MCP client — the Claude Code plugin marketplace above is Claude-specific, but the server isn't. After installing + `abe init`, register it in your client's MCP config with command `abe` and arg `mcp`. For Codex, add the same `abe mcp` invocation to your Codex MCP server config.

## Quick start

```bash
# First-time setup — interactive wizard asks how many models you want and
# walks you through each (OpenAI / Anthropic / local URL / CLI), then writes
# the config. For OpenAI and local/OpenAI-compatible endpoints it can query
# the endpoint's model list so you pick from a menu instead of typing an id.
# (Option 1's installer runs this for you.)
abe init

# Debate (reads ./abe.yaml, then ~/.config/abe/config.yaml by default)
abe debate "Is Postgres a good default database?"

# Second opinion (single model)
abe validate --reviewer codex "We should rewrite this service in Rust."

# Inspect configured models + reachability
abe models

# Run as an MCP server over stdio (exposes `debate` and `validate`)
abe mcp

# Serve the web UI + JSON API at http://127.0.0.1:8080
abe serve                      # local only
abe serve --host 0.0.0.0       # expose on your LAN (UI is UNAUTHENTICATED — trusted networks only)
```

Add `--json` to `debate`/`validate` for machine-readable output. Core `debate` flags: `--rounds N`, `--protocol synthesis|majority|judge`. Two bigger capabilities have their own sections below: **[attaching files](#attaching-files-context)** (`--files`, `--context-scope`, `--lede`) and **[personas](#personas)** (`--persona`).

```bash
# Attach a design doc and give two models opposing lenses
abe debate --files DESIGN.md,README.md \
  --persona gpt=the-challenger,claude=the-advocate \
  "Is this architecture sound?"
```

JSON API: `POST /api/debate {prompt, rounds?, protocol?, context?, context_scope?, personas?}` and `POST /api/validate {statement, reviewer?, context?}`. (For `/api/debate` and the MCP `debate` tool, `context` is the file *contents* — the server never reads host paths.)

## Config (YAML)

Copy [`config.example.yaml`](config.example.yaml) to `./abe.yaml` or `~/.config/abe/config.yaml`.

```yaml
defaults:
  temperature: 0.7
  max_tokens: 1024
  timeout_secs: 120
  retries: 2            # HTTP providers only (CLI providers are not retried)
  max_context_kb: 50    # warn when assembled context exceeds this

models:
  - { name: gpt,    kind: openai,            model: gpt-5.5,         api_key_env: OPENAI_API_KEY, persona: the-challenger }
  - { name: claude, kind: anthropic,         model: claude-opus-4-8, api_key_env: ANTHROPIC_API_KEY }
  - { name: local,  kind: openai-compatible, model: qwen3,           base_url: "http://192.168.1.10:8000/v1" }  # no key = no auth
  - { name: codex,  kind: cli, cli: codex,  fast: true }
  - { name: cc,     kind: cli, cli: claude }

debate:
  rounds: 2             # 0 = broadcast + decide only
  protocol: synthesis   # synthesis | majority | judge
  chairman: gpt         # model used for synthesis/judge (defaults to first model)
  anonymize: true       # hide model identities during cross-review
  min_models: 2         # abort if fewer than this respond
  context_scope: full   # which stages see --files: off | first | chair-first | full
  context_max_tokens: 12000  # cap on attached --files context (est. ~4 chars/token);
                             # over this it's truncated (or summarized, with --lede)

validate:
  reviewers: [codex]    # default reviewer(s) for `abe validate`
```

## Attaching files (context)

Pass reference material — a design doc, README, architecture notes, a spec — so the models debate *your* material instead of guessing. Files are read locally, **secret-scanned**, then injected into the prompt.

```bash
# One or more files, comma-separated
abe debate --files DESIGN.md "Does this design hold up under load?"
abe debate --files DESIGN.md,API.md,NOTES.md "Where are the gaps?"
```

**Which rounds see the files** — `--context-scope` (default `full`, or set `debate.context_scope` in YAML):

| scope | round 0 (opening) | critique rounds | chairman (synthesis) |
|-------|:--:|:--:|:--:|
| `off` | – | – | – |
| `first` | ✓ | – | – |
| `chair-first` | ✓ | – | ✓ |
| `full` *(default)* | ✓ | ✓ | ✓ |

`full` keeps the doc in front of every model the whole debate (most faithful, most tokens); dial down for big docs.

**Size guard** — attached context is capped at `debate.context_max_tokens` (default `12000`, estimated at ~4 chars/token). Over the cap it's truncated with a warning. To compress instead of truncating, add **`--lede`**: it summarizes the files to fit using the fast extractive [`lede`](https://github.com/yonk-labs/lede) tool (no LLM call). If `lede` isn't on `PATH`, it warns and falls back to truncation.

```bash
abe debate --files HUGE-SPEC.md --lede "Summarize the risks in this spec."
```

**Secrets** — file *contents* are scanned for credentials before sending; a risky file aborts the run. Pass `--allow-secrets` to override.

**MCP / HTTP** take the file *contents* as a `context` string (and `context_scope`) — the server never reads host paths. The agent/host reads the file and passes its text.

## Personas

Give each model a distinct voice/perspective so the panel argues from different angles. A model's persona becomes its **system prompt** for answering and critiquing; the chairman's synthesis stays neutral. Default is no persona.

```bash
abe personas                       # list the 12 bundled voices
abe debate --persona gemma=the-challenger,qwen=the-engineer "Is Postgres a good default?"
```

Set it durably in YAML per model (`persona: the-challenger`), or override per call with `--persona model=name`. A persona reference can be:

- a **bundled name** (table below) — run `abe personas` for the full descriptions;
- a **file path** — `--persona gemma=./voices/grumpy-sre.md` (the file's contents become the system prompt). Drop your own persona files anywhere and point at them;
- an **inline prompt** — any value containing whitespace is used verbatim:
  ```bash
  abe debate --persona 'gemma=You are a paranoid security reviewer who assumes every input is hostile.' "Review this."
  ```
  ```yaml
  # …or durably, as a YAML multi-line block:
  - { name: gemma, kind: openai-compatible, model: x, base_url: "...", persona: "You are a paranoid security reviewer." }
  ```

### Bundled voices

| name | lens it argues from |
|------|---------------------|
| `the-challenger` | skeptical performance expert — "what workload? show me the methodology" |
| `the-engineer` | mechanism-first — "what's actually happening under the hood?" |
| `data-nerd` | numbers only — refuses adjectives without a metric, version, and hardware |
| `the-builder` | ships it — happy path vs. the unhappy path, setup, error handling |
| `the-strategist` | OSS strategy veteran — "that's table stakes, not optional" |
| `the-advocate` | tech-lawyer / movement builder — sovereignty, licensing, societal stakes |
| `the-buyer` | technical buyer — TCO, lock-in, bus factor, "what's the exit plan?" |
| `the-ceo` | enterprise exec — ROI, platform strategy, scale in production |
| `the-cmo` | marketing strategist — business consequence, narrative, adoption |
| `the-founder` | builder-philosopher — data-backed, empathetic contrarian |
| `the-community-builder` | accessibility & onboarding — no gatekeeping, a concrete next step |
| `the-yonk` | 20-yr OSS DB/AI vet — production scars, right-sizing over hype |

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
- `debate --files` and `validate --files` secret-scan file contents before sending; pass `--allow-secrets` to override.
- `abe serve` binds `127.0.0.1` by default. `--host 0.0.0.0` exposes the **unauthenticated** UI on all interfaces — only do this on a trusted LAN.

## Status

v0.x. CLI (`init` / `debate` / `validate` / `models`), MCP server (`mcp`), and web UI + JSON API (`serve`). See `docs/` for the original design spec + plan.
