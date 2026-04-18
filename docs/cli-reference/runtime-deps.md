# codexbar runtime dependencies (external CLIs and credential files)

This file documents what must exist **at runtime** on the user's machine for `codexbar` to return data for each source mode / provider. It is sourced from the upstream per-provider docs at
`https://github.com/steipete/CodexBar/tree/main/docs/` (read via web, not cloned) plus observations from this audit.

codexbar itself is self-contained — it does not re-use `claude` / `codex` OAuth flows on its own. Instead, for `--source cli` it **shells out to the upstream CLI tool in a PTY** and scrapes the tool's own slash-command output. For `--source api` it reads the API key from env or `~/.codexbar/config.json` and calls provider REST endpoints directly.

## TUI-packaging summary (for the AUR PKGBUILD work later)

For the `codexbar-tui` package we need to propagate these as **`optdepends`** (user picks which they need):

```
optdepends=(
  'claude-code: Claude usage via `codexbar usage --provider claude --source cli`'
  'codex: Codex usage via `codexbar usage --provider codex --source cli`'
  'gemini-cli: Gemini usage via `codexbar usage --provider gemini`'
  'kiro-cli: Kiro usage (AUR)'
)
```

(Exact Arch package names TBD — `claude-code` on AUR, `codex` on AUR, `gemini-cli` on AUR, `kiro-cli` on AUR. Verify before writing the PKGBUILD.)

## Per-provider runtime matrix

| Provider | Source mode we'd use on Linux | External binary on PATH | Credential file read | Env vars |
|---|---|---|---|---|
| **Claude** | `cli` | **`claude`** (PTY-run for `/usage`, `/status`) | `~/.claude/.credentials.json` (for `--source oauth` fallback) | none |
| **Claude** | `oauth` | none (file-only) | `~/.claude/.credentials.json` (macOS Keychain on mac; file on Linux) | none |
| **Codex** | `cli` | **`codex`** (RPC server via `codex -s read-only -a untrusted app-server`, PTY fallback) | `~/.codex/auth.json` (OAuth tokens from `codex login`) | `$CODEX_HOME` overrides the directory |
| **Gemini** | `api` | **`gemini`** CLI (codexbar extracts OAuth client secret from `node_modules/@google/gemini-cli-core/dist/src/code_assist/oauth2.js` inside the installed CLI) | `~/.gemini/oauth_creds.json`, `~/.gemini/settings.json` (auth type must be `oauth-personal`) | none (direct API-key auth is explicitly unsupported) |
| **Copilot** | `api` | none (codexbar does its own device-flow login) | `~/.codexbar/config.json` (stores `apiKey`) | none |
| **Kiro** | `cli` | **`kiro-cli`** (logged in via AWS Builder ID) | none outside the CLI's own state | none |
| **Vertex AI** | `oauth` | optional `gcloud` for refresh | `~/.config/gcloud/application_default_credentials.json` (ADC) | `$GOOGLE_APPLICATION_CREDENTIALS` |
| **Kilo** | `api` or `cli` | `kilo-cli` (optional for CLI mode) | `~/.local/share/kilo/auth.json` (CLI fallback), `~/.codexbar/config.json` (API key) | `KILO_API_KEY` |
| **z.ai** | `api` | none | `~/.codexbar/config.json` (optional) | `Z_AI_API_KEY`, `Z_AI_API_HOST`, `Z_AI_QUOTA_URL` |
| **Warp** | `api` | none | `~/.codexbar/config.json` (optional) | `WARP_API_KEY`, `WARP_TOKEN` |
| **OpenRouter** | `api` | none | `~/.codexbar/config.json` (optional) | `OPENROUTER_API_KEY` |
| **Kimi K2** | `api` | none | none | `KIMI_K2_API_KEY`, `KIMI_API_KEY` |
| **Kimi** | `api` | none | none | `KIMI_AUTH_TOKEN` (JWT, extracted manually from cookie) |
| **Alibaba Coding Plan** | `api` | none | none | `ALIBABA_CODING_PLAN_API_KEY` |
| **MiniMax** | `web` (macOS-gated) | none | none | `MINIMAX_COOKIE_HEADER` (blocked on Linux until upstream decouples env-var cookies from WKWebView) |
| **JetBrains AI** | `local` | none (reads config file of an installed IDE) | `~/.config/JetBrains/*/options/AIAssistantQuotaManager2.xml` | none |
| **Antigravity** | `local` | none (talks to a localhost language-server) | none | none |
| **Cursor / OpenCode / OpenCode Go / Amp / Abacus / Perplexity / Factory-Droid / Ollama** | `web` | none usable on Linux | browser cookies | provider-specific cookie-override env vars | — these are **⛔ blocked in v0.20 on Linux** by the `selected source requires web support and is only supported on macOS` gate. |

## TUI v1 focus — the only runtime deps that matter

Given our v1 scope (Claude + Codex, `--source cli` only — see `tui-needs.md`):

| User wants… | Must have on PATH | Must have on disk |
|---|---|---|
| Claude quota | `claude` (Anthropic's official CLI) | `~/.claude/.credentials.json` (produced by `claude` login) |
| Codex quota | `codex` (OpenAI's official CLI) | `~/.codex/auth.json` (produced by `codex login`) |
| Claude cost history | *(none — scans JSONL directly)* | `~/.claude/projects/**/*.jsonl` (populated by using `claude` normally) |
| Codex cost history | *(none — scans JSONL directly)* | `~/.codex/sessions/**/*.jsonl` (populated by using `codex` normally) |

The TUI should detect missing CLIs / credential files at startup and render an actionable message (e.g. "`codex` CLI not found on PATH — run `pacman -S codex` and `codex login`") instead of just surfacing the `error.message` from codexbar's JSON.

## Things codexbar never shells out to, despite what you might expect

- **`gh` (GitHub CLI)** — codexbar does its own device flow for Copilot. No `gh` dependency.
- **`aws` (AWS CLI)** — only `kiro-cli` itself, which handles Builder-ID internally.
- **`python` / `pip`** — none of the providers use Python helpers.
- **`gnome-keyring` / `libsecret` / `kwallet`** — no Linux secret-store integration in v0.20. Everything Linux-side is file- or env-var-based.

## Sanity at startup

For the TUI's boot check, we should probe:

1. `codexbar --version` exits non-zero? → missing upstream binary. Hard fail.
2. `command -v claude` missing AND user enabled Claude? → warn, render "install `claude-code`".
3. `command -v codex` missing AND user enabled Codex? → warn, render "install `codex`".
4. `~/.claude/.credentials.json` missing AND Claude enabled? → warn, render "run `claude` once to log in".
5. `~/.codex/auth.json` missing AND Codex enabled? → warn, render "run `codex login`".

None of these checks rely on shelling out to codexbar itself — they're local filesystem / PATH probes, and they give the user a better error than codexbar's opaque `"message": "Error"`.
