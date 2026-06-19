//! Pre-flight secret scanning for any file content placed into a prompt.
//! Patterns ported from the `second-opinion` skill. The scan is the only
//! safeguard for backends without a read-only sandbox (e.g. opencode).

use regex::Regex;
use std::sync::OnceLock;

const SECRET_PATTERN: &str = r#"AKIA[0-9A-Z]{16}|sk-[A-Za-z0-9]{20,}|ghp_[A-Za-z0-9]{36}|gho_[A-Za-z0-9]{36}|xox[baprs]-[A-Za-z0-9-]{10,}|-----BEGIN [A-Z ]*PRIVATE KEY-----|aws_secret_access_key\s*=|password\s*=\s*['"][^'"\s]{8,}|api[_-]?key\s*=\s*['"][^'"\s]{16,}"#;

fn secret_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(SECRET_PATTERN).expect("valid secret regex"))
}

/// Scan text for likely secrets; returns a redacted snippet for each match.
pub fn scan_secrets(text: &str) -> Vec<String> {
    secret_re()
        .find_iter(text)
        .map(|m| redact(m.as_str()))
        .collect()
}

fn redact(s: &str) -> String {
    let head: String = s.chars().take(4).collect();
    format!("{head}\u{2026}[redacted]")
}

/// Cheap filename heuristic for files likely to contain secrets.
pub fn risky_filename(name: &str) -> bool {
    let lower = name.to_lowercase();
    [".env", ".pem", ".key", "credentials", "secret", "id_rsa"]
        .iter()
        .any(|p| lower.contains(p))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_openai_style_key() {
        assert!(!scan_secrets("token = sk-abcdefghijklmnopqrstuvwxyz0123").is_empty());
    }

    #[test]
    fn detects_aws_access_key_id() {
        assert!(!scan_secrets("AKIAIOSFODNN7EXAMPLE").is_empty());
    }

    #[test]
    fn detects_private_key_header() {
        assert!(!scan_secrets("-----BEGIN RSA PRIVATE KEY-----").is_empty());
    }

    #[test]
    fn clean_text_has_no_hits() {
        assert!(scan_secrets("just normal prose, nothing secret here").is_empty());
    }

    #[test]
    fn hits_are_redacted() {
        let hits = scan_secrets("sk-abcdefghijklmnopqrstuvwxyz0123");
        assert!(!hits[0].contains("abcdefghijklmnop"));
    }

    #[test]
    fn flags_risky_filenames() {
        assert!(risky_filename(".env.local"));
        assert!(risky_filename("server.pem"));
        assert!(!risky_filename("notes.txt"));
    }
}
