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
    /// codexbar returned a well-formed JSON payload whose `error` block we
    /// extracted verbatim. Used for the case where `codexbar config dump`
    /// rejects a flag and emits `[{"error":{"message":"Unknown option
    /// --foo"}}]` on stdout instead of the normal ConfigDump shape.
    #[error("codexbar rejected the request: {0}")]
    Remote(String),
    #[error("codexbar output shape not recognised (first bytes: {0:?})")]
    UnknownShape(String),
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
    /// `None` when codexbar omitted the `totalCost` field entirely —
    /// observed when the bucket's models include something codexbar v0.20
    /// doesn't have pricing for (e.g. a freshly-released model). Treating
    /// "missing" as `0.0` would hide the ambiguity: a real zero and an
    /// unpriced day look identical to the user. Option preserves the
    /// distinction so the renderer can show `—` rather than `$0.00`.
    #[serde(default, rename = "totalCost")]
    pub total_cost: Option<f64>,
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
    /// Missing field = codexbar has no pricing for this model (see DailyCost).
    #[serde(default)]
    pub cost: Option<f64>,
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

/// Parse the stdout of `codexbar config dump`.
///
/// Observed shapes in v0.20:
///
/// 1. **Happy path** — a single top-level object:
///    `{"version":1,"providers":[{"id":"codex","enabled":true},...]}`.
///    Field order varies across codexbar builds; serde handles that.
///
/// 2. **Flag-rejection path** — a top-level array of error records, same
///    framing as `parse_usage`: `[{"error":{"message":"Unknown option
///    --foo"}}]`. codexbar v0.20 emits this on `config dump` if it
///    doesn't recognise one of the flags (e.g. `--no-color`).
///
/// We walk whichever shape we get, returning the ConfigDump on success and
/// surfacing the error message to the caller on the second shape so the
/// status line can say *why* startup failed rather than a raw serde error.
pub fn parse_config_dump(bytes: &[u8]) -> Result<ConfigDump, ParseError> {
    // Dispatch on the first non-whitespace byte. `{` is the happy path
    // (single top-level object); `[` is the flag-rejection / generic
    // parse_usage-style framing where every value is checked for either a
    // ConfigDump shape or an `error.message` we can surface. Anything else
    // is a shape we don't recognise.
    let leading = bytes
        .iter()
        .copied()
        .find(|b| !b.is_ascii_whitespace());

    match leading {
        Some(b'{') => Ok(serde_json::from_slice::<ConfigDump>(bytes)?),
        Some(b'[') => parse_streaming_config_dump(bytes),
        _ => {
            let head: String = bytes
                .iter()
                .take(80)
                .map(|b| *b as char)
                .collect::<String>()
                .replace('\n', " ");
            Err(ParseError::UnknownShape(head))
        }
    }
}

/// Slow path: codexbar handed us a `parse_usage`-style top-level array (or
/// several concatenated). Walk each value and stop at the first one that is
/// either a ConfigDump or carries an `error.message`.
fn parse_streaming_config_dump(bytes: &[u8]) -> Result<ConfigDump, ParseError> {
    let iter = serde_json::Deserializer::from_slice(bytes).into_iter::<Vec<serde_json::Value>>();
    for chunk in iter {
        let chunk = chunk?;
        for value in &chunk {
            if value.is_object() {
                if let Ok(cd) = serde_json::from_value::<ConfigDump>(value.clone()) {
                    // Only accept if the value actually looks like a dump
                    // (has a providers array). Defaults-everywhere on
                    // ConfigDump make serde happy to accept anything;
                    // guard against silent data loss.
                    if value.get("providers").is_some() {
                        return Ok(cd);
                    }
                }
                if let Some(msg) = value
                    .get("error")
                    .and_then(|e| e.get("message"))
                    .and_then(|m| m.as_str())
                {
                    return Err(ParseError::Remote(msg.to_string()));
                }
            }
        }
    }
    let head: String = bytes
        .iter()
        .take(80)
        .map(|b| *b as char)
        .collect::<String>()
        .replace('\n', " ");
    Err(ParseError::UnknownShape(head))
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
    const CONFIG_DUMP: &[u8] = include_bytes!("../docs/cli-reference/config-dump.json");
    const CONFIG_DUMP_PRETTY: &[u8] =
        include_bytes!("../docs/cli-reference/config-dump-pretty.json");

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
        let day_total = first.total_cost.expect("audit fixture has totalCost");
        assert!(day_total > 0.0);
        assert!(!first.model_breakdowns.is_empty());
        // Breakdown costs should roughly sum to totalCost; upstream rounding
        // is occasionally off in the trailing digit so allow 1 cent drift.
        let sum: f64 = first
            .model_breakdowns
            .iter()
            .map(|m| m.cost.unwrap_or(0.0))
            .sum();
        assert!((sum - day_total).abs() < 0.01);
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
    fn config_dump_parses_real_captured_output() {
        let c = parse_config_dump(CONFIG_DUMP).unwrap();
        assert_eq!(c.version, 1);
        // v0.20 ships 25 provider slots in the default config. Pin the
        // count so a future codexbar release that renames/adds/drops one
        // surfaces loudly rather than drifting silently.
        assert_eq!(c.providers.len(), 25);
        let ids = c.ids();
        assert_eq!(ids.first().map(|s| s.as_str()), Some("codex"));
        assert!(ids.contains(&"claude".to_string()));
        assert!(ids.contains(&"gemini".to_string()));
        assert!(ids.contains(&"vertexai".to_string()));
        // Field-order agnosticism: the v0.20 Linux build emits
        // {"version":1,"providers":[{"enabled":true,"id":"codex"},...]}
        // with enabled before id; older/macOS builds emit id before enabled.
        // Both must parse identically.
        assert_eq!(ids.len(), c.providers.len());
    }

    #[test]
    fn config_dump_pretty_parses_identically() {
        let compact = parse_config_dump(CONFIG_DUMP).unwrap();
        let pretty = parse_config_dump(CONFIG_DUMP_PRETTY).unwrap();
        assert_eq!(compact.version, pretty.version);
        assert_eq!(compact.ids(), pretty.ids());
    }

    #[test]
    fn config_dump_preserves_order_regardless_of_enabled_flag() {
        // Synthetic: covers the contract that ids() preserves input order
        // and ignores the `enabled` flag. Real captures don't exercise
        // this (every fixture has the same default order upstream emits).
        let body = br#"{"version":1,"providers":[
            {"id":"codex","enabled":true},
            {"id":"claude","enabled":false},
            {"id":"gemini","enabled":false},
            {"id":"warp","enabled":true}
        ]}"#;
        let c = parse_config_dump(body).unwrap();
        assert_eq!(c.ids(), vec!["codex", "claude", "gemini", "warp"]);
    }

    #[test]
    fn config_dump_flag_rejection_surfaces_message() {
        // codexbar v0.20 emits this shape on stdout when `config dump`
        // receives an unrecognised flag. The parser must surface the
        // error message as ParseError::Remote so the caller can display
        // it, not a generic serde decode error.
        let body =
            br#"[{"error":{"message":"Unknown option --no-color","kind":"args","code":1},"source":"cli","provider":"cli"}]"#;
        match parse_config_dump(body) {
            Err(ParseError::Remote(msg)) => {
                assert!(msg.contains("--no-color"), "got: {msg}");
            }
            other => panic!("expected Remote, got {other:?}"),
        }
    }

    #[test]
    fn config_dump_unknown_shape_is_an_error_not_a_panic() {
        let err = parse_config_dump(br#"42"#).unwrap_err();
        match err {
            ParseError::UnknownShape(_) | ParseError::Json(_) => {}
            other => panic!("unexpected: {other:?}"),
        }
    }
}
