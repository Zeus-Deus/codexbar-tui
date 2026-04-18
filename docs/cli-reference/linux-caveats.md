# Linux caveats for codexbar v0.20

This file is the "what actually works on Linux with nothing but `~/.codex` and `~/.claude` CLI auth" survey. Sourced from:

- Captured outputs in this directory.
- `codexbar --help` output on this host.
- The per-provider docs under `https://github.com/steipete/CodexBar/tree/main/docs/` (read via web, not cloned).

## Binary-level caveats (before you pick a provider)

### Missing shared library on modern Arch

The shipped `CodexBarCLI` binary is dynamically linked against **`libxml2.so.2`** (libxml2 ABI 2.x), but mainline Arch has moved to `libxml2` ABI 2.16 (`libxml2.so.16`). Out of the box you get:

```
codexbar: error while loading shared libraries: libxml2.so.2: cannot open shared object file: No such file or directory
```

**Fix on Arch:** `pacman -S libxml2-legacy` (in `extra/`). That package installs `/usr/lib/libxml2.so.2` on the default loader path — no wrapper or `LD_LIBRARY_PATH` needed once it is installed. For this audit we could not run `sudo pacman` non-interactively, so we shipped `~/.local/bin/codexbar` as a wrapper that sets `LD_LIBRARY_PATH=~/.local/lib/codexbar` with `libxml2.so.2` extracted from the package; that wrapper is **audit-only** and must not ship in the AUR PKGBUILD. The real AUR package declares `depends=('libxml2-legacy')` and installs the upstream binary directly.

### `--version` does not print a version

`codexbar --version` prints `CodexBar` — no number. The actual version comes from:

- The GitHub release tag (what we installed was `v0.20`).
- The embedded upstream-CLI version surfaced in JSON (`"version": "2.1.114"` is the `claude` CLI version, not codexbar's).

The TUI can't rely on `--version` to feature-flag against codexbar behavior. Pin to a known release in the AUR package and treat whatever ships as opaque.

### `--source auto` is macOS-only

Every provider default source path (`auto`) falls back to a web-cookie scraper on macOS only. On Linux it errors immediately:

```
Error: selected source requires web support and is only supported on macOS.
```

(verified on every provider we attempted). This is the headline Linux constraint — the TUI must always pass `--source cli` (or `--source api`/`--source oauth` depending on provider).

### `--source web` on Linux: unsupported, period

Upstream `docs/webkit.md` confirms the web path uses **macOS WKWebView + Safari/Chrome/Firefox cookie stores at `~/Library/...`** — there is no Linux Chrome (`~/.config/google-chrome/`) or Firefox (`~/.mozilla/firefox/`) cookie implementation in v0.20. Do not present `--source web` to the user on Linux.

## Provider-by-provider matrix (Linux, CLI-only auth)

Legend:

- **✅ Linux native** — works on this machine with just `~/.codex` or `~/.claude` style CLI auth.
- **🟡 Linux with env var / API key** — works if the user exports a key or writes one to `~/.codexbar/config.json`.
- **🔴 Browser cookies required** — upstream reads cookies from Linux browser paths. Unverified whether cookie paths are hardcoded to macOS in v0.20 (see note below).
- **⛔ macOS-only in v0.20** — uses Keychain, Safari, or WKWebView and has no Linux code path.

| Provider | Status | How it authenticates | Usable source modes on Linux | Notes |
|---|---|---|---|---|
| Claude | ✅ | `~/.claude/.credentials.json` + `claude` CLI on PATH (PTY `/usage` scrape) | `cli` (confirmed), `oauth` (plausible via the file fallback — untested) | Verified working on this host. `cost` subcommand scans `~/.claude/projects/**/*.jsonl` — no auth needed beyond filesystem read. |
| Codex | ✅* | `~/.codex/auth.json` (OAuth tokens) + `codex` CLI on PATH (RPC/PTY) | `cli` | *Not verified end-to-end on this host because `~/.codex/auth.json` is missing (user never ran `codex login`). Capture `usage-codex-cli.json` shows the auth-missing error: `"Codex connection failed: codex account authentication required to read rate limits"`. Once the user logs into Codex CLI, this should go green. |
| Gemini | 🟡 | Gemini CLI's own OAuth credentials (its cache dir) | `api` | Docs say CodexBar reuses the Gemini CLI's stored token. Needs `gemini` CLI installed + logged in. |
| z.ai | 🟡 | `Z_AI_API_KEY` env var | `api` | Zero-auth-file. |
| Warp | 🟡 | `WARP_API_KEY` / `WARP_TOKEN` env var | `api` | — |
| OpenRouter | 🟡 | `OPENROUTER_API_KEY` env var or `~/.codexbar/config.json` | `api` | — |
| Kimi K2 | 🟡 | `KIMI_K2_API_KEY` / `KIMI_API_KEY` env var | `api` | — |
| Copilot | 🟡 | GitHub device-flow token | `api` | The token acquisition flow is upstream's responsibility; user just needs to have logged in once. |
| Kilo | 🟡 | `KILO_API_KEY` or `~/.local/share/kilo/auth.json` | `api`, `cli` | — |
| Kiro | 🟡 | `kiro-cli` installed + AWS Builder-ID login | `cli` | Requires extra binary. |
| Vertex AI | 🟡 | `gcloud auth application-default login` (ADC file at `~/.config/gcloud/application_default_credentials.json`) | `oauth` | Scans `~/.claude/projects/` as well. |
| JetBrains AI | 🟡 | XML config from local IDE install (`~/.config/JetBrains/...AIAssistantQuotaManager2.xml`) | `local` | Needs an installed JetBrains IDE. |
| Antigravity | 🟡 | Local HTTPS language server on localhost | `local` | Standalone, no remote auth. |
| Ollama (ollama.com account) | 🔴 | Browser cookies from `ollama.com` | `web` | ⚠️ Verify before offering: upstream doc's cookie-path examples are macOS (`~/Library/...`); whether v0.20 reads Linux Chrome/Firefox (`~/.config/google-chrome/*/Cookies`, `~/.mozilla/firefox/*/cookies.sqlite`) is **not confirmed by our smoke tests**. On this Linux host `--source web` is blocked at the runtime level with "macOS only", so in practice: ⛔ until upstream ships a Linux WebKit alternative. |
| Cursor | 🔴 → ⛔ | `cursor.com`/`cursor.sh` cookies | `web` | Same story: web source is macOS-blocked in v0.20. |
| OpenCode / OpenCode Go | 🔴 → ⛔ | `opencode.ai` cookies | `web` | Same. |
| Amp | 🔴 → ⛔ | `ampcode.com` cookies | `web` | Same. |
| Abacus AI | 🔴 → ⛔ | `abacus.ai` cookies | `web` | Same. |
| Perplexity | 🔴 → ⛔ | Browser cookies, manual fallback | `web` | Same. |
| MiniMax | 🟡 | `MINIMAX_COOKIE_HEADER` env var | `web`\* | Env-var cookie header means no browser needed; still goes through the "web" code path which is macOS-gated. Try but expect failure until upstream decouples env-var cookie headers from the WebKit runtime. |
| Kimi | 🟡 | `KIMI_AUTH_TOKEN` env var (JWT extracted from a cookie) | `api` | — |
| Alibaba Coding Plan | 🟡 | `ALIBABA_CODING_PLAN_API_KEY` env var or cookies | `api` | — |
| Factory/Droid | 🔴 → ⛔ | Browser cookies (or inferred env var) | `web` | Same macOS gate. |
| Jetbrains/Augment/Synthetic | 🟡 | Env-var / config file (mixed) | `api` | Minor providers — pursue only on request. |

**Summary for v1:** Claude ✅, Codex ✅ (once user logs in), Gemini / z.ai / Warp / OpenRouter / Copilot / Kimi K2 🟡 via API keys. Everything else is either niche or blocked by the macOS-only web source path.

## Browser cookie path reference (for future work if upstream adds Linux web support)

Stored here so that if/when we want to audit the `--source web` fix upstream, we can point at the correct Linux paths:

| Browser | Linux path |
|---|---|
| Chrome / Chromium | `~/.config/google-chrome/*/Cookies`, `~/.config/chromium/*/Cookies` (SQLite, encrypted with libsecret/kwallet) |
| Brave | `~/.config/BraveSoftware/Brave-Browser/*/Cookies` |
| Firefox | `~/.mozilla/firefox/*.default*/cookies.sqlite` |
| Vivaldi | `~/.config/vivaldi/*/Cookies` |

Whether these paths appear at all in the v0.20 binary is unverified here; what *is* verified is that `--source auto` / `--source web` on Linux all trip the runtime gate `"selected source requires web support and is only supported on macOS."` before any cookie path is consulted. So even if the paths are baked in, the code path is unreachable on Linux in v0.20. Do not promise web-source functionality on Linux; it simply is not there yet.

## Keychain / Secret-Service

Upstream stores some secrets in the macOS Keychain (`Claude Code-credentials`, `CodexBar-openai-cookies`, etc.). There is no libsecret / gnome-keyring / kwallet integration on Linux in v0.20. Everything that would otherwise read Keychain either falls back to a file on Linux (Claude) or simply does not work (Codex web, Cursor, etc.).

## Sandboxing / FHS notes

Running under a strict sandbox (bubblewrap, flatpak, etc.) the binary needs:

- `/home/$USER/.codex/` read access
- `/home/$USER/.claude/` read access
- `/home/$USER/.codexbar/` read+write access (config + cache)
- `/tmp/` write access
- PATH access to `codex` and `claude` CLIs
- `libxml2.so.2` (from `libxml2-legacy`) resolvable by the dynamic loader — Arch's package installs it at `/usr/lib/libxml2.so.2`, which the default loader path picks up automatically
- Network egress (only for web/api sources — cli/local do not need network)

The AUR package just needs `depends=('libxml2-legacy')` — no wrapper. The `LD_LIBRARY_PATH` dance we used during this audit was only because we couldn't `sudo pacman -S libxml2-legacy` non-interactively.
