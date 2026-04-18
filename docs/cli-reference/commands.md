# codexbar CLI — subcommand reference

Captured from `codexbar` **v0.20** (Linux x86_64, CodexBarCLI tarball) running on Arch (omarchy). Every subcommand's `--help` output is captured verbatim in `help-*.txt`; this file is the digest. `--version` prints only `CodexBar` (no version string) — the `v0.20` label comes from the GitHub release tag and the embedded Claude version `2.1.114` that shows up in the JSON payloads. `codexbar -V` behaves the same.

## Subcommand tree

```
codexbar
├── usage                    # live quota from providers (default action)
├── cost                     # local token-cost rollup from on-disk logs
└── config
    ├── validate             # lint ~/.codexbar/config.json (default)
    └── dump                 # print normalized config to stdout
```

### Routing quirks

- Bare `codexbar` runs the `usage` subcommand with `--provider all --source auto`.
- `codexbar --help`, `codexbar help`, `codexbar <anything-else> --help` all print the **`usage`** help text, not a top-level help. The real way to get per-subcommand help is `codexbar <sub> --help`.
- `codexbar zzzfakecmd` errors with `Unknown command 'zzzfakecmd'` and exits non-zero — so the binary does know the difference between a valid subcommand and junk, it just uses `usage` help as the fallback text for `--help`.

## `codexbar usage`

Fetches live quota for one or more providers.

```
codexbar usage [--format text|json] [--json] [--json-only] [--json-output]
               [--log-level ...] [-v|--verbose]
               [--provider codex|claude|cursor|opencode|opencodego|alibaba-coding-plan|
                           factory|gemini|antigravity|copilot|zai|minimax|kimi|kilo|kiro|
                           vertexai|augment|jetbrains|kimik2|amp|ollama|synthetic|warp|
                           openrouter|perplexity|both|all]
               [--account <label>] [--account-index <index>] [--all-accounts]
               [--no-credits] [--no-color] [--pretty] [--status]
               [--source <auto|web|cli|oauth|api>]
               [--web-timeout <seconds>] [--web-debug-dump-html]
               [--antigravity-plan-debug] [--augment-debug]
```

Key flags:

| Flag | Effect |
|---|---|
| `--provider <id>` | One of the supported provider IDs (note: `alibaba-coding-plan` on the CLI maps to `alibaba` in the config dump). `both` = codex+claude; `all` = every enabled provider in `~/.codexbar/config.json`. |
| `--source <mode>` | `auto` (default; macOS-only for most providers), `web` (macOS only), `cli`, `oauth`, `api`. On Linux we must force `cli`/`api`/`oauth` — see `linux-caveats.md`. |
| `--format json` / `--json` | Machine-readable output on stdout. Equivalent flags. |
| `--pretty` | Human-formatted JSON (indent 2). Otherwise single-line. |
| `--status` | Adds a `status` object with the provider's public status-page verdict. |
| `--account` / `--account-index` / `--all-accounts` | Multi-token selection. Requires a single `--provider`. |
| `--no-credits` | Drops the `credits` block from Codex output (untested here — no Codex auth). |
| `--web-timeout <s>` | Timeout for web cookie fetches. Irrelevant on Linux (web is unsupported). |

## `codexbar cost`

```
codexbar cost [--format text|json] [--json] [--json-only] [--json-output]
              [--log-level ...] [-v|--verbose]
              [--provider codex|claude|...|both|all]
              [--no-color] [--pretty] [--refresh]
```

- Reads local JSONL logs (`~/.claude/projects/**/*.jsonl` on Linux, plus `~/.codex/sessions/YYYY/MM/DD/*.jsonl` and archived sessions). No network or CLI launch needed.
- Upstream docs claim "`--refresh` forces rescan, otherwise uses cached scan results". In practice (see `timings.md`) both variants take ~15 s on this machine — the scan appears to run every call.
- Does **not** accept `--source`, `--account`, `--all-accounts`. Flags advertised by the upstream docs that do not exist here: `--account*`, `--source`, `--status`.

## `codexbar config`

```
codexbar config validate [--format text|json] [--pretty] [--json-only] [--json-output] [-v]
codexbar config dump     [--format text|json] [--pretty] [--json-only] [--json-output] [-v]
```

- `validate` — lints `~/.codexbar/config.json`. JSON output is an empty array (`[]`) when clean. Warnings keep exit 0; errors exit non-zero (per upstream docs, unverified here because our config is clean).
- `dump` — prints normalized config. Contents are provider enable flags plus a `version` int. **No secrets** in this file on our box; see the redaction section below.

## Global flags (every subcommand)

```
-h, --help        Show (the usage subcommand's) help
-V, --version     Prints "CodexBar" — no version number
-v, --verbose     Enable verbose logging to stderr
--no-color        Disable ANSI colors in text output
--log-level <trace|verbose|debug|info|warning|error|critical>
--json-output     Emit machine-readable logs (JSONL) on stderr
--json-only       Suppress non-JSON output; errors become JSON payloads
```

## Exit codes (from upstream `docs/cli.md`)

| Code | Meaning |
|---|---|
| 0 | success |
| 1 | unexpected failure (also: per-provider runtime/provider error when stdout still has JSON) |
| 2 | provider missing (binary not on PATH) |
| 3 | parse/format error |
| 4 | CLI timeout |

Observed on Linux with this build: `usage --provider <any> --source auto` → exit 1 with a JSON error payload; `usage --provider claude --source cli` → exit 0 with data; `cost ...` → exit 0 even when `daily` is empty.

## Providers that exist in the `--provider` flag but not in the default config dump

The `--provider` flag allows `alibaba-coding-plan`, but `config dump` lists the same provider as `alibaba`. The config dump also contains no entry for `abacus` even though upstream docs cover it, and none for `droid` (upstream release notes mention Factory/Droid in the same breath). Treat the flag list as authoritative for routing; treat the dump list as authoritative for "which providers this installation ships enable flags for."

## Redaction notes for committed outputs

The captured `config-dump.txt` / `config-dump-pretty.txt` on this box contains only provider-enable flags and `version: 1`. No `tokenAccounts`, no cookies, no API keys. **No redaction performed or needed.** If a future contributor has configured `~/.codexbar/config.json` with token accounts, they MUST strip the `tokenAccounts` array (and any `apiKey` / `cookie` / `sessionKey` fields) before committing refreshed output.
