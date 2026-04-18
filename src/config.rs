//! User config for codexbar-tui.
//!
//! File: `~/.config/codexbar-tui/config.toml` (XDG via `directories`).
//!
//! The provider *list* is not set here — it comes from `codexbar config dump`
//! at startup so we automatically pick up whatever providers the user has
//! enabled upstream. This file just lets the user:
//!
//!   1. Hide specific providers the TUI should skip, even if codexbar has
//!      them enabled. (Denylist, applied over codexbar's enabled set.)
//!   2. Tune refresh intervals.
//!
//! Shape:
//!
//! ```toml
//! # Optional. IDs as they appear in `codexbar config dump` providers[].id
//! # (e.g. "codex", "claude", "cursor", "zai"...). Match is case-insensitive.
//! hidden_providers = ["factory", "perplexity"]
//!
//! [refresh]
//! usage_secs = 60    # clamped to >= 30
//! cost_secs  = 300   # clamped to >= 30
//! ```
//!
//! Missing file -> defaults (no denylist, 60s / 300s intervals).
//! Malformed file -> error, surfaced as a status-line message at startup.
//! We never write this file from the TUI -- it's user-owned.

use std::collections::HashSet;
use std::fs;
use std::path::PathBuf;
use std::time::Duration;

use directories::ProjectDirs;
use serde::Deserialize;
use thiserror::Error;

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

#[derive(Debug, Clone, Default)]
pub struct Config {
    /// Lower-cased provider IDs the user wants hidden. Compared against
    /// `codexbar config dump` IDs after normalising both to lower-case.
    pub hidden: HashSet<String>,
    pub intervals: RefreshIntervals,
}

impl Config {
    pub fn is_hidden(&self, id: &str) -> bool {
        self.hidden.contains(&id.trim().to_ascii_lowercase())
    }
}

/// Disk representation — only deserialize from this; never serialize it
/// back. `deny_unknown_fields` makes typos surface loudly.
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields, default)]
struct Raw {
    hidden_providers: Option<Vec<String>>,
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

    if let Some(names) = raw.hidden_providers {
        cfg.hidden = names
            .into_iter()
            .map(|n| n.trim().to_ascii_lowercase())
            .filter(|n| !n.is_empty())
            .collect();
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
        assert!(cfg.hidden.is_empty());
        assert_eq!(cfg.intervals.usage, Duration::from_secs(60));
    }

    #[test]
    fn parses_full_file() {
        let f = write_tmp(
            r#"
hidden_providers = ["factory", "Perplexity"]

[refresh]
usage_secs = 120
cost_secs = 600
"#,
        );
        let (cfg, found) = load_from(f.path()).unwrap();
        assert!(found.is_some());
        assert!(cfg.is_hidden("factory"));
        assert!(cfg.is_hidden("perplexity"));
        assert!(cfg.is_hidden("PERPLEXITY"), "lookup is case-insensitive");
        assert!(!cfg.is_hidden("claude"));
        assert_eq!(cfg.intervals.usage, Duration::from_secs(120));
        assert_eq!(cfg.intervals.cost, Duration::from_secs(600));
    }

    #[test]
    fn empty_hidden_list_is_not_an_error() {
        let f = write_tmp(r#"hidden_providers = []"#);
        let (cfg, _) = load_from(f.path()).unwrap();
        assert!(cfg.hidden.is_empty());
    }

    #[test]
    fn whitespace_only_entries_are_ignored() {
        let f = write_tmp(r#"hidden_providers = ["  ", "factory"]"#);
        let (cfg, _) = load_from(f.path()).unwrap();
        assert!(cfg.is_hidden("factory"));
        assert_eq!(cfg.hidden.len(), 1);
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
        let f = write_tmp("hidden_providers = [not closed");
        let err = load_from(f.path()).unwrap_err();
        assert!(matches!(err, ConfigError::Toml { .. }));
    }

    #[test]
    fn unknown_top_level_field_is_an_error() {
        // deny_unknown_fields means typos surface loudly rather than silently.
        let f = write_tmp(r#"hiden_providers = ["factory"]"#);
        let err = load_from(f.path()).unwrap_err();
        assert!(matches!(err, ConfigError::Toml { .. }));
    }
}
