//! Serde targets 1:1 with the upstream codexbar v0.20 JSON shapes and a
//! streaming parser that copes with its two quirks:
//!
//! 1. `codexbar usage` does **not** emit a single JSON document. It emits
//!    one `[...]` array per provider attempt, concatenated with no
//!    separator. We chain a `Deserializer::into_iter::<Vec<UsageRecord>>`
//!    walk to flatten all of them.
//! 2. Every Codex attempt trails a spurious record shaped like
//!    `{"provider":"cli","source":"cli","error":{"message":"Error",...}}`.
//!    We filter it out post-parse.
//!
//! See docs/cli-reference/schema.md for the observed field-by-field
//! schema. Optional fields are `Option<T>` so a newer codexbar version
//! adding fields does not break us.

use chrono::{DateTime, NaiveDate, Utc};
use serde::Deserialize;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum ParseError {
    #[error("json parse error: {0}")]
    Json(#[from] serde_json::Error),
}

// ---------------------------------------------------------------------------
// `codexbar usage` records
// ---------------------------------------------------------------------------

/// One record emitted by `codexbar usage`. Both success and error paths are
/// carried here — `usage`/`status`/`version` are absent on error; `error` is
/// absent on success.
#[derive(Debug, Clone, Deserialize)]
pub struct UsageRecord {
    pub provider: String,
    pub source: String,
    #[serde(default)]
    pub version: Option<String>,
    #[serde(default)]
    pub usage: Option<UsageBlock>,
    #[serde(default)]
    pub status: Option<StatusBlock>,
    #[serde(default)]
    pub error: Option<ErrorBlock>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct UsageBlock {
    pub identity: Identity,
    #[serde(default)]
    pub primary: Option<Window>,
    #[serde(default)]
    pub secondary: Option<Window>,
    #[serde(default)]
    pub tertiary: Option<Window>,
    #[serde(rename = "updatedAt")]
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Identity {
    #[serde(rename = "providerID")]
    pub provider_id: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Window {
    #[serde(default, rename = "usedPercent")]
    pub used_percent: Option<u8>,
    #[serde(rename = "windowMinutes")]
    pub window_minutes: u32,
    #[serde(default, rename = "resetsAt")]
    pub resets_at: Option<DateTime<Utc>>,
    #[serde(default, rename = "resetDescription")]
    pub reset_description: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct StatusBlock {
    pub indicator: String,
    pub description: String,
    pub url: String,
    #[serde(rename = "updatedAt")]
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ErrorBlock {
    pub code: i32,
    pub kind: String,
    pub message: String,
}

// ---------------------------------------------------------------------------
// `codexbar cost` records
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Deserialize)]
pub struct CostRecord {
    pub provider: String,
    pub source: String,
    #[serde(rename = "updatedAt")]
    pub updated_at: DateTime<Utc>,
    #[serde(default)]
    pub daily: Vec<DailyCost>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct DailyCost {
    pub date: NaiveDate,
    #[serde(default, rename = "inputTokens")]
    pub input_tokens: u64,
    #[serde(default, rename = "outputTokens")]
    pub output_tokens: u64,
    #[serde(default, rename = "cacheCreationTokens")]
    pub cache_creation_tokens: u64,
    #[serde(default, rename = "cacheReadTokens")]
    pub cache_read_tokens: u64,
    #[serde(default, rename = "totalTokens")]
    pub total_tokens: u64,
    #[serde(default, rename = "totalCost")]
    pub total_cost: f64,
    #[serde(default, rename = "modelsUsed")]
    pub models_used: Vec<String>,
    #[serde(default, rename = "modelBreakdowns")]
    pub model_breakdowns: Vec<ModelBreakdown>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ModelBreakdown {
    #[serde(rename = "modelName")]
    pub model_name: String,
    #[serde(default, rename = "totalTokens")]
    pub total_tokens: u64,
    #[serde(default)]
    pub cost: f64,
}

// ---------------------------------------------------------------------------
// `codexbar config dump` payload
// ---------------------------------------------------------------------------

/// Shape of `codexbar config dump` stdout. Single top-level object, no
/// arrays. See docs/cli-reference/config-dump-pretty.txt.
#[derive(Debug, Clone, Deserialize)]
pub struct ConfigDump {
    #[serde(default)]
    pub version: u32,
    #[serde(default)]
    pub providers: Vec<ConfigDumpProvider>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ConfigDumpProvider {
    pub id: String,
    #[serde(default)]
    pub enabled: bool,
}

impl ConfigDump {
    /// All provider IDs the dump carries, in upstream's emit order (which
    /// is the order we render panels).
    ///
    /// We intentionally ignore `ConfigDumpProvider.enabled`: that flag is a
    /// macOS-GUI convention (the CodexBar menu bar has checkboxes that
    /// toggle it). On fresh Linux installs every entry defaults to `false`
    /// — filtering on it would show nothing by default. Per-user hiding
    /// belongs in our own `hidden_providers` denylist; Linux-unsupported
    /// providers are filtered via `providers::LINUX_WEB_ONLY`.
    pub fn ids(&self) -> Vec<String> {
        self.providers.iter().map(|p| p.id.clone()).collect()
    }
}

/// Parse the stdout of `codexbar config dump`. The output is a well-formed
/// single JSON object; this is a thin wrapper around `serde_json::from_slice`
/// with our ParseError envelope.
pub fn parse_config_dump(bytes: &[u8]) -> Result<ConfigDump, ParseError> {
    Ok(serde_json::from_slice(bytes)?)
}

// ---------------------------------------------------------------------------
// Streaming parse for concatenated top-level arrays
// ---------------------------------------------------------------------------

/// Walk an arbitrary number of concatenated top-level JSON arrays, returning
/// their flattened elements. Empty input returns an empty vec.
fn parse_concatenated_arrays<T: for<'de> Deserialize<'de>>(
    bytes: &[u8],
) -> Result<Vec<T>, ParseError> {
    let mut out = Vec::new();
    let stream = serde_json::Deserializer::from_slice(bytes).into_iter::<Vec<T>>();
    for chunk in stream {
        out.extend(chunk?);
    }
    Ok(out)
}

/// Is this the spurious record that codexbar always tacks onto a Codex
/// attempt? See schema.md. Shape: `provider == "cli" && source == "cli" &&
/// error.kind == "provider" && error.message == "Error"`.
fn is_spurious_cli_noise(r: &UsageRecord) -> bool {
    r.provider == "cli"
        && r.source == "cli"
        && r
            .error
            .as_ref()
            .is_some_and(|e| e.kind == "provider" && e.message == "Error")
}

/// Parse the stdout of `codexbar usage ... --format json`. Handles the
/// concatenated-arrays framing and strips the spurious `provider=="cli"`
/// record.
pub fn parse_usage(bytes: &[u8]) -> Result<Vec<UsageRecord>, ParseError> {
    let mut records: Vec<UsageRecord> = parse_concatenated_arrays(bytes)?;
    records.retain(|r| !is_spurious_cli_noise(r));
    Ok(records)
}

/// Parse the stdout of `codexbar cost ... --format json`. codexbar emits a
/// single well-formed top-level array here, but we still tolerate the
/// concatenated framing in case that changes.
pub fn parse_cost(bytes: &[u8]) -> Result<Vec<CostRecord>, ParseError> {
    parse_concatenated_arrays(bytes)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    const USAGE_CLAUDE_CLI: &[u8] = include_bytes!("../docs/cli-reference/usage-claude-cli.json");
    const USAGE_CLAUDE_STATUS: &[u8] =
        include_bytes!("../docs/cli-reference/usage-claude-status.json");
    const USAGE_CLAUDE_AUTO: &[u8] = include_bytes!("../docs/cli-reference/usage-claude.json");
    const USAGE_CODEX_CLI: &[u8] = include_bytes!("../docs/cli-reference/usage-codex-cli.json");
    const USAGE_ALL: &[u8] = include_bytes!("../docs/cli-reference/usage-all.json");
    const COST_CLAUDE: &[u8] = include_bytes!("../docs/cli-reference/cost-claude.json");
    const COST_CODEX: &[u8] = include_bytes!("../docs/cli-reference/cost-codex.json");
    const CONFIG_DUMP: &[u8] = include_bytes!("../docs/cli-reference/config-dump.txt");

    #[test]
    fn usage_claude_cli_success() {
        let rs = parse_usage(USAGE_CLAUDE_CLI).unwrap();
        assert_eq!(rs.len(), 1);
        let r = &rs[0];
        assert_eq!(r.provider, "claude");
        assert_eq!(r.source, "claude");
        assert!(r.error.is_none());
        let u = r.usage.as_ref().expect("usage block");
        assert_eq!(u.identity.provider_id, "claude");
        let primary = u.primary.as_ref().expect("primary window");
        assert_eq!(primary.window_minutes, 300);
        assert!(primary.used_percent.is_some());
        // Secondary carries the only resets_at in this fixture.
        let secondary = u.secondary.as_ref().unwrap();
        assert!(secondary.resets_at.is_some());
        assert!(secondary.reset_description.is_some());
    }

    #[test]
    fn usage_claude_status_has_status_block() {
        let rs = parse_usage(USAGE_CLAUDE_STATUS).unwrap();
        assert_eq!(rs.len(), 1);
        let s = rs[0].status.as_ref().expect("status block");
        assert_eq!(s.indicator, "none");
        assert_eq!(s.description, "All Systems Operational");
        assert!(s.url.starts_with("https://"));
    }

    #[test]
    fn usage_auto_source_error_on_linux() {
        let rs = parse_usage(USAGE_CLAUDE_AUTO).unwrap();
        // Fixture has two concatenated arrays: [claude-auto error] + [spurious cli].
        // Spurious filter removes the second => exactly one record left.
        assert_eq!(rs.len(), 1);
        let e = rs[0].error.as_ref().expect("error block");
        assert_eq!(e.kind, "runtime");
        assert!(e.message.contains("macOS"));
        assert!(rs[0].usage.is_none());
    }

    #[test]
    fn usage_codex_cli_drops_spurious_record() {
        // Fixture has codex-cli auth error + spurious cli record.
        let rs = parse_usage(USAGE_CODEX_CLI).unwrap();
        assert_eq!(rs.len(), 1, "spurious cli record must be filtered out");
        assert_eq!(rs[0].provider, "codex");
        let e = rs[0].error.as_ref().unwrap();
        assert_eq!(e.kind, "provider");
        assert!(e.message.to_lowercase().contains("authentication"));
    }

    #[test]
    fn usage_all_errors_out_both_providers_but_leaves_no_cli_noise() {
        // --provider all on Linux: codex runtime error + spurious cli. Both
        // documented in schema.md.
        let rs = parse_usage(USAGE_ALL).unwrap();
        assert_eq!(rs.len(), 1);
        assert_eq!(rs[0].provider, "codex");
        assert_eq!(rs[0].error.as_ref().unwrap().kind, "runtime");
    }

    #[test]
    fn cost_claude_parses_many_days() {
        let rs = parse_cost(COST_CLAUDE).unwrap();
        assert_eq!(rs.len(), 1);
        let c = &rs[0];
        assert_eq!(c.provider, "claude");
        assert_eq!(c.source, "local");
        assert!(c.daily.len() >= 5, "need more than a handful of days");
        let first = &c.daily[0];
        assert!(first.total_tokens > 0);
        assert!(first.total_cost > 0.0);
        assert!(!first.model_breakdowns.is_empty());
        // Breakdown costs should roughly sum to totalCost; upstream rounding
        // is occasionally off in the trailing digit so allow 1 cent drift.
        let sum: f64 = first.model_breakdowns.iter().map(|m| m.cost).sum();
        assert!((sum - first.total_cost).abs() < 0.01);
    }

    #[test]
    fn cost_codex_empty_daily_is_not_an_error() {
        let rs = parse_cost(COST_CODEX).unwrap();
        assert_eq!(rs.len(), 1);
        assert_eq!(rs[0].provider, "codex");
        assert!(rs[0].daily.is_empty());
    }

    #[test]
    fn empty_input_is_empty_vec_not_error() {
        assert!(parse_usage(b"").unwrap().is_empty());
        assert!(parse_cost(b"").unwrap().is_empty());
    }

    #[test]
    fn malformed_input_is_an_error() {
        assert!(parse_usage(b"not json").is_err());
    }

    #[test]
    fn config_dump_returns_every_listed_provider() {
        let c = parse_config_dump(CONFIG_DUMP).unwrap();
        assert_eq!(c.version, 1);
        assert!(!c.providers.is_empty());
        // Every provider in the dump is returned, regardless of the
        // `enabled` flag. On this v0.20 fixture only codex is flagged
        // enabled=true, but the TUI still shows all of them (after the
        // Linux-skip list + user denylist are applied elsewhere).
        let ids = c.ids();
        assert!(ids.contains(&"codex".to_string()));
        assert!(ids.contains(&"claude".to_string()));
        assert!(ids.contains(&"gemini".to_string()));
        assert_eq!(ids.len(), c.providers.len());
    }

    #[test]
    fn config_dump_preserves_order_regardless_of_enabled_flag() {
        // Two of these are flagged enabled=false; ids() must still return
        // all four in input order.
        let body = br#"{"version":1,"providers":[
            {"id":"codex","enabled":true},
            {"id":"claude","enabled":false},
            {"id":"gemini","enabled":false},
            {"id":"warp","enabled":true}
        ]}"#;
        let c = parse_config_dump(body).unwrap();
        assert_eq!(c.ids(), vec!["codex", "claude", "gemini", "warp"]);
    }
}
