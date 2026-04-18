# codexbar-tui

Thin Rust TUI on top of the upstream `codexbar` CLI (steipete/CodexBar). Reads its JSON output and renders it in the terminal, Omarchy-themed. We do not reimplement its backend.

- Project: `~/projects/codexbar-tui/`
- AUR package: `~/aur-repos/codexbar-tui/`

The upstream `codexbar` binary must be on `$PATH` at runtime. The AUR package declares that dependency.

Provider support is dynamic: the TUI calls `codexbar config dump` at startup and renders a panel per enabled provider in that order. Do NOT hardcode provider IDs in the renderer or scheduler; users hide specific providers via a `hidden_providers = [...]` denylist in `~/.config/codexbar-tui/config.toml`.