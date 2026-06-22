//! Bundled debate personas — short system prompts embedded at build time.
//!
//! Each persona is a 1–2 paragraph, second-person directive (distilled from the
//! full persona library) used directly as a model's system message so that
//! model answers and critiques in that voice/perspective. Default is no persona.

/// (name, system-prompt) for every bundled persona, in listing order.
pub const PERSONAS: &[(&str, &str)] = &[
    ("the-challenger", include_str!("../personas/the-challenger.md")),
    ("the-engineer", include_str!("../personas/the-engineer.md")),
    ("the-strategist", include_str!("../personas/the-strategist.md")),
    ("the-advocate", include_str!("../personas/the-advocate.md")),
    ("data-nerd", include_str!("../personas/data-nerd.md")),
    ("the-buyer", include_str!("../personas/the-buyer.md")),
    ("the-ceo", include_str!("../personas/the-ceo.md")),
    ("the-cmo", include_str!("../personas/the-cmo.md")),
    ("the-founder", include_str!("../personas/the-founder.md")),
    ("the-builder", include_str!("../personas/the-builder.md")),
    ("the-community-builder", include_str!("../personas/the-community-builder.md")),
    ("the-yonk", include_str!("../personas/the-yonk.md")),
];

/// Resolve a persona reference to its system-prompt text. A reference may be an
/// inline literal prompt (anything containing whitespace — e.g. a YAML
/// multi-line block or quoted CLI text — is used verbatim), a path to a readable
/// file (its contents become the prompt), or a bundled persona name
/// (case-insensitive, see `PERSONAS`). Errors if a bareword matches none of
/// these. This is what lets users drop in custom personas alongside the bundled
/// set.
pub fn resolve(reference: &str) -> anyhow::Result<String> {
    let r = reference.trim();
    if r.chars().any(char::is_whitespace) {
        return Ok(r.to_string()); // inline literal prompt
    }
    if std::path::Path::new(r).is_file() {
        return std::fs::read_to_string(r)
            .map(|s| s.trim().to_string())
            .map_err(|e| anyhow::anyhow!("reading persona file `{r}`: {e}"));
    }
    if let Some(s) = get(r) {
        return Ok(s.to_string());
    }
    anyhow::bail!(
        "unknown persona `{r}` — not a bundled persona, a file path, or inline text (see `abe personas`)"
    )
}

/// The system prompt for a persona by name (case-insensitive), if bundled.
pub fn get(name: &str) -> Option<&'static str> {
    let name = name.trim();
    PERSONAS
        .iter()
        .find(|(n, _)| n.eq_ignore_ascii_case(name))
        .map(|(_, s)| s.trim())
}

/// All bundled persona names, in listing order.
pub fn names() -> Vec<&'static str> {
    PERSONAS.iter().map(|(n, _)| *n).collect()
}

/// A one-line gist for `abe personas`: the persona's first line/paragraph,
/// trimmed. Callers truncate for display. (First *line*, not first sentence —
/// abbreviations like "Dr." would split a sentence in the wrong place.)
pub fn gist(system: &str) -> &str {
    system.trim().lines().next().unwrap_or("").trim()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bundles_all_twelve() {
        assert_eq!(names().len(), 12);
    }

    #[test]
    fn get_is_case_insensitive_and_rejects_unknown() {
        assert!(get("the-challenger").unwrap().contains("Marcus Vane"));
        assert!(get("THE-CHALLENGER").is_some());
        assert!(get("  the-yonk  ").is_some(), "should trim the lookup");
        assert!(get("nope").is_none());
    }

    #[test]
    fn every_persona_is_a_nonempty_second_person_prompt() {
        for (name, _) in PERSONAS {
            let s = get(name).unwrap();
            assert!(s.starts_with("You are"), "{name} should be a 2nd-person prompt");
            assert!(s.len() > 200, "{name} should be a substantive prompt");
        }
    }

    #[test]
    fn resolve_handles_inline_file_and_bundled() {
        // Inline literal: contains whitespace → used verbatim as the prompt.
        assert_eq!(resolve("You are a grumpy SRE.").unwrap(), "You are a grumpy SRE.");
        // Bundled name → the embedded prompt.
        assert!(resolve("the-challenger").unwrap().contains("Marcus Vane"));
        // File path (a real repo file; cargo test runs at the crate root).
        assert!(resolve("personas/the-engineer.md").unwrap().contains("Nadia Kohler"));
        // Unknown bareword → error.
        assert!(resolve("no-such-persona").is_err());
    }

    #[test]
    fn gist_is_the_first_line() {
        // First line/paragraph, not first sentence (so "Dr." doesn't split it).
        assert_eq!(gist("You are Dr. Val. More.\n\nSecond para."), "You are Dr. Val. More.");
        assert_eq!(gist("  single line  "), "single line");
    }
}
