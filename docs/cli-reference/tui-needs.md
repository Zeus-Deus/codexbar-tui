# TUI v1 surface: which codexbar commands we actually invoke

## TL;DR

v1 shells out to **exactly two** commands, both with forced source flags:

1. `codexbar usage --provider <id> --source cli --format json` — the "now" panel (session + weekly bars, reset times).
2. `codexbar cost --provider <id> --format json` — the "history" panel (daily rollup + per-model cost).

Everything else (`--status`, `--account*`, other providers, web sources) is either unavailable on Linux with CLI-only auth (see `linux-caveats.md`) or pure nice-to-have that belongs in v2.

## Why this subset

### Things we *need*

- **Session 5-hour bar + weekly bar(s) + resets** → `usage` is the only subcommand that surfaces `primary`/`secondary`/`tertiary` + `resetsAt`. `cost` does not show quota at all.
- **Daily / per-model cost** → `cost` is the only subcommand that returns `daily[]` and `modelBreakdowns[]`. `usage` does not.

### Things we're intentionally skipping in v1

| Surface | Command we'd need | Why defer |
|---|---|---|
| Statuspage indicator | `usage --status` | Doesn't add Linux-only data; same 15 s call cost as plain `usage`. Keep as a v1 flag behind a config toggle. Fetch on a **separate**, slower schedule (hourly) if enabled. |
| Multi-account Claude/Codex | `usage --account ...` / `--all-accounts` | Requires a populated `~/.codexbar/config.json` `tokenAccounts` array that this host does not have. Defer until a user asks. |
| Providers other than Claude + Codex | `usage --provider cursor\|gemini\|...` | Each is its own auth/source ball of yarn (see `linux-caveats.md`). Ship v1 with Claude + Codex only; add provider plugins once the core works. |
| `config validate` / `config dump` | same | Useful for a "doctor" subcommand later. Not a live-view concern. |
| `--source web` anything | — | macOS-only. Hard-blocked on Linux. |
| `--source oauth` for Claude | `usage --provider claude --source oauth` | Requires a `user:profile`-scoped OAuth token written to `~/.claude/.credentials.json`. Our box has this file but we haven't tested the path. Treat as stretch goal. |

## Invocation contract v1

The TUI spawns codexbar in a background thread / async task; it never blocks the render loop on a codexbar call.

### Refresh cadence

Based on `timings.md` (`usage --source cli` and `cost` both take ~15 s cold and ~15 s warm — there is **no effective cache**):

| Command | Interval | Rationale |
|---|---|---|
| `usage --provider claude --source cli --format json` | **60 s** minimum; user-configurable to 30–300 s | 15 s cost + we don't want the `claude` CLI being launched into a PTY constantly. 60 s is polite. |
| `cost --provider claude --format json` | **5 min** | Cost data only changes when a day rolls or sessions complete; users don't need sub-minute updates of totals. |
| `usage --provider codex --source cli --format json` | same as claude, but per-provider toggle | Only if the user has `~/.codex/auth.json`. |
| `usage --status` (optional) | **1 hour** | Just a status-page shim; no point polling more often. |

Every refresh writes to a shared `Option<Snapshot>` the render layer reads. The renderer shows the last good snapshot plus an "age" badge (e.g. `3m ago`).

### Output handling

- **Parse both stdout AND non-zero exit as data.** Exit 1 still contains a JSON error record — the TUI should display the `error.message` in the bar, not crash.
- **Streaming parse** — use `serde_json::Deserializer::from_slice(...).into_iter::<Vec<Record>>()` to walk multiple concatenated arrays (see `schema.md` framing quirk).
- **Filter out the spurious `{"provider":"cli","source":"cli","message":"Error"}` record** emitted alongside Codex attempts.
- **Timeout** the child at ~30 s (2× observed p100). Kill and treat as a transient error — keep showing the previous snapshot.

### Subprocess flags we pass unconditionally

```
--format json          # machine-readable
--no-color             # no stray ANSI even though json path shouldn't emit any
--source cli           # usage only; ignored by `cost`
```

We do **not** pass `--pretty` — compact JSON is faster to parse and the TUI does its own formatting.

## Out of scope for v1 (documented so we don't drift)

- Writing to `~/.codexbar/config.json` (we never mutate upstream state).
- Interactive account switching.
- Anything that talks directly to provider APIs.
- Re-implementing any of codexbar's cookie/auth discovery.
