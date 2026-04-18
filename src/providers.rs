//! Compile-time policy about which upstream codexbar providers are usable
//! on Linux.
//!
//! Every ID in [`LINUX_WEB_ONLY`] is a provider whose only source mode in
//! codexbar v0.20 is `web` — which on Linux errors with
//! `"selected source requires web support and is only supported on macOS."`
//! (see `docs/cli-reference/linux-caveats.md`). There is no point spawning
//! a worker for these: every poll would fail immediately and the user would
//! just see red panels forever.
//!
//! **If upstream adds `--source cli` (or any other non-web source) for one
//! of these providers, REMOVE that ID from `LINUX_WEB_ONLY`**. The TUI will
//! then pick it up automatically via the next `codexbar config dump` read.
//!
//! Not in this list, and therefore expected to work on Linux:
//! `claude`, `codex` (CLI auth), `gemini`, `zai`, `warp`, `openrouter`,
//! `copilot`, `kimik2`, `kilo`, `kiro`, `vertexai`, `jetbrains`,
//! `antigravity`, `synthetic` (env-var / API-key / local-file auth).

/// Providers whose only v0.20 source mode (`web`) is macOS-gated. Skipped
/// at startup so we never spawn a worker that can only return errors.
pub const LINUX_WEB_ONLY: &[&str] = &[
    "cursor",
    "opencode",
    "opencodego",
    "amp",
    "abacus",
    "perplexity",
    "factory",
    "ollama",
    "minimax",
];

/// Case-insensitive membership check against [`LINUX_WEB_ONLY`].
pub fn is_linux_web_only(id: &str) -> bool {
    let lc = id.trim().to_ascii_lowercase();
    LINUX_WEB_ONLY.iter().any(|&x| x == lc)
}

/// True if we can determine this provider is NOT authenticated **without
/// spawning anything**. Used as a preflight to avoid invoking `codexbar`
/// (which shells out to the provider CLI) when we already know the call
/// will fail and the provider CLI has expensive side effects.
///
/// The original motivation is Codex: `codex` CLI opens a browser tab for
/// OAuth every time it is invoked against a missing `~/.codex/auth.json`,
/// and codexbar's v0.20 `cli` source tries the RPC path then falls back to
/// PTY — double invocation, double (or more) tabs. Checking for the
/// auth file up front short-circuits every subsequent poll as well, so
/// the user only opens tabs when they intentionally run `codex login`.
///
/// Providers we don't have a filesystem check for return `false` (be
/// permissive; let codexbar speak and let the post-poll pause-on-error
/// path handle it).
pub fn known_auth_missing(id: &str) -> bool {
    match id.trim().to_ascii_lowercase().as_str() {
        "codex" => {
            let dir = std::env::var_os("CODEX_HOME")
                .map(std::path::PathBuf::from)
                .or_else(|| {
                    std::env::var_os("HOME")
                        .map(|h| std::path::PathBuf::from(h).join(".codex"))
                });
            match dir {
                Some(d) => !d.join("auth.json").exists(),
                None => false, // no HOME / CODEX_HOME → can't decide, defer
            }
        }
        // Anything else: defer to codexbar. Claude CLI (`claude`) does not
        // open browser tabs on invocation without `.credentials.json`, so
        // there is no urgent need to preflight it. If another provider is
        // found to misbehave the same way, add its check here.
        _ => false,
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    /// Providers documented in docs/cli-reference/linux-caveats.md as
    /// working on Linux via CLI auth / env vars / API keys / local files.
    /// The skip list must never shadow any of these.
    const LINUX_WORKING: &[&str] = &[
        "claude",
        "codex",
        "gemini",
        "zai",
        "warp",
        "openrouter",
        "copilot",
        "kimik2",
        "kilo",
        "kiro",
        "vertexai",
        "jetbrains",
        "antigravity",
        "synthetic",
    ];

    #[test]
    fn skip_list_is_disjoint_from_working_set() {
        for id in LINUX_WORKING {
            assert!(
                !is_linux_web_only(id),
                "{id} is documented to work on Linux but is in LINUX_WEB_ONLY"
            );
        }
    }

    #[test]
    fn skip_list_matches_is_case_insensitive() {
        assert!(is_linux_web_only("factory"));
        assert!(is_linux_web_only("Factory"));
        assert!(is_linux_web_only("  FACTORY  "));
        assert!(!is_linux_web_only("claude"));
    }

    #[test]
    fn skip_list_has_no_duplicates() {
        let mut seen = std::collections::HashSet::new();
        for id in LINUX_WEB_ONLY {
            assert!(seen.insert(*id), "duplicate entry in LINUX_WEB_ONLY: {id}");
        }
    }

    #[test]
    fn known_auth_missing_probes_codex_home_then_home() {
        use std::path::PathBuf;
        let tmp = tempfile::tempdir().expect("tempdir");
        let dir: PathBuf = tmp.path().to_path_buf();

        // 1. CODEX_HOME points at a dir without auth.json → missing.
        // SAFETY: tests run single-threaded within this module so the env
        // mutations don't race with each other.
        // # Safety: this is a unit test; no other thread observes the env.
        unsafe {
            std::env::set_var("CODEX_HOME", &dir);
        }
        assert!(known_auth_missing("codex"));
        assert!(known_auth_missing("CODEX"), "case insensitive");

        // 2. Create the auth file → no longer missing.
        std::fs::write(dir.join("auth.json"), b"{\"stub\":true}").unwrap();
        assert!(!known_auth_missing("codex"));

        // 3. Other providers always return false (we don't check them).
        assert!(!known_auth_missing("claude"));
        assert!(!known_auth_missing("gemini"));
        assert!(!known_auth_missing("some-unknown"));

        unsafe {
            std::env::remove_var("CODEX_HOME");
        }
    }
}
