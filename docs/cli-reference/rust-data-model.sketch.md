# Proposed Rust data-model sketch (pseudocode)

Structs only — field names + types. **No real Rust yet** per audit rules. This is for review before any `Cargo.toml` exists.

## Parsing layer (serde targets, 1:1 with JSON)

```text
// Streaming outer parser: multiple top-level arrays, concatenated.
//   serde_json::Deserializer::from_slice(bytes).into_iter::<Vec<UsageRecord>>()
//   -> chain flatten -> Vec<UsageRecord>

struct UsageRecord {
    provider:   String,                     // required
    source:     String,                     // required
    version:    Option<String>,             // claude CLI version etc
    usage:      Option<UsageBlock>,         // absent on error
    status:     Option<StatusBlock>,        // only with --status
    error:      Option<ErrorBlock>,         // only on per-provider failure
}

struct UsageBlock {
    identity:   Identity,
    primary:    Option<Window>,             // 5h / session
    secondary:  Option<Window>,             // weekly
    tertiary:   Option<Window>,             // weekly opus / secondary weekly
    updated_at: DateTime<Utc>,              // "updatedAt"
}

struct Identity {
    provider_id: String,                    // "providerID"
    // future: account label, email, workspace
}

struct Window {
    used_percent:      Option<u8>,          // "usedPercent"; absent when unavailable
    window_minutes:    u32,                 // "windowMinutes"
    resets_at:         Option<DateTime<Utc>>,   // "resetsAt" (RFC3339)
    reset_description: Option<String>,      // "resetDescription" (free text, sometimes whitespace-stripped)
}

struct StatusBlock {
    indicator:   String,                    // "none" | "minor" | "major" | "critical"
    description: String,
    url:         String,
    updated_at:  DateTime<Utc>,
}

struct ErrorBlock {
    code:    i32,
    kind:    String,                        // "runtime" | "provider" | ...
    message: String,
}

// cost subcommand

struct CostRecord {
    provider:   String,
    source:     String,                     // always "local" observed
    updated_at: DateTime<Utc>,
    daily:      Vec<DailyCost>,
    // upstream docs also mention (not observed at v0.20):
    // session_tokens, session_cost_usd, last30_days_tokens,
    // last30_days_cost_usd, totals{}
}

struct DailyCost {
    date:                  NaiveDate,       // "YYYY-MM-DD"
    input_tokens:          u64,
    output_tokens:         u64,
    cache_creation_tokens: u64,
    cache_read_tokens:     u64,
    total_tokens:          u64,
    total_cost:            f64,             // USD
    models_used:           Vec<String>,
    model_breakdowns:      Vec<ModelBreakdown>,
}

struct ModelBreakdown {
    model_name:   String,
    total_tokens: u64,
    cost:         f64,
}

// config dump

struct ConfigDump {
    version:   u32,                         // currently 1
    providers: Vec<ConfigProvider>,
}

struct ConfigProvider {
    id:      String,
    enabled: bool,
}
```

## Domain layer (what the TUI actually renders)

```text
// Providers we ship v1 support for. Extend later.
enum ProviderId { Claude, Codex }          // future: Gemini, Copilot, ZAI, ...

// Unified snapshot for one provider. Built by merging the latest
// successful UsageRecord + CostRecord for the provider.
struct ProviderSnapshot {
    provider:      ProviderId,
    fetched_at:    DateTime<Utc>,           // when codexbar-tui received this
    upstream_at:   Option<DateTime<Utc>>,   // usage.updated_at
    health:        ProviderHealth,
    session:       Option<QuotaBar>,        // from usage.primary
    weekly:        Option<QuotaBar>,        // from usage.secondary
    weekly_opus:   Option<QuotaBar>,        // from usage.tertiary (Claude only)
    cost_today:    Option<f64>,             // USD, from cost.daily[date=today]
    cost_30d:      Option<f64>,             // USD, sum of last 30 cost.daily entries
    top_models_today: Vec<ModelShare>,      // sorted desc by cost
    last_error:    Option<String>,          // most recent error message to surface
}

enum ProviderHealth {
    Ok,
    Stale { since: DateTime<Utc> },         // snapshot older than 2× refresh interval
    AuthMissing,                            // provider error kind == "provider", known auth string
    NotSupportedOnLinux,                    // error kind == "runtime", message contains "macOS only"
    Error { message: String },
}

struct QuotaBar {
    used_percent:    u8,                    // 0..=100 clamp
    window_label:    String,                // "5h" | "7d" | "weekly opus"
    resets_at:       Option<DateTime<Utc>>,
    resets_in:       Option<Duration>,      // computed: resets_at - now (None if unknown)
    reset_hint:      Option<String>,        // lightly-cleaned reset_description
}

struct ModelShare {
    model:   String,
    cost:    f64,
    tokens:  u64,
    percent_of_day: u8,
}

// App-wide state the renderer reads.
struct AppState {
    providers_enabled: Vec<ProviderId>,     // user config, not codexbar config
    snapshots:         HashMap<ProviderId, ProviderSnapshot>,
    refresh_interval:  RefreshIntervals,
    theme:             OmarchyTheme,
}

struct RefreshIntervals {
    usage_secs: u32,                        // default 60, min 30
    cost_secs:  u32,                        // default 300
    status_secs: u32,                       // default 3600, 0 = disabled
}

// Orchestration layer spawns one worker per (ProviderId, Command) pair.
enum Command { Usage, Cost, UsageStatus }
```

## Error-path handling (decision table, not code)

| `error.kind` | `error.message` matches | Map to `ProviderHealth` |
|---|---|---|
| `"runtime"` | contains `"macOS only"` | `NotSupportedOnLinux` |
| `"provider"` | contains `"authentication required"` or `"unauthorized"` | `AuthMissing` |
| `"provider"` | just `"Error"` (the spurious `provider=="cli"` record) | filter out entirely before mapping |
| any | other | `Error { message }` |

Combined with: if no record at all comes back within the timeout, fall back to `Stale { since: last_known }` on the existing snapshot; otherwise `Error { "timed out" }` if there is no prior snapshot.

## What the rest of the crate needs, at a glance (not structs, just services)

- **spawner**: `Command` → child process → captures stdout/stderr, enforces 30 s timeout.
- **parser**: bytes → `Vec<UsageRecord>` / `CostRecord`, tolerant of concatenated arrays and of missing optional fields.
- **merger**: latest `UsageRecord` + `CostRecord` per provider → `ProviderSnapshot`.
- **scheduler**: one `tokio::time::interval` per (provider, command).
- **renderer**: `AppState` → `ratatui` widgets. Omarchy theme pulled from `~/.config/omarchy/current/theme/` at startup.
- **config**: our own `~/.config/codexbar-tui/config.toml` with provider toggles + refresh intervals. We do NOT edit `~/.codexbar/config.json`.

## Things I want to confirm before coding

1. Whether `usage.primary.resetsAt` ever appears for Claude (not in our sample).
2. What Codex's `usage` JSON looks like once `~/.codex/auth.json` exists.
3. Whether `status.indicator` has values beyond `"none"` in the wild (we've only seen clean).
4. Whether the concatenated-arrays framing changes in a future codexbar release — if so, simplify the parser.
5. Whether there's a way to get codexbar to emit a single unified document across providers (probably not; the upstream design is "one query, multiple providers in parallel, one array per provider").
