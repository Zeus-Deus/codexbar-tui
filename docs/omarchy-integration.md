# Omarchy integration audit (scoped to 4 questions)

Sourced from the `omarchy-kb` MCP server (Omarchy docs + release notes) plus direct inspection of this host: Omarchy **3.5.1**, installed at `~/.local/share/omarchy/` with user configs under `~/.config/`.

## a) Default terminal emulator

**Alacritty.** From the Omarchy manual, "Terminal" page (v3.5.1):

> Alacritty is the default terminal for Omarchy. It's fast, beautiful, and compatible with even old computers. It does not, however, support native tabs, splits, or image rendering. If you use Tmux, you may not mind, but if not, we fully support Ghostty and Kitty as options as well. Pick your preference under Install > Terminal in the Omarchy menu.

Corroborated by a v3.2.3 release note: "Add Alacritty back as a default package to provide a fallback for systems that do not support Ghostty" — so for a window the v3.2-v3.5 line has bounced between Ghostty and Alacritty as default. 3.5.1 ships Alacritty.

**Font:** `JetBrainsMono Nerd Font`, both terminal and system (Omarchy manual, "Fonts" page). Nerd Font means we *can* safely use the full Nerd-Font Private Use Area glyph set (powerline chevrons, icons, etc.) in the TUI. We should still test-render in Ghostty and Kitty since users swap.

**Implications for the TUI layout:**
- Monospaced cell assumption holds across Alacritty / Ghostty / Kitty.
- Nerd Font glyphs (`U+E000`–`U+F8FF`, `U+F0000`+) are safe — but gate them behind a config flag for users who've swapped fonts. Fallback to ASCII.
- Unicode box-drawing (`U+2500`-range) is universally fine.
- Double-width emoji **is not** safe across Alacritty; prefer Nerd-Font pictograms over Unicode emoji for the quota bar accents.

## b) Is `libxml2-legacy` installed by default?

**No.** Verified on this Omarchy 3.5.1 install:

```
$ pacman -Q libxml2-legacy
error: package 'libxml2-legacy' was not found
$ pacman -Q libxml2
libxml2 2.15.2-1
```

Searching the Omarchy manual and release notes (via `omarchy-kb`) for `libxml2` returns no hits. Omarchy's install scripts under `~/.local/share/omarchy/install/` contain no reference to `libxml2-legacy`. So it is a genuine install-time addition users will incur.

**Actions for the TUI's README and AUR package:**

- `depends=('libxml2-legacy')` must stay in the PKGBUILD — it really is required; `pacman` will fetch it on install.
- README should call this out in the install notes: "codexbar (the upstream Swift CLI) is linked against `libxml2.so.2`. Installing `codexbar-tui` will pull in `libxml2-legacy` from `extra/`. No user action needed."
- Not a showstopper — `libxml2-legacy` is in `extra/` (first-party Arch repo), so `pacman -S` resolves it without AUR.

## c) Standard integration points for user-installed CLI/TUI tools

Four integration surfaces, all config-file based (Omarchy's philosophy is "dotfiles in `~/.config/`, source-of-truth in `~/.local/share/omarchy/`"). All four let a packaged tool register itself without mutating Omarchy's own source tree:

### 1. Hyprland keybinds → `~/.config/hypr/bindings.conf`

- File format: one `bindd = <mods>, <key>, <label>, exec, <command>` per binding.
- Pattern Omarchy users follow (observed on this host): third-party installers append a section delimited with a `# <pkg>-hotkey-managed (do not edit this line manually)` comment so the package owns its lines cleanly. Example currently in my file:
  ```
  # vexis-hotkey-managed (do not edit this line manually)
  bind = CTRL SHIFT, space, pass, class:^(vexis|org\.codemux\.vexis)$
  ```
- For our TUI: reserve e.g. `Super + Shift + U` ("Usage") and let the user opt in via the README — we don't want to touch their bindings file from the AUR post-install hook. The Omarchy menu's `Setup > Config > Hyprland` is the official edit path.
- The cheatsheet on `Super + K` enumerates all bindings, so any we add surfaces automatically.

### 2. Waybar modules → `~/.config/waybar/config.jsonc` + `~/.config/waybar/style.css`

- `config.jsonc` has `modules-left` / `modules-center` / `modules-right` arrays. Third-party packages register a `custom/<name>` module and reference it from one of those arrays.
- Convention on this host uses begin/end markers so a package can idempotently add/remove its slot:
  ```jsonc
  // vexis-waybar-managed — BEGIN
  ,"custom/vexis"
  // vexis-waybar-managed — END
  ```
- A minimal module definition (from the existing `custom/update` module):
  ```jsonc
  "custom/update": {
    "format": "",
    "exec": "omarchy-update-available",
    "on-click": "omarchy-launch-floating-terminal-with-presentation omarchy-update",
    "tooltip-format": "Omarchy update available",
    "signal": 7,
    "interval": 3600
  }
  ```
- **Future v2 hook (not v1):** ship a `codexbar-tui-waybar` helper script that prints a short Claude/Codex quota string (e.g. `CC 45% / CX 12%`) so users can add a `custom/codexbar-tui` module to Waybar. The script can trust that codexbar's JSON is available. **Not in scope for v1.** Documenting it so we know the hook exists.

### 3. Walker app launcher → `~/.local/share/applications/*.desktop`

- Walker (Omarchy's `wofi` replacement as of 1.6.0) indexes `~/.local/share/applications/` + `~/.local/share/omarchy/applications/` + `/usr/share/applications/`.
- For the TUI we install one XDG `.desktop` file in `/usr/share/applications/codexbar-tui.desktop` via the PKGBUILD:
  ```
  [Desktop Entry]
  Name=codexbar-tui
  Comment=Terminal UI for codexbar quota + cost
  Exec=alacritty -e codexbar-tui      # or `xdg-terminal-exec codexbar-tui`
  Terminal=false
  Type=Application
  Categories=Utility;System;
  Icon=utilities-terminal
  StartupNotify=false
  ```
- Using `xdg-terminal-exec` makes the launcher honor the user's `Install > Terminal` choice instead of hard-coding Alacritty. That is the Omarchy-idiomatic way — Omarchy's own `Super + Return` binding does exactly this.

### 4. Omarchy menu → *not a public extension point*

The Omarchy menu (`Super + Alt + Space`) is driven by `omarchy-menu` in `~/.local/share/omarchy/bin/` and its entries are baked into the script. There is no documented plugin or drop-in directory for third-party menu entries. Anything we want users to "find from the menu" has to go via Walker (`Super + Space`) or a Hyprland keybind instead.

### Summary

v1 ships: a desktop entry (Walker). v1.1+: an optional example block for `~/.config/hypr/bindings.conf` documented in the README. v2: a Waybar custom module helper script. No menu integration.

## d) `~/.config/omarchy/current/theme/colors.toml` schema

Sample from this host's active theme (headed by a large ASCII-art banner, which I'm omitting; skipping to the key-value region):

```toml
# Accent and UI colors
accent = "#6fb8e3"
active_border_color = "#f2fcff"
active_tab_background = "#6fb8e3"

# Cursor colors
cursor = "#f2fcff"

# Primary colors
foreground = "#d6e2ee"
background = "#1b2d40"

# Selection colors
selection_foreground = "#1b2d40"
selection_background = "#4d9ed3"

# Normal colors (ANSI 0-7)
color0  = "#1b2d40"
color1  = "#4d86b0"
color2  = "#5e95bc"
color3  = "#6fa4c9"
color4  = "#6fb8e3"
color5  = "#8bc9eb"
color6  = "#b4e4f6"
color7  = "#d6e2ee"

# Bright colors (ANSI 8-15)
color8  = "#4A6B80"
color9  = "#73a6cb"
color10 = "#86b7d8"
color11 = "#9dcae5"
color12 = "#f2fcff"
color13 = "#b1d8ee"
color14 = "#d1eef8"
color15 = "#ffffff"
```

### Top-level keys

All keys are at the root (no TOML sections), every value is a hex string `"#rrggbb"`:

| Key | Semantic |
|---|---|
| `accent` | Theme accent color (buttons, highlights, active glyphs) |
| `active_border_color` | Hyprland active-window border |
| `active_tab_background` | Active tab strip |
| `cursor` | Terminal cursor |
| `foreground` | Default text color |
| `background` | Default background |
| `selection_foreground` | Text color when selected |
| `selection_background` | Selection highlight |
| `color0` .. `color7` | ANSI normal palette (black, red, green, yellow, blue, magenta, cyan, white) |
| `color8` .. `color15` | ANSI bright palette (same order) |

Path: `~/.config/omarchy/current` is a symlink → `~/.local/share/omarchy/themes/<current-theme>/`. Re-themeing swaps the symlink target (via `Install > Style > Theme` / `Remove > Style > Theme` in the Omarchy menu).

### v1 stance

**v1 uses standard ANSI palette only** — we emit `color0`-`color15` semantically (e.g. "red = error", "yellow = warning", "cyan = session bar") and let the terminal resolve them from whichever Omarchy theme is active. No reading of `colors.toml` required.

### v2 plan (documented here so we don't re-derive it later)

When we want accent-aware rendering (e.g. progress-bar fill matching `accent`), parse `~/.config/omarchy/current/theme/colors.toml` at startup:

- Use a plain `toml::from_str::<HashMap<String, String>>(...)` — flat table, no nested sections.
- Resolve symlink first (`fs::canonicalize`) so we cache by real path and invalidate on theme switch.
- Watch for changes via `inotify`/`notify` on the symlink target directory if we want hot-reload; otherwise re-read on focus-in.
- Fall back gracefully: if the file is missing, or any key fails to parse as `#RRGGBB`, use ANSI defaults.
- Keys we'd actually use: `accent` (bar fill for the primary provider), `foreground`/`background` (always, but terminal handles these already), `color1` (error), `color3` (warning), `color2` (ok).

No file writing from the TUI ever. Themes are read-only for us.
