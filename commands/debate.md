---
description: Run a multi-model debate on a question and return a synthesized answer with an agreement/disagreement report
---

Use the `abe` debate MCP tool to run a multi-model debate on the question below. Then present, in order:

1. The final synthesized answer.
2. The points of agreement across the models.
3. The points of disagreement.

The `abe` debate tool accepts optional extras — use them when they fit the request:

- **`context`** — reference material to ground the debate (e.g. a design doc, README, or architecture notes). Pass the file *contents* as a string. Pair with **`context_scope`** (`off` | `first` | `chair-first` | `full`) to control which rounds see it; default `full`.
- **`personas`** — assign each model a debating voice as `"model=persona,model2=persona2"` (e.g. `gemma=the-challenger,qwen=the-engineer`). Run `abe personas` (or the shell fallback below) to see the bundled names. Omit for a neutral debate.

If the user attached or referenced a document, read it and pass its contents as `context`. If they asked for specific perspectives (a skeptic, an engineer, a buyer…), map those to `personas`.

If the `abe` MCP tool is unavailable, fall back to the shell: `abe debate "$ARGUMENTS"`, adding `--files <paths>`, `--context-scope <scope>`, and/or `--persona model=name,...` as needed (`abe personas` lists the voices).

Question: $ARGUMENTS
