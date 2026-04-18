# JSON output schema (observed on Linux, codexbar v0.20)

All fields here are **observed** from the captured outputs in this directory. Where upstream docs claim a field but we could not produce it on this box (no Codex auth, no browser cookies on Linux), the field is flagged **`[docs-only]`**. Do not hard-code absence — treat every non-primary field as optional.

## Top-level framing quirk (IMPORTANT)

`codexbar usage` does **not** emit a single JSON document. It emits **one `[ ... ]` array per provider attempt, concatenated, with no separator**. Example (from `usage-codex-cli.json`, reformatted to show the boundary):

```
[ { "error": {...}, "provider": "codex", "source": "cli" } ]
[ { "error": {...}, "provider": "cli",   "source": "cli" } ]
```

Stock `serde_json::from_slice` will choke on this. The parser must use a streaming deserializer (`serde_json::Deserializer::from_slice(buf).into_iter::<Vec<Record>>()`) and flatten.

The secondary `"provider":"cli","source":"cli"` block with `"message":"Error"` appears every time a Codex attempt is made, even when `--provider codex --source cli` is the only thing requested. It looks like a spurious fallback emission (possibly a bug in v0.20). Treat it as: **if a record has `provider == "cli"` and the only field is `error`, discard it.**

`codexbar cost` emits a single well-formed array. `codexbar config dump` emits a single well-formed object. `config validate` emits `[]` on success.

## `usage` record (success case)

Captured shape (`usage-claude-cli.json` for the no-status case, `usage-claude-status.json` for the `--status` case; both reformatted below). Every example value in the table is **observed** unless explicitly marked "inferred".

```json
{
  "provider": "claude",
  "source":   "claude",
  "version":  "2.1.114",
  "usage": {
    "identity":  { "providerID": "claude" },
    "primary":   { "usedPercent": 45, "windowMinutes": 300 },
    "secondary": {
      "usedPercent": 15,
      "windowMinutes": 10080,
      "resetsAt": "2026-04-23T19:00:00Z",
      "resetDescription": "ResetsApr23,9pm(Europe/Brussels)"
    },
    "tertiary":  { "usedPercent": 0, "windowMinutes": 10080 },
    "updatedAt": "2026-04-18T17:14:36Z"
  },
  "status": {                                      /* present only with --status */
    "indicator":   "none",
    "description": "All Systems Operational",
    "url":         "https://status.claude.com/",
    "updatedAt":   "2026-04-18T15:33:48Z"
  }
}
```

### Field-by-field

| Path | Type | Nullable | Units | Example | Meaning |
|---|---|---|---|---|---|
| `provider` | string | no | enum (see commands.md) | `"claude"` | Provider ID, matches `--provider` input |
| `source` | string | no | enum: `"claude"`, `"cli"`, `"codex"`, `"openai-web"`, `"web"`, `"oauth"`, `"api"`, `"auto"`, `"local"`, `"cli"` | `"claude"` | Which source path was actually used. Note: the `claude` PTY fallback reports back as `"source": "claude"` (not `"cli"`). |
| `version` | string | no | semver-ish | `"2.1.114"` | Embedded version of the upstream tool used (here: `claude` CLI version) |
| `usage.identity.providerID` | string | no | — | `"claude"` | Redundant with top-level `provider`; for Codex this carries account labels when multi-account is in play `[docs-only]` |
| `usage.primary.usedPercent` | integer | **yes** (missing if exhausted/unknown) | percent 0–100 | `45` | Session-window consumption |
| `usage.primary.windowMinutes` | integer | no | minutes | `300` | 5-hour session window |
| `usage.primary.resetsAt` | string (RFC3339 UTC) | yes | ISO timestamp | not present in our sample | Absolute reset time for primary window |
| `usage.primary.resetDescription` | string | yes | free text | absent in our sample | Human-formatted reset (note the whitespace-stripped formatting in our secondary sample: `"ResetsApr23,9pm(Europe/Brussels)"`) |
| `usage.secondary.*` | same shape as primary | — | — | — | Weekly window (here `windowMinutes: 10080` = 7 d). Claude weekly reset is represented as **both** `resetsAt` (absolute RFC3339) **and** `resetDescription` (localized text). |
| `usage.tertiary.*` | same shape as primary | — | — | — | Secondary weekly bucket (Claude uses this for Opus-specific weekly counter; `usedPercent: 0` here because we're not on Opus this week) |
| `usage.updatedAt` | string (RFC3339 UTC) | no | ISO timestamp | `"2026-04-18T17:14:36Z"` | When codexbar last refreshed this record |
| `status.indicator` | string | yes (only with `--status`) | **observed:** `"none"`. **inferred from statuspage.io convention** (not observed on this box): `"minor"`, `"major"`, `"critical"`. | `"none"` | Provider statuspage health |
| `status.description` | string | yes | — | `"All Systems Operational"` | Statuspage headline |
| `status.url` | string | yes | URL | `"https://status.claude.com/"` | Link the TUI can expose |
| `status.updatedAt` | string (RFC3339 UTC) | yes | ISO timestamp | `"2026-04-18T15:33:48Z"` | Last statuspage poll time (verified: appears in `usage-claude-status.json`) |

### `usage.primary` / `secondary` / `tertiary` — semantic mapping

Observed for Claude CLI source:

| Slot | `windowMinutes` | Maps to |
|---|---|---|
| `primary` | 300 | 5-hour session window |
| `secondary` | 10080 | Weekly all-models window (with `resetsAt` + `resetDescription`) |
| `tertiary` | 10080 | Weekly Opus-specific window |

Upstream per-provider docs suggest Codex uses `primary=session`, `secondary=weekly`, possibly no `tertiary`. This is `[docs-only]` for us until we have a working Codex login.

### Reset-time representation

- **Absolute timestamp**: `usage.secondary.resetsAt` — RFC3339, UTC (`Z`), e.g. `"2026-04-23T19:00:00Z"`. Only populated when the provider exposes one.
- **Seconds-remaining**: **not present** in observed JSON. The TUI must compute `resetsAt - now` itself.
- **Free-text**: `usage.secondary.resetDescription` — already localized to `Europe/Brussels` in our sample. The whitespace is stripped (`"ResetsApr23,9pm(Europe/Brussels)"`); do not attempt to parse it.
- `primary` / `tertiary` in the captured Claude output have **no `resetsAt`** (missing keys) — the TUI must render these as "-- left" or similar.

### Exhausted / null / unavailable states

1. **Missing key** = unknown / not applicable. Example: `usage.primary.resetsAt` is absent for Claude session windows. Model it as `Option<T>`.
2. **`usedPercent` absent** — occurs when a provider reports quota not applicable (e.g. unlimited tier). Unconfirmed here; upstream docs call it out.
3. **Hard exhaustion** (`usedPercent == 100`) is just a number — the provider may or may not include a later `resetsAt`. Don't treat "100" as a separate state, but treat "`usedPercent` missing AND `resetsAt` in the past" as a hint that the provider is in an error state.

## `usage` record (error case)

Captured shape (`usage-all.json`, every provider on Linux with `--source auto`):

```json
{
  "provider": "codex",
  "source":   "auto",
  "error": {
    "code":    1,
    "kind":    "runtime",
    "message": "Error: selected source requires web support and is only supported on macOS."
  }
}
```

### Field-by-field

| Path | Type | Nullable | Meaning |
|---|---|---|---|
| `provider` | string | no | Same enum as success case |
| `source` | string | no | The source that was attempted |
| `error.code` | integer | no | Non-zero numeric code. Same space as the process exit code but per-provider. |
| `error.kind` | string | no | Observed values: `"runtime"` (platform not supported), `"provider"` (provider-side auth/connection failure). Upstream likely has more. |
| `error.message` | string | no | Human-readable. Sometimes generic (`"Error"`) — don't assume it's parseable. |

When the whole array element has `"error"`, the `"usage"` / `"status"` / `"version"` keys are **absent entirely** (not null). Deserialize with `#[serde(default)]` or make them `Option`.

### The spurious `"provider":"cli","source":"cli","message":"Error"` record

Every `--provider codex*` invocation on this Linux box also emitted a second top-level array containing:

```json
{ "provider": "cli", "source": "cli", "error": { "code": 1, "kind": "provider", "message": "Error" } }
```

This record is useless for the TUI. Filter with the rule above. If this gets fixed upstream, the filter is a no-op.

## `cost` record

Captured shape (`cost-claude.json`, trimmed):

```json
{
  "provider":  "claude",
  "source":    "local",
  "updatedAt": "2026-04-18T17:23:59Z",
  "daily": [
    {
      "date": "2026-03-20",
      "inputTokens":          176730,
      "outputTokens":         4550827,
      "cacheCreationTokens":  35549636,
      "cacheReadTokens":      434382496,
      "totalTokens":          474659689,
      "totalCost":            194.7024141,
      "modelsUsed": [ "claude-haiku-4-5", "claude-opus-4-6", "claude-sonnet-4-6" ],
      "modelBreakdowns": [
        { "modelName": "claude-haiku-4-5", "totalTokens": 342092738, "cost": 84.76249325 },
        { "modelName": "claude-opus-4-6",  "totalTokens": 104309889, "cost": 79.5223045 },
        { "modelName": "claude-sonnet-4-6","totalTokens": 28257062,  "cost": 30.41761635 }
      ]
    }
  ]
}
```

### Field-by-field

| Path | Type | Nullable | Units | Meaning |
|---|---|---|---|---|
| `provider` | string | no | enum | Same as usage |
| `source` | string | no | `"local"` only (observed) | `cost` never uses web/CLI; `"local"` means "scanned from your JSONL logs" |
| `updatedAt` | string (RFC3339) | no | UTC ISO | When the scan completed |
| `daily` | array | no (may be empty) | — | One entry per calendar day with activity. **Not densely filled** — days without activity are simply omitted (verified: gaps exist in our claude output). |
| `daily[].date` | string | no | `YYYY-MM-DD` **local-date** | Day of activity in the **local timezone** at the moment of invocation (honors `$TZ`). See "Cost date timezone" below for the proof. |
| `daily[].inputTokens` | integer | no | tokens | Regular input tokens (excl. cache) |
| `daily[].outputTokens` | integer | no | tokens | Regular output tokens |
| `daily[].cacheCreationTokens` | integer | no | tokens | Prompt-cache writes |
| `daily[].cacheReadTokens` | integer | no | tokens | Prompt-cache hits |
| `daily[].totalTokens` | integer | no | tokens | Sum of the four above |
| `daily[].totalCost` | number (f64) | no | USD | Sum of `modelBreakdowns[].cost` |
| `daily[].modelsUsed` | array of string | no | — | Unique list of model names that day |
| `daily[].modelBreakdowns[]` | array of object | no | — | Per-model rollup for that day |
| `daily[].modelBreakdowns[].modelName` | string | no | — | e.g. `"claude-opus-4-6"` |
| `daily[].modelBreakdowns[].totalTokens` | integer | no | tokens | Per-model token total |
| `daily[].modelBreakdowns[].cost` | number (f64) | no | USD | Per-model cost |

### Upstream-documented but not observed

Upstream `docs/cli.md` also advertises:

- `sessionTokens`, `sessionCostUSD`
- `last30DaysTokens`, `last30DaysCostUSD`
- `totals: { inputTokens, outputTokens, ..., totalCost }`

None of these appeared in our `cost-claude.json` or `cost-codex.json` at v0.20. They may be provider-conditional, or may have been removed/renamed. Treat them as optional in the Rust model and log a warning if they appear, so we catch the change.

### Cost date timezone (verified, not inferred)

`cost.daily[].date` is bucketed by the **process's local timezone**, resolved the standard way (`$TZ` env var → `/etc/localtime`). Not UTC, not the Claude/Codex server clock, not a fixed day-rollover time.

Verification (run 2026-04-18 on a `Europe/Brussels` host):

```
# Same command, three timezones, diff'd buckets:
TZ=UTC               codexbar cost --provider claude --format json
TZ=Europe/Brussels   codexbar cost --provider claude --format json
TZ=Pacific/Auckland  codexbar cost --provider claude --format json
```

Result (last 3 buckets, `totalTokens`):

| Bucket date | TZ=UTC | TZ=Europe/Brussels | TZ=Pacific/Auckland |
|---|---|---|---|
| `2026-04-16` | 281,329,667 | 282,805,683 | _(shifted; this bucket is `2026-04-17` on Auckland)_ |
| `2026-04-17` | 181,448,983 | 135,285,061 | 281,329,667 (label: `2026-04-17`) |
| `2026-04-18` | 77,329,984 | 123,493,906 | 181,448,983 (label: `2026-04-18`) |
| `2026-04-19` | _(not yet)_ | _(not yet)_ | 77,329,984 (label: `2026-04-19`) |

Auckland is UTC+12/13. A bucket that sums to 77,329,984 lands on `2026-04-18` in both UTC and Brussels (boundaries ≤ 2 h apart), but on `2026-04-19` in Auckland — proving the bucket is picked by local-calendar-day, not by any fixed anchor. UTC vs Brussels buckets also differ in **token totals** per bucket (e.g. `2026-04-17` is 181 M tokens in UTC vs 135 M in Brussels) because the 22:00Z→00:00Z local-midnight window moves tokens across the date line.

**Implication for the TUI:** if we want "today" semantics, we do nothing — just look up `today's local date`. If we ever want UTC-normalized rollups, we must set `TZ=UTC` in the child process's env before spawning.

### Cost: exhausted / empty state

Codex path on this box has no sessions → `cost-codex.json` is:

```json
{ "daily": [], "provider": "codex", "source": "local", "updatedAt": "2026-04-18T17:23:59Z" }
```

`daily: []` + no totals block = "nothing to show." The TUI should render "no local cost data for <provider>".

## `config dump` payload

```json
{
  "version": 1,
  "providers": [
    { "id": "codex",  "enabled": true  },
    { "id": "claude", "enabled": false },
    /* ...each provider, in fixed order... */
  ]
}
```

| Path | Type | Nullable | Meaning |
|---|---|---|---|
| `version` | integer | no | Schema version (currently `1`) |
| `providers[].id` | string | no | Provider ID — authoritative list for which providers this install knows about |
| `providers[].enabled` | bool | no | Whether `--provider all` includes this one |

Provider-level token accounts are not dumped on a fresh install. If a user has configured `tokenAccounts`, they **will** appear in dump output and must be redacted before sharing.

## `config validate` payload

Success: `[]` (empty array).
Error: upstream docs say a non-empty array with warning/error records. Not observed on this box — our config is the default.

## Units / numeric precision summary

| Category | Type in Rust |
|---|---|
| Percent (`usedPercent`) | `u8` (0–100) |
| Window length (`windowMinutes`) | `u32` (minutes) |
| Absolute timestamp (`resetsAt`, `updatedAt`) | `chrono::DateTime<Utc>` (parse with RFC3339) |
| Dates (`daily[].date`) | `chrono::NaiveDate` |
| Token counts | `u64` (Claude numbers clear 400 M/day; u32 is not safe) |
| Costs (`cost`, `totalCost`) | `f64` (they are already f64 in the JSON; do not convert to cents — Claude's numbers have 9 decimal places like `84.76249325`) |
| Error codes | `i32` |
