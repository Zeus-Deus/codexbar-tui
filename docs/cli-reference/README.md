# codexbar CLI reference (audit artifacts)

Everything in this directory is captured from the upstream `codexbar` CLI running on Linux x86_64 (Arch / omarchy), version tag `v0.20`. Nothing here is hand-authored backend data — it's all either invocation output or notes derived from invocation output + upstream docs read via web.

## What's here

| File | Origin |
|---|---|
| `help-root.txt` / `help-usage.txt` / `help-cost.txt` / `help-config.txt` | `codexbar [sub] --help` verbatim |
| `usage-all.json` | `codexbar usage --format json --pretty` (errors on Linux, captured for shape) |
| `usage-claude.json` | `codexbar usage --provider claude --format json --pretty` (errors: `--source auto` is macOS-only) |
| `usage-codex.json` | `codexbar usage --provider codex --format json --pretty` (errors: same) |
| `usage-claude-cli.json` | `codexbar usage --provider claude --source cli --format json --pretty` (**success**, the actual TUI path) |
| `usage-claude-status.json` | `codexbar usage --provider claude --source cli --status --format json --pretty` (the observed `status` block) |
| `ldd.txt` | `ldd ~/.local/bin/CodexBarCLI` output — input to `pkgbuild-deps.md` |
| `usage-codex-cli.json` | `codexbar usage --provider codex --source cli --format json --pretty` (auth-missing error) |
| `cost-claude.json` | `codexbar cost --provider claude --format json --pretty` (success, large) |
| `cost-codex.json` | `codexbar cost --provider codex --format json --pretty` (empty `daily[]` — no codex sessions here) |
| `config-dump.txt` | `codexbar config dump` (compact, committed) |
| `config-dump-pretty.txt` | `codexbar config dump --pretty` |
| `config-validate.json` | `codexbar config validate --format json --pretty` (empty `[]` — config is clean) |
| `commands.md` | Subcommand / flag reference, digested |
| `schema.md` | JSON shape reference with nullability + units |
| `timings.md` | Wall-clock measurements for the TUI polling-interval decision |
| `linux-caveats.md` | Per-provider Linux support matrix + `libxml2.so.2` / `--source auto` gotchas |
| `runtime-deps.md` | External CLIs and credential files codexbar itself needs at runtime |
| `pkgbuild-deps.md` | Shared-lib → Arch-package map + proposed `depends` / `optdepends` for a future PKGBUILD |
| `tui-needs.md` | Which commands the v1 TUI actually shells out to, and why the rest are skipped |
| `rust-data-model.sketch.md` | Pseudocode struct sketch for review before any Rust is written |

## Redaction

The captured `config-dump*` files on this box contain only provider-enable flags plus `version: 1`. No secrets. No redaction performed. If a contributor refreshes these outputs on a host with `tokenAccounts` configured in `~/.codexbar/config.json`, they **must** strip the `tokenAccounts` array (and any `apiKey` / `cookie` / `sessionKey` fields) before committing.

## Reproducing

Prereqs: `libxml2-legacy` installed from Arch `extra/` (provides `/usr/lib/libxml2.so.2`). `codexbar` on PATH (CLI tarball `CodexBarCLI-v0.20-linux-x86_64.tar.gz` from the GitHub releases).

```
# Fresh captures (run from this directory):
codexbar --help                                                        > help-root.txt
codexbar usage --help                                                  > help-usage.txt
codexbar cost --help                                                   > help-cost.txt
codexbar config --help                                                 > help-config.txt
codexbar usage --format json --pretty                                  > usage-all.json
codexbar usage --provider claude --format json --pretty                > usage-claude.json
codexbar usage --provider codex  --format json --pretty                > usage-codex.json
codexbar usage --provider claude --source cli --format json --pretty   > usage-claude-cli.json
codexbar usage --provider claude --source cli --status --format json --pretty > usage-claude-status.json
codexbar usage --provider codex  --source cli --format json --pretty   > usage-codex-cli.json
codexbar cost  --provider claude --format json --pretty                > cost-claude.json
codexbar cost  --provider codex  --format json --pretty                > cost-codex.json
codexbar config dump                                                   > config-dump.txt
codexbar config dump --pretty                                          > config-dump-pretty.txt
codexbar config validate --format json --pretty                        > config-validate.json
```

Exit code 1 is expected for every `usage-*.json` except the `*-cli.json` successes (auto source is macOS-only on Linux).
