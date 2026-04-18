# codexbar-tui

Thin Rust TUI on top of the upstream `codexbar` CLI (steipete/CodexBar). Reads its JSON output and renders it in the terminal, Omarchy-themed. We do not reimplement its backend.

- Project: `~/projects/codexbar-tui/`
- AUR package: `~/aur-repos/codexbar-tui/`

The upstream `codexbar` binary must be on `$PATH` at runtime. The AUR package declares that dependency.

Provider support is dynamic: the TUI calls `codexbar config dump` at startup and renders a panel per listed provider in that order. Do NOT hardcode provider IDs in the renderer or scheduler.

The TUI deliberately ignores codexbar's per-provider `enabled` flag. That flag is a macOS-GUI convention (CodexBar.app has menu-bar checkboxes that toggle it) and defaults to `false` on fresh Linux installs, so filtering on it would show nothing. Provider visibility on Linux is controlled by three things, in order:

1. **Is codexbar listing the provider at all?** If `codexbar config dump` doesn't emit an entry for it, we can't show it.
2. **Is it skipped as Linux-web-only?** `src/providers.rs::LINUX_WEB_ONLY` lists providers whose only v0.20 source mode is `web`, which is macOS-gated — we drop those at startup so we don't spawn workers that can only return errors. Remove an ID from this list if upstream adds a non-web source for it.
3. **Did the user hide it?** `hidden_providers = [...]` in `~/.config/codexbar-tui/config.toml` is a case-insensitive denylist applied after the Linux filter.

Authentication state is **not** part of visibility. If a provider is listed and usable on Linux, it always gets a panel; the panel shows `AuthMissing` / `Error` / `NotSupportedOnLinux` health when the corresponding subprocess call fails.