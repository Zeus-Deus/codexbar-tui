//! Turn parse-layer records into the domain-layer `ProviderSnapshot` the
//! renderer actually consumes.
//!
//! Responsibilities:
//!
//! * Find the first useful `UsageRecord` for a provider (the spurious
//!   `provider == "cli"` records are already filtered in parse.rs, so we
//!   just take the first one whose `provider` matches).
//! * Pick out today's `DailyCost` and the 30-day rolling sum.
//! * Map provider error records to `ProviderHealth` via the decision table
//!   from docs/cli-reference/rust-data-model.sketch.md.
//!
//! Kept intentionally mechanical — no I/O, no clock reads except the
//! `today` parameter the caller passes in. That keeps the module unit
//! testable without a mock clock.

use chrono::{DateTime, NaiveDate, Utc};

use crate::parse::{CostRecord, DailyCost, UsageRecord};

// ---------------------------------------------------------------------------
// Domain types (see docs/cli-reference/rust-data-model.sketch.md)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ProviderId {
    Claude,
    Codex,
}

impl ProviderId {
    pub fn cli_id(self) -> &'static str {
        match self {
            ProviderId::Claude => "claude",
            ProviderId::Codex => "codex",
        }
    }
    pub fn label(self) -> &'static str {
        match self {
            ProviderId::Claude => "Claude",
            ProviderId::Codex => "Codex",
        }
    }
}

#[derive(Debug, Clone)]
pub enum ProviderHealth {
    Ok,
    Stale { since: DateTime<Utc> },
    AuthMissing,
    NotSupportedOnLinux,
    Error { message: String },
}

#[derive(Debug, Clone)]
pub struct QuotaBar {
    pub used_percent: u8,
    pub window_label: String,
    pub resets_at: Option<DateTime<Utc>>,
    pub reset_hint: Option<String>,
}

#[derive(Debug, Clone)]
pub struct ModelShare {
    pub model: String,
    pub cost: f64,
    pub tokens: u64,
    pub percent_of_day: u8,
}

#[derive(Debug, Clone)]
pub struct ProviderSnapshot {
    pub provider: ProviderId,
    pub fetched_at: DateTime<Utc>,
    pub upstream_at: Option<DateTime<Utc>>,
    pub health: ProviderHealth,
    pub session: Option<QuotaBar>,
    pub weekly: Option<QuotaBar>,
    pub weekly_opus: Option<QuotaBar>,
    pub cost_today: Option<f64>,
    pub cost_30d: Option<f64>,
    pub top_models_today: Vec<ModelShare>,
    pub last_error: Option<String>,
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn window_label(minutes: u32, slot: WindowSlot) -> String {
    match (minutes, slot) {
        (300, _) => "5h".into(),
        (10080, WindowSlot::Secondary) => "7d".into(),
        (10080, WindowSlot::Tertiary) => "7d opus".into(),
        (m, _) if m % 1440 == 0 => format!("{}d", m / 1440),
        (m, _) if m % 60 == 0 => format!("{}h", m / 60),
        (m, _) => format!("{m}m"),
    }
}

/// Lightly clean the upstream `resetDescription`, which in observed v0.20
/// output arrives with whitespace stripped, e.g. "ResetsApr23,9pm(Europe/
/// Brussels)". We just re-insert a space after "Resets", after each comma,
/// and before a leading parenthesis so it reads tolerably.
fn clean_reset_hint(raw: &str) -> String {
    raw.replacen("Resets", "Resets ", 1)
        .replace(",", ", ")
        .replace("(", " (")
}

enum WindowSlot {
    Primary,
    Secondary,
    Tertiary,
}

fn to_bar(win: &crate::parse::Window, slot: WindowSlot) -> QuotaBar {
    QuotaBar {
        used_percent: win.used_percent.unwrap_or(0).min(100),
        window_label: window_label(win.window_minutes, slot),
        resets_at: win.resets_at,
        reset_hint: win.reset_description.as_deref().map(clean_reset_hint),
    }
}

/// Error decision table. Mirrors the table in rust-data-model.sketch.md.
fn classify_error(kind: &str, message: &str) -> ProviderHealth {
    let msg_lc = message.to_lowercase();
    if kind == "runtime" && msg_lc.contains("macos") {
        ProviderHealth::NotSupportedOnLinux
    } else if kind == "provider"
        && (msg_lc.contains("authentication required") || msg_lc.contains("unauthorized"))
    {
        ProviderHealth::AuthMissing
    } else {
        ProviderHealth::Error {
            message: message.to_string(),
        }
    }
}

// ---------------------------------------------------------------------------
// Public merge
// ---------------------------------------------------------------------------

/// Build a fresh snapshot from whatever the latest subprocess calls produced.
///
/// * `usage_records` — output of `parse::parse_usage` on the most recent
///   `codexbar usage` call for this provider (already filtered of CLI
///   noise). Empty vec = provider produced no records; that becomes an
///   `Error{"no data"}`.
/// * `cost_record` — `Some` if the most recent `codexbar cost` call
///   succeeded, `None` otherwise. Missing cost data is NOT an error; the
///   `cost_today`/`cost_30d` fields simply stay `None`.
/// * `today` — the local-calendar-day (in whatever tz the `cost` call was
///   spawned with; caller passes it in so merge is pure).
/// * `now` — the clock instant `fetched_at` is stamped with.
pub fn build_snapshot(
    provider: ProviderId,
    usage_records: &[UsageRecord],
    cost_record: Option<&CostRecord>,
    today: NaiveDate,
    now: DateTime<Utc>,
) -> ProviderSnapshot {
    let mut snap = ProviderSnapshot {
        provider,
        fetched_at: now,
        upstream_at: None,
        health: ProviderHealth::Error {
            message: "no data".into(),
        },
        session: None,
        weekly: None,
        weekly_opus: None,
        cost_today: None,
        cost_30d: None,
        top_models_today: Vec::new(),
        last_error: None,
    };

    // --- Usage ---------------------------------------------------------
    let target_id = provider.cli_id();
    let record = usage_records
        .iter()
        .find(|r| r.provider == target_id)
        // Fall back to the first record if there's no exact-provider match
        // (the error array from --source auto on Linux carries the provider
        // id even on failure, so this is belt-and-suspenders).
        .or_else(|| usage_records.first());

    if let Some(r) = record {
        snap.upstream_at = r.usage.as_ref().map(|u| u.updated_at);
        match (&r.usage, &r.error) {
            (Some(u), _) => {
                snap.health = ProviderHealth::Ok;
                snap.session = u.primary.as_ref().map(|w| to_bar(w, WindowSlot::Primary));
                snap.weekly = u
                    .secondary
                    .as_ref()
                    .map(|w| to_bar(w, WindowSlot::Secondary));
                snap.weekly_opus = u
                    .tertiary
                    .as_ref()
                    .map(|w| to_bar(w, WindowSlot::Tertiary));
            }
            (None, Some(e)) => {
                snap.health = classify_error(&e.kind, &e.message);
                snap.last_error = Some(e.message.clone());
            }
            (None, None) => {
                // No usage, no error — treat as an unknown-shape failure.
                snap.health = ProviderHealth::Error {
                    message: "empty usage record".into(),
                };
            }
        }
    }

    // --- Cost ----------------------------------------------------------
    if let Some(cost) = cost_record {
        if !cost.daily.is_empty() {
            let today_bucket = cost.daily.iter().find(|d| d.date == today);
            snap.cost_today = today_bucket.map(|d| d.total_cost);
            snap.top_models_today = top_models(today_bucket);

            // Last 30 entries: daily is unordered but rows are one-per-day,
            // so sort descending and take the head.
            let mut by_date: Vec<&DailyCost> = cost.daily.iter().collect();
            by_date.sort_by(|a, b| b.date.cmp(&a.date));
            let slice = &by_date[..by_date.len().min(30)];
            let sum: f64 = slice.iter().map(|d| d.total_cost).sum();
            snap.cost_30d = Some(sum);
        }
    }

    snap
}

fn top_models(today: Option<&DailyCost>) -> Vec<ModelShare> {
    let Some(day) = today else { return Vec::new() };
    let day_cost = day.total_cost.max(f64::EPSILON); // avoid /0 when cost==0
    let mut models: Vec<ModelShare> = day
        .model_breakdowns
        .iter()
        .map(|m| ModelShare {
            model: m.model_name.clone(),
            cost: m.cost,
            tokens: m.total_tokens,
            percent_of_day: ((m.cost / day_cost) * 100.0).round().clamp(0.0, 100.0) as u8,
        })
        .collect();
    models.sort_by(|a, b| b.cost.partial_cmp(&a.cost).unwrap_or(std::cmp::Ordering::Equal));
    models.truncate(3);
    models
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parse::{parse_cost, parse_usage};

    const USAGE_CLAUDE_CLI: &[u8] = include_bytes!("../docs/cli-reference/usage-claude-cli.json");
    const USAGE_CODEX_CLI: &[u8] = include_bytes!("../docs/cli-reference/usage-codex-cli.json");
    const USAGE_CLAUDE_AUTO: &[u8] = include_bytes!("../docs/cli-reference/usage-claude.json");
    const COST_CLAUDE: &[u8] = include_bytes!("../docs/cli-reference/cost-claude.json");

    fn now_utc() -> DateTime<Utc> {
        "2026-04-18T17:40:00Z".parse().unwrap()
    }

    fn today() -> NaiveDate {
        "2026-04-18".parse().unwrap()
    }

    #[test]
    fn claude_cli_success_builds_ok_snapshot_with_all_three_bars() {
        let usage = parse_usage(USAGE_CLAUDE_CLI).unwrap();
        let cost = parse_cost(COST_CLAUDE).unwrap();
        let snap = build_snapshot(
            ProviderId::Claude,
            &usage,
            Some(&cost[0]),
            today(),
            now_utc(),
        );
        assert!(matches!(snap.health, ProviderHealth::Ok));
        let s = snap.session.as_ref().expect("session bar");
        assert_eq!(s.window_label, "5h");
        let w = snap.weekly.as_ref().expect("weekly bar");
        assert_eq!(w.window_label, "7d");
        assert!(w.resets_at.is_some(), "weekly carries resetsAt");
        let o = snap.weekly_opus.as_ref().expect("opus bar");
        assert_eq!(o.window_label, "7d opus");
        assert!(snap.cost_today.is_some());
        assert!(snap.cost_30d.is_some());
        assert!(!snap.top_models_today.is_empty());
        // Top-3 cap.
        assert!(snap.top_models_today.len() <= 3);
    }

    #[test]
    fn codex_cli_auth_missing_is_classified() {
        let usage = parse_usage(USAGE_CODEX_CLI).unwrap();
        let snap = build_snapshot(ProviderId::Codex, &usage, None, today(), now_utc());
        assert!(matches!(snap.health, ProviderHealth::AuthMissing));
        assert!(snap.session.is_none());
        assert!(snap.last_error.is_some());
    }

    #[test]
    fn linux_auto_source_error_is_classified_not_supported() {
        let usage = parse_usage(USAGE_CLAUDE_AUTO).unwrap();
        let snap = build_snapshot(ProviderId::Claude, &usage, None, today(), now_utc());
        assert!(matches!(snap.health, ProviderHealth::NotSupportedOnLinux));
    }

    #[test]
    fn missing_cost_record_is_not_an_error() {
        let usage = parse_usage(USAGE_CLAUDE_CLI).unwrap();
        let snap = build_snapshot(ProviderId::Claude, &usage, None, today(), now_utc());
        assert!(matches!(snap.health, ProviderHealth::Ok));
        assert!(snap.cost_today.is_none());
        assert!(snap.cost_30d.is_none());
    }

    #[test]
    fn empty_usage_records_surfaces_as_no_data_error() {
        let snap = build_snapshot(ProviderId::Claude, &[], None, today(), now_utc());
        assert!(matches!(snap.health, ProviderHealth::Error { .. }));
    }

    #[test]
    fn top_models_are_sorted_and_capped() {
        let cost = parse_cost(COST_CLAUDE).unwrap();
        let today_bucket = cost[0]
            .daily
            .iter()
            .max_by_key(|d| d.model_breakdowns.len())
            .unwrap();
        let top = top_models(Some(today_bucket));
        assert!(top.len() <= 3);
        for pair in top.windows(2) {
            assert!(pair[0].cost >= pair[1].cost, "sort descending by cost");
        }
        let total: u32 = top.iter().map(|m| m.percent_of_day as u32).sum();
        assert!(total <= 100 + 3, "percents roughly sum <= 100 with rounding");
    }

    #[test]
    fn reset_hint_is_spaced() {
        let hint = clean_reset_hint("ResetsApr23,9pm(Europe/Brussels)");
        assert_eq!(hint, "Resets Apr23, 9pm (Europe/Brussels)");
    }

    #[test]
    fn window_label_maps_known_minutes() {
        assert_eq!(window_label(300, WindowSlot::Primary), "5h");
        assert_eq!(window_label(10080, WindowSlot::Secondary), "7d");
        assert_eq!(window_label(10080, WindowSlot::Tertiary), "7d opus");
        assert_eq!(window_label(60, WindowSlot::Primary), "1h");
        assert_eq!(window_label(1440, WindowSlot::Primary), "1d");
        assert_eq!(window_label(45, WindowSlot::Primary), "45m");
    }
}
