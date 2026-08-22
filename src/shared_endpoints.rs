//! Shared model-endpoint roster for the yonk-tools suite, read from
//! `~/.config/yonk-suite/models.yaml`. Fixes a recurring bug: which model is
//! actually served at a given LAN box changes over time, and until this
//! existed that fact was duplicated across every tool's own config
//! independently, going stale in each one separately. This is abe's own
//! independent copy of the loader (no shared crate across the suite — same
//! per-repo copy-paste-and-adapt precedent as thread.rs).
//!
//! A model entry opts in with `endpoint: <key>`; `resolve_endpoints` fills in
//! `model`/`base_url`/`api_key_env` from this file for any entry that set it,
//! before validation ever sees the config. Everything downstream of that
//! (HttpProvider::new, etc.) stays exactly as it was — it never sees `endpoint`.

use serde::Deserialize;
use std::collections::HashMap;
use std::path::PathBuf;

#[derive(Debug, Clone, Deserialize)]
pub struct SharedEndpoint {
    pub base_url: String,
    pub model: String,
    #[serde(default)]
    pub api_key_env: Option<String>,
}

#[derive(Debug, Deserialize)]
struct SharedFile {
    #[serde(default)]
    endpoints: HashMap<String, SharedEndpoint>,
}

fn shared_config_path() -> Option<PathBuf> {
    let home = std::env::var_os("HOME")?;
    Some(PathBuf::from(home).join(".config/yonk-suite/models.yaml"))
}

/// Missing file -> empty map (nothing to resolve, not an error) — a project
/// with no `endpoint:` references anywhere never needs this file to exist.
pub fn load() -> anyhow::Result<HashMap<String, SharedEndpoint>> {
    let Some(path) = shared_config_path() else {
        return Ok(HashMap::new());
    };
    if !path.exists() {
        return Ok(HashMap::new());
    }
    let s = std::fs::read_to_string(&path)
        .map_err(|e| anyhow::anyhow!("reading {}: {e}", path.display()))?;
    let parsed: SharedFile = serde_yaml::from_str(&s)
        .map_err(|e| anyhow::anyhow!("parsing {}: {e}", path.display()))?;
    Ok(parsed.endpoints)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_the_documented_shape() {
        let parsed: SharedFile = serde_yaml::from_str(
            "endpoints:\n  lan133:\n    base_url: http://192.168.1.133:8000/v1\n    model: qwen36-nvfp4\n",
        )
        .unwrap();
        let e = parsed.endpoints.get("lan133").unwrap();
        assert_eq!(e.base_url, "http://192.168.1.133:8000/v1");
        assert_eq!(e.model, "qwen36-nvfp4");
        assert!(e.api_key_env.is_none());
    }
}
