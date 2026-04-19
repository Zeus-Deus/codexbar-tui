# codexbar-tui

Thin, Omarchy-themed terminal UI on top of the upstream [`codexbar`][upstream]
CLI. Reads its JSON output and renders a live usage / cost panel per provider
(Claude, Codex, Gemini, …) — exactly whichever providers `codexbar config dump`
reports.

<!-- screenshot goes here once we have one -->

## Install (Omarchy / Arch)

```bash
yay -S codexbar-tui        # or: paru -S codexbar-tui
codexbar-tui-setup-omarchy # wires up Super + Ctrl + U + floating window
```

That's it. Press **Super + Ctrl + U** to pop the TUI as a centered floating
window, same behaviour as Super + Ctrl + B for Bluetooth.

The setup script is idempotent — rerun it any time. It only touches two of
*your* Omarchy files, both inside marker-delimited blocks it owns:

- `~/.config/hypr/bindings.conf` &nbsp;→&nbsp; the `SUPER CTRL + U` bind
- `~/.config/hypr/windows.conf` &nbsp;→&nbsp; the float/center/size windowrule

Remove the marked blocks to uninstall the hotkey; `yay -R codexbar-tui`
removes the binary.

## Requirements

- Omarchy (tested on 3.5.x) or any Hyprland + `xdg-terminal-exec` setup.
- The upstream [`codexbar`][upstream] CLI on `$PATH` at runtime. We call
  `codexbar config dump` + `codexbar usage` + `codexbar cost` as subprocesses;
  we do not reimplement its backend. Install it per upstream's instructions.

## Run it without the hotkey

```bash
codexbar-tui
```

Keys inside the TUI: `q` quits, `r` forces a refresh, `a` toggles error /
not-supported panels.

## Config

Optional `~/.config/codexbar-tui/config.toml`:

```toml
# Providers listed here are hidden from the UI (case-insensitive).
# Useful for silencing one you don't use without uninstalling it upstream.
hidden_providers = ["gemini"]
```

Provider visibility on Linux follows three rules in order:

1. If `codexbar config dump` doesn't list a provider, we can't show it.
2. Providers whose only v0.20 source mode is `web` (macOS-gated) are
   skipped at startup.
3. Anything in `hidden_providers` is hidden.

Authentication state is *not* part of visibility. If a provider is listed and
usable on Linux, it always gets a panel; the panel shows `AuthMissing` /
`Error` / `NotSupportedOnLinux` health when the corresponding subprocess
call fails.

## License

MIT. See [LICENSE](LICENSE).

[upstream]: https://github.com/steipete/CodexBar
