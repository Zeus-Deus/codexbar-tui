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

/// Opaque provider identifier. The string is whatever
/// `codexbar config dump` emits for `providers[].id` — we do not hardcode
/// a closed set. Known IDs at time of writing: "claude", "codex", "cursor",
/// "opencode", "opencodego", "alibaba", "factory", "gemini", "antigravity",
/// "copilot", "zai", "minimax", "kimi", "kilo", "kiro", "vertexai",
/// "augment", "jetbrains", "kimik2", "amp", "ollama", "synthetic", "warp",
/// "openrouter", "perplexity". See docs/cli-reference/linux-caveats.md for
/// which of those are usable on Linux.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ProviderId(pub String);

impl ProviderId {
    pub fn new<S: Into<String>>(id: S) -> Self {
        Self(id.into())
    }
    /// The string codexbar expects on the `--provider` flag. Identity.
    pub fn cli_id(&self) -> &str {
        &self.0
    }
    /// Human-friendly panel title. Looked up in a small prettification
    /// table; unknown IDs return the raw string capitalised.
    pub fn label(&self) -> String {
        pretty_name(&self.0)
    }
}

impl std::fmt::Display for ProviderId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// Tiny lookup for the providers whose canonical capitalization diverges
/// from a naive title-case. Anything missing falls back to the raw ID with
/// the first letter upper-cased — good enough for ad-hoc provider names.
fn pretty_name(id: &str) -> String {
    match id {
        "claude" => "Claude".into(),
        "codex" => "Codex".into(),
        "gemini" => "Gemini".into(),
        "copilot" => "Copilot".into(),
        "cursor" => "Cursor".into(),
        "opencode" => "OpenCode".into(),
        "opencodego" => "OpenCode Go".into(),
        "zai" => "z.ai".into(),
        "kimi" => "Kimi".into(),
        "kimik2" => "Kimi K2".into(),
        "minimax" => "MiniMax".into(),
        "kilo" => "Kilo".into(),
        "kiro" => "Kiro".into(),
        "vertexai" => "Vertex AI".into(),
        "augment" => "Augment".into(),
        "jetbrains" => "JetBrains AI".into(),
        "antigravity" => "Antigravity".into(),
        "amp" => "Amp".into(),
        "ollama" => "Ollama".into(),
        "synthetic" => "Synthetic".into(),
        "warp" => "Warp".into(),
        "openrouter" => "OpenRouter".into(),
        "perplexity" => "Perplexity".into(),
        "alibaba" => "Alibaba Coding Plan".into(),
        "factory" => "Factory (Droid)".into(),
        other => {
            let mut c = other.chars();
            match c.next() {
                Some(first) => first.to_uppercase().chain(c).collect(),
                None => String::new(),
            }
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
    /// Any quota windows the provider exposed, in the upstream
    /// primary → secondary → tertiary order. No provider-specific
    /// naming lives here; each window carries its own `window_label`.
    pub windows: Vec<QuotaBar>,
    pub cost_today: Option<f64>,
    pub cost_30d: Option<f64>,
    pub top_models_today: Vec<ModelShare>,
    pub last_error: Option<String>,
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Map a `windowMinutes` value to a short display string. Two well-known
/// anchors (300 → "5h", 10080 → "weekly"); anything else falls through to a
/// numeric `Xd`/`Xh`/`Xm`. Provider-agnostic by design — the renderer no
/// longer knows what "opus" means.
fn window_label(minutes: u32) -> String {
    match minutes {
        300 => "5h".into(),
        10080 => "weekly".into(),
        m if m % 1440 == 0 => format!("{}d", m / 1440),
        m if m % 60 == 0 => format!("{}h", m / 60),
        m => format!("{m}m"),
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

fn to_bar(win: &crate::parse::Window) -> QuotaBar {
    QuotaBar {
        used_percent: win.used_percent.unwrap_or(0).min(100),
        window_label: window_label(win.window_minutes),
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
    let target_id = provider.cli_id().to_string();
    let mut snap = ProviderSnapshot {
        provider,
        fetched_at: now,
        upstream_at: None,
        health: ProviderHealth::Error {
            message: "no data".into(),
        },
        windows: Vec::new(),
        cost_today: None,
        cost_30d: None,
        top_models_today: Vec::new(),
        last_error: None,
    };

    // --- Usage ---------------------------------------------------------
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
                // Iterate primary → secondary → tertiary in that order;
                // keep whichever slots the provider populated. The UI
                // renders the Vec in order and never has to know which
                // slot was which.
                for slot in [&u.primary, &u.secondary, &u.tertiary] {
                    if let Some(w) = slot {
                        snap.windows.push(to_bar(w));
                    }
                }
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

    fn claude() -> ProviderId {
        ProviderId::new("claude")
    }
    fn codex() -> ProviderId {
        ProviderId::new("codex")
    }

    #[test]
    fn claude_cli_success_populates_windows_in_primary_secondary_tertiary_order() {
        let usage = parse_usage(USAGE_CLAUDE_CLI).unwrap();
        let cost = parse_cost(COST_CLAUDE).unwrap();
        let snap = build_snapshot(claude(), &usage, Some(&cost[0]), today(), now_utc());
        assert!(matches!(snap.health, ProviderHealth::Ok));
        assert_eq!(snap.windows.len(), 3, "claude populates all three windows");
        assert_eq!(snap.windows[0].window_label, "5h"); // primary
        assert_eq!(snap.windows[1].window_label, "weekly"); // secondary
        assert!(
            snap.windows[1].resets_at.is_some(),
            "weekly carries resetsAt"
        );
        assert_eq!(snap.windows[2].window_label, "weekly"); // tertiary -- now same label
        assert!(snap.cost_today.is_some());
        assert!(snap.cost_30d.is_some());
        assert!(!snap.top_models_today.is_empty());
        assert!(snap.top_models_today.len() <= 3);
    }

    #[test]
    fn codex_cli_auth_missing_is_classified() {
        let usage = parse_usage(USAGE_CODEX_CLI).unwrap();
        let snap = build_snapshot(codex(), &usage, None, today(), now_utc());
        assert!(matches!(snap.health, ProviderHealth::AuthMissing));
        assert!(snap.windows.is_empty());
        assert!(snap.last_error.is_some());
    }

    #[test]
    fn linux_auto_source_error_is_classified_not_supported() {
        let usage = parse_usage(USAGE_CLAUDE_AUTO).unwrap();
        let snap = build_snapshot(claude(), &usage, None, today(), now_utc());
        assert!(matches!(snap.health, ProviderHealth::NotSupportedOnLinux));
    }

    #[test]
    fn missing_cost_record_is_not_an_error() {
        let usage = parse_usage(USAGE_CLAUDE_CLI).unwrap();
        let snap = build_snapshot(claude(), &usage, None, today(), now_utc());
        assert!(matches!(snap.health, ProviderHealth::Ok));
        assert!(snap.cost_today.is_none());
        assert!(snap.cost_30d.is_none());
    }

    #[test]
    fn empty_usage_records_surfaces_as_no_data_error() {
        let snap = build_snapshot(claude(), &[], None, today(), now_utc());
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
        assert_eq!(window_label(300), "5h");
        assert_eq!(window_label(10080), "weekly");
        assert_eq!(window_label(60), "1h");
        assert_eq!(window_label(1440), "1d");
        assert_eq!(window_label(45), "45m");
        assert_eq!(window_label(20160), "14d"); // unknown cadence falls through
    }

    #[test]
    fn provider_id_pretty_names_cover_known_and_unknown() {
        assert_eq!(ProviderId::new("claude").label(), "Claude");
        assert_eq!(ProviderId::new("zai").label(), "z.ai");
        assert_eq!(ProviderId::new("opencodego").label(), "OpenCode Go");
        // Unknown IDs capitalise the first char and pass through.
        assert_eq!(ProviderId::new("some-new-provider").label(), "Some-new-provider");
    }

    // --- Task 1: local-vs-utc "today" alignment -------------------------
    //
    // Codexbar buckets cost by local calendar day (honors $TZ; see
    // docs/cli-reference/schema.md). The caller MUST pass the local date
    // for the cost_today lookup; feeding UTC-today on a timezone east or
    // west of UTC will select the wrong bucket when the two dates differ.
    //
    // This test constructs a synthetic CostRecord where 2026-04-17 and
    // 2026-04-18 buckets carry unambiguous sentinel costs, then asserts
    // that the passed-in `today` decides which one lands in cost_today.
    // main.rs invokes build_snapshot with `chrono::Local::now().date_naive()`,
    // so in production this lookup is always local-aligned.
    #[test]
    fn cost_today_lookup_uses_the_provided_local_date() {
        use crate::parse::{CostRecord, DailyCost};
        let record = CostRecord {
            provider: "claude".into(),
            source: "local".into(),
            updated_at: now_utc(),
            daily: vec![
                DailyCost {
                    date: "2026-04-17".parse().unwrap(),
                    input_tokens: 0,
                    output_tokens: 0,
                    cache_creation_tokens: 0,
                    cache_read_tokens: 0,
                    total_tokens: 0,
                    total_cost: 17.17,
                    models_used: vec![],
                    model_breakdowns: vec![],
                },
                DailyCost {
                    date: "2026-04-18".parse().unwrap(),
                    input_tokens: 0,
                    output_tokens: 0,
                    cache_creation_tokens: 0,
                    cache_read_tokens: 0,
                    total_tokens: 0,
                    total_cost: 18.18,
                    models_used: vec![],
                    model_breakdowns: vec![],
                },
            ],
        };
        let usage = parse_usage(USAGE_CLAUDE_CLI).unwrap();

        // Passing today=2026-04-18 (local date on this host) must pick
        // the 18.18 bucket -- NOT 17.17 (which would be what a UTC-today
        // would select at 23:00 local, UTC-1).
        let snap_local = build_snapshot(
            claude(),
            &usage,
            Some(&record),
            "2026-04-18".parse().unwrap(),
            now_utc(),
        );
        assert_eq!(snap_local.cost_today, Some(18.18));

        // Demonstrate the bug we're guarding against: if the caller ever
        // regresses to Utc::now().date_naive() on a user whose local
        // midnight hasn't flipped yet, we'd see the prior-day bucket.
        let snap_prior = build_snapshot(
            claude(),
            &usage,
            Some(&record),
            "2026-04-17".parse().unwrap(),
            now_utc(),
        );
        assert_eq!(snap_prior.cost_today, Some(17.17));

        // A date that isn't in the bucket list surfaces as None, not as a
        // panic or a stale value.
        let snap_future = build_snapshot(
            claude(),
            &usage,
            Some(&record),
            "2026-04-20".parse().unwrap(),
            now_utc(),
        );
        assert_eq!(snap_future.cost_today, None);
    }
}
