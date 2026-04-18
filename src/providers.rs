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
}
