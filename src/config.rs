//! User config for codexbar-tui.
//!
//! File: `~/.config/codexbar-tui/config.toml` (XDG via `directories`).
//! Missing file -> defaults (Claude + Codex enabled, 60s/300s intervals).
//! Malformed file -> error, surfaced as a status-line message at startup.
//!
//! Shape:
//!
//! ```toml
//! providers = ["claude", "codex"]   # subset of {"claude", "codex"}; unknown names are ignored
//!
//! [refresh]
//! usage_secs = 60                   # clamped to >= 30
//! cost_secs  = 300                  # clamped to >= 30
//! ```
//!
//! We do **not** ever write this file from the TUI — it's user-owned.

use std::fs;
use std::path::PathBuf;
use std::time::Duration;

use directories::ProjectDirs;
use serde::Deserialize;
use thiserror::Error;

use crate::merge::ProviderId;
use crate::state::RefreshIntervals;

#[derive(Debug, Error)]
pub enum ConfigError {
    #[error("reading config file {path:?}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("parsing config file {path:?}: {source}")]
    Toml {
        path: PathBuf,
        #[source]
        source: toml::de::Error,
    },
}

#[derive(Debug, Clone)]
pub struct Config {
    pub providers: Vec<ProviderId>,
    pub intervals: RefreshIntervals,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            providers: vec![ProviderId::Claude, ProviderId::Codex],
            intervals: RefreshIntervals::default(),
        }
    }
}

/// Disk representation — only deserialize from this; never serialize it
/// back. Deserialize tolerant (unknown fields ignored, missing sections
/// fall back to defaults).
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields, default)]
struct Raw {
    providers: Option<Vec<String>>,
    refresh: Option<RawRefresh>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields, default)]
struct RawRefresh {
    usage_secs: Option<u64>,
    cost_secs: Option<u64>,
}

/// Canonical path. Falls back to `~/.config/codexbar-tui/config.toml` if
/// `ProjectDirs` comes back empty (unusual — usually means no $HOME).
pub fn default_path() -> Option<PathBuf> {
    let dirs = ProjectDirs::from("", "", "codexbar-tui")?;
    Some(dirs.config_dir().join("config.toml"))
}

/// Load + merge with defaults. Returns `(Config, Some(path))` on a
/// successful disk read, `(Config::default(), None)` when no config file
/// exists, and a `ConfigError` on malformed TOML.
pub fn load() -> Result<(Config, Option<PathBuf>), ConfigError> {
    let Some(path) = default_path() else {
        return Ok((Config::default(), None));
    };
    load_from(&path)
}

pub fn load_from(path: &std::path::Path) -> Result<(Config, Option<PathBuf>), ConfigError> {
    match fs::read_to_string(path) {
        Ok(body) => {
            let raw: Raw = toml::from_str(&body).map_err(|e| ConfigError::Toml {
                path: path.to_path_buf(),
                source: e,
            })?;
            Ok((merge_with_defaults(raw), Some(path.to_path_buf())))
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok((Config::default(), None)),
        Err(source) => Err(ConfigError::Io {
            path: path.to_path_buf(),
            source,
        }),
    }
}

fn merge_with_defaults(raw: Raw) -> Config {
    let mut cfg = Config::default();

    if let Some(names) = raw.providers {
        cfg.providers = names
            .into_iter()
            .filter_map(|n| match n.trim().to_ascii_lowercase().as_str() {
                "claude" => Some(ProviderId::Claude),
                "codex" => Some(ProviderId::Codex),
                _ => None, // silently skip unknowns; v1 only supports these two
            })
            .collect();
        // Preserve at least one provider so the TUI has something to render.
        if cfg.providers.is_empty() {
            cfg.providers = vec![ProviderId::Claude, ProviderId::Codex];
        }
    }

    if let Some(r) = raw.refresh {
        if let Some(s) = r.usage_secs {
            cfg.intervals.usage = Duration::from_secs(s);
        }
        if let Some(s) = r.cost_secs {
            cfg.intervals.cost = Duration::from_secs(s);
        }
        cfg.intervals = cfg.intervals.clamped();
    }

    cfg
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn write_tmp(body: &str) -> tempfile::NamedTempFile {
        let mut f = tempfile::NamedTempFile::new().unwrap();
        writeln!(f, "{body}").unwrap();
        f
    }

    #[test]
    fn defaults_when_no_file() {
        let path = std::env::temp_dir().join("codexbar-tui-absent.toml");
        let _ = std::fs::remove_file(&path);
        let (cfg, found) = load_from(&path).unwrap();
        assert!(found.is_none());
        assert_eq!(cfg.providers, vec![ProviderId::Claude, ProviderId::Codex]);
        assert_eq!(cfg.intervals.usage, Duration::from_secs(60));
    }

    #[test]
    fn parses_full_file() {
        let f = write_tmp(
            r#"
providers = ["claude"]

[refresh]
usage_secs = 120
cost_secs = 600
"#,
        );
        let (cfg, found) = load_from(f.path()).unwrap();
        assert!(found.is_some());
        assert_eq!(cfg.providers, vec![ProviderId::Claude]);
        assert_eq!(cfg.intervals.usage, Duration::from_secs(120));
        assert_eq!(cfg.intervals.cost, Duration::from_secs(600));
    }

    #[test]
    fn unknown_providers_are_dropped_not_errors() {
        let f = write_tmp(r#"providers = ["claude", "gemini", "copilot"]"#);
        let (cfg, _) = load_from(f.path()).unwrap();
        assert_eq!(cfg.providers, vec![ProviderId::Claude]);
    }

    #[test]
    fn empty_provider_list_falls_back_to_defaults() {
        let f = write_tmp(r#"providers = ["nope"]"#);
        let (cfg, _) = load_from(f.path()).unwrap();
        assert_eq!(cfg.providers, vec![ProviderId::Claude, ProviderId::Codex]);
    }

    #[test]
    fn intervals_clamped_to_30s_floor() {
        let f = write_tmp(
            r#"
[refresh]
usage_secs = 5
cost_secs = 10
"#,
        );
        let (cfg, _) = load_from(f.path()).unwrap();
        assert_eq!(cfg.intervals.usage, Duration::from_secs(30));
        assert_eq!(cfg.intervals.cost, Duration::from_secs(30));
    }

    #[test]
    fn malformed_toml_returns_error() {
        let f = write_tmp("providers = [not closed");
        let err = load_from(f.path()).unwrap_err();
        assert!(matches!(err, ConfigError::Toml { .. }));
    }

    #[test]
    fn unknown_top_level_field_is_an_error() {
        // deny_unknown_fields means typos surface loudly rather than silently.
        let f = write_tmp(r#"providesr = ["claude"]"#);
        let err = load_from(f.path()).unwrap_err();
        assert!(matches!(err, ConfigError::Toml { .. }));
    }
}
