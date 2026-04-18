# PKGBUILD dependency planning

Derived from `ldd ~/.local/bin/CodexBarCLI` (see `ldd.txt`) mapped to Arch packages via `pacman -Qo`. Exactly one library (`libxml2.so.2`) is not resolvable from stock Arch and needs the `libxml2-legacy` package.

No PKGBUILD is written yet — this file is the input to that future step.

## Shared libraries → Arch packages

| Library | Resolved path | Owning Arch package |
|---|---|---|
| `libcurl.so.4` | `/usr/lib/libcurl.so.4` | `curl` |
| `libxml2.so.2` | **not found on stock Arch** | **`libxml2-legacy`** (in `extra/`) |
| `libm.so.6` | `/usr/lib/libm.so.6` | `glibc` |
| `libstdc++.so.6` | `/usr/lib/libstdc++.so.6` | `libstdc++` (alias for `gcc-libs`) |
| `libsqlite3.so.0` | `/usr/lib/libsqlite3.so.0` | `sqlite` |
| `libgcc_s.so.1` | `/usr/lib/libgcc_s.so.1` | `libgcc` (alias for `gcc-libs`) |
| `libc.so.6` | `/usr/lib/libc.so.6` | `glibc` |
| `libnghttp3.so.9` | `/usr/lib/libnghttp3.so.9` | `libnghttp3` |
| `libngtcp2_crypto_ossl.so.0` | `/usr/lib/libngtcp2_crypto_ossl.so.0` | `libngtcp2` |
| `libngtcp2.so.16` | `/usr/lib/libngtcp2.so.16` | `libngtcp2` |
| `libnghttp2.so.14` | `/usr/lib/libnghttp2.so.14` | `libnghttp2` |
| `libidn2.so.0` | `/usr/lib/libidn2.so.0` | `libidn2` |
| `libssh2.so.1` | `/usr/lib/libssh2.so.1` | `libssh2` |
| `libpsl.so.5` | `/usr/lib/libpsl.so.5` | `libpsl` |
| `libssl.so.3` | `/usr/lib/libssl.so.3` | `openssl` |
| `libcrypto.so.3` | `/usr/lib/libcrypto.so.3` | `openssl` |
| `libgssapi_krb5.so.2` | `/usr/lib/libgssapi_krb5.so.2` | `krb5` |
| `libzstd.so.1` | `/usr/lib/libzstd.so.1` | `zstd` |
| `libbrotlidec.so.1` | `/usr/lib/libbrotlidec.so.1` | `brotli` |
| `libz.so.1` | `/usr/lib/libz.so.1` | `zlib` |
| `libunistring.so.5` | `/usr/lib/libunistring.so.5` | `libunistring` |
| `libkrb5.so.3` | `/usr/lib/libkrb5.so.3` | `krb5` |
| `libk5crypto.so.3` | `/usr/lib/libk5crypto.so.3` | `krb5` |
| `libcom_err.so.2` | `/usr/lib/libcom_err.so.2` | `e2fsprogs` |
| `libkrb5support.so.0` | `/usr/lib/libkrb5support.so.0` | `krb5` |
| `libkeyutils.so.1` | `/usr/lib/libkeyutils.so.1` | `keyutils` |
| `libresolv.so.2` | `/usr/lib/libresolv.so.2` | `glibc` |
| `libbrotlicommon.so.1` | `/usr/lib/libbrotlicommon.so.1` | `brotli` |

### Deduplicated owner set

`brotli`, `curl`, `e2fsprogs`, `gcc-libs`, `glibc`, `keyutils`, `krb5`, `libidn2`, `libnghttp2`, `libnghttp3`, `libngtcp2`, `libpsl`, `libssh2`, `libunistring`, `libxml2-legacy`, `openssl`, `sqlite`, `zlib`, `zstd`.

## Proposed `depends` for the `codexbar-tui` AUR package

The TUI's AUR package will declare `codexbar` as a dependency (whether we produce that package ourselves or depend on an existing one — TBD). Either way, **the transitive runtime deps must be resolvable on a clean Arch install.**

### Hard depends (must be installed for codexbar to run at all)

Most of the list above is already a transitive dependency of every Arch desktop system (glibc, gcc-libs, curl, openssl, zlib, etc.) and `pacman` will never prompt about them. The ones we **must** spell out because they aren't guaranteed on a minimal system:

```
depends=(
  # Transitive via the upstream codexbar CLI:
  'libxml2-legacy'     # the only non-stock lib
  'curl'               # libcurl.so.4
  'sqlite'             # libsqlite3.so.0
  'krb5'               # libgssapi_krb5.so.2, libkrb5.so.3, etc.
  'libnghttp2'         # libnghttp2.so.14
  'libnghttp3'         # libnghttp3.so.9
  'libngtcp2'          # libngtcp2.so.16 + libngtcp2_crypto_ossl.so.0
  'libidn2'            # libidn2.so.0
  'libssh2'            # libssh2.so.1
  'libpsl'             # libpsl.so.5
  'brotli'             # libbrotli{dec,common}.so.1
  'libunistring'       # libunistring.so.5
  'keyutils'           # libkeyutils.so.1
  'e2fsprogs'          # libcom_err.so.2 (usually core, listed for paranoia)
  'zstd'               # libzstd.so.1
  # Implicit on any Arch install (listed for documentation only, not required in depends=()):
  #   glibc, gcc-libs, zlib, openssl
)
```

Note: `curl` alone pulls in most of the libcurl-related items (`libidn2`, `libpsl`, `libssh2`, `nghttp2`, `nghttp3`, `ngtcp2`, `brotli`, `zstd`) as its own deps — so in practice a shorter list of `('codexbar' 'libxml2-legacy' 'curl' 'sqlite' 'krb5' 'libunistring' 'keyutils')` covers everything transitively. Final pruning against `pactree curl` etc. before the PKGBUILD ships.

If the upstream `codexbar` binary is packaged separately (as `codexbar-bin` on AUR or similar), our `codexbar-tui` package just does:

```
depends=('codexbar' 'libxml2-legacy')
```

…and leaves the lib list to the upstream package.

### Optional depends (data sources — user picks)

These are the **upstream-CLI** runtime deps documented in `runtime-deps.md`. They are per-provider and each unlocks one chunk of TUI functionality:

```
optdepends=(
  'claude-code: read Claude quota via `claude` CLI (--source cli) and scan ~/.claude/projects for cost'
  'codex: read Codex quota via `codex` CLI (--source cli) and scan ~/.codex/sessions for cost'
  'gemini-cli: read Gemini quota via stored OAuth creds'
  'kiro-cli: read Kiro quota (requires AWS Builder ID login)'
)
```

Exact Arch package names are AUR-sourced and must be verified before the PKGBUILD lands:

- `claude-code` — Anthropic's CLI (AUR)
- `codex` — OpenAI's CLI (AUR) — verify package name
- `gemini-cli` — Google's CLI (AUR) — verify package name
- `kiro-cli` — AWS's CLI for Kiro (AUR) — verify availability

None are in `extra/` or `core/` as of v0.20 release date.

### Not depends (common misconceptions)

- **`gh`** (GitHub CLI) — codexbar does its own Copilot device flow.
- **`gnome-keyring` / `libsecret` / `kwallet`** — codexbar does **not** integrate with Linux secret stores in v0.20.
- **`python` / `nodejs` / `bun`** — the upstream tool is a statically-built Swift binary; it doesn't spawn a language runtime itself. (The `claude` / `gemini` CLIs it shells into are Node-based, but that's their own dependency — covered by their AUR packages.)
- **`webkit2gtk` / any GUI lib** — web source is macOS-only; no GTK/Qt needed on Linux.

## Architecture constraints

Upstream ships `CodexBarCLI-v0.20-linux-x86_64.tar.gz` and `CodexBarCLI-v0.20-linux-aarch64.tar.gz`. So:

```
arch=('x86_64' 'aarch64')
```

…with two separate `source` / `sha256sums` entries keyed by `CARCH`.

## Next step (not part of this audit)

Write the PKGBUILD for `codexbar-bin` (upstream binary) first, then `codexbar-tui` layered on top of it. Do not bundle the upstream binary inside the TUI package.
