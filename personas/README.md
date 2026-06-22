# Bundled personas

Each `*.md` here is a short, second-person **system prompt** that a model adopts
during a debate (see the [Personas](../README.md#personas) section). They're
embedded into the binary at build time via `include_str!` in
[`src/persona.rs`](../src/persona.rs).

## Just want to use your own persona?

You don't need to touch this directory. Pass a **file path** or **inline text**
to `--persona` (or the YAML `persona:` field) — no rebuild required:

```bash
abe debate --persona 'gpt=./voices/grumpy-sre.md' "…"          # a file
abe debate --persona 'gpt=You are a terse, paranoid SRE.' "…"  # inline
```

This directory is only for personas that ship **bundled** — referenceable by a
short name (like `the-challenger`) and listed by `abe personas`.

## Add a bundled persona

1. **Write the file** — `personas/<name>.md`, one or two plain-prose paragraphs,
   second person, opening `You are <Name>, "<Alias>" — …`. Describe *how they
   reason and argue*, what they push on, and 2–4 signature moves — not a bio.
   It's prepended verbatim as the model's system message, so make it actionable.
   Keep it tight (~120–200 words). No front-matter, headings, or bullet lists.

2. **Register it** — add one line to the `PERSONAS` table in
   [`src/persona.rs`](../src/persona.rs), in listing order:

   ```rust
   ("<name>", include_str!("../personas/<name>.md")),
   ```

3. **Rebuild** — `cargo build`. That's it: `abe personas`, `--persona`, the YAML
   `persona:` field, and config validation all read the `PERSONAS` table, so the
   new name works everywhere automatically. `cargo test` checks every bundled
   persona is a non-empty second-person prompt.

## Style notes

The bundled set was distilled from longer persona profiles. Keep new ones
consistent: a clear lens (the angle they argue from), concrete verbal moves used
*sparingly*, and a reminder to critique ideas, not people. Read a couple of the
existing files first — `the-challenger.md` and `the-engineer.md` are good models.
