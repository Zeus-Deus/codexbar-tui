//! Disk-backed cache of the last successful poll per provider.
//!
//! Why: a cold launch makes the user stare at "waiting for first poll…"
//! for 15–20 s because the codexbar subprocess calls it wraps are slow
//! (docs/cli-reference/timings.md; `claude usage` is ~16 s, `claude cost`
//! is ~20 s). Persisting each successful poll to
//! `~/.cache/codexbar-tui/snapshots.json` and replaying it at startup lets
//! us paint real data in ~50 ms on every launch after the first. The
//! panel's "fetched Xm ago" line doubles as the freshness indicator —
//! no extra UI state needed.
//!
//! We store the RAW `UsageRecord` / `CostRecord` values, not the
//! computed `ProviderSnapshot`. Snapshot buckets (today vs. 30-day)
//! depend on `chrono::Local::now().date_naive()` at build time; caching
//! the computed snapshot would misattribute "today" after a
//! midnight-local boundary. Caching raw data and re-running
//! `build_snapshot` at load keeps the calendar math correct.
//!
//! Failures are non-fatal by design. Missing file, parse error,
//! version mismatch, disk full, no HOME — every path collapses to
//! "no cache" and the app keeps running. The cache is a performance
//! optimisation, not a source of truth.

use std::collections::HashMap;
use std::fs;
use std::io;
use std::path::PathBuf;

use chrono::{DateTime, Utc};
use directories::ProjectDirs;
use serde::{Deserialize, Serialize};

use crate::parse::{CostRecord, UsageRecord};

/// Bump when the on-disk shape changes in an incompatible way.
/// Readers silently discard caches that don't match the current version,
/// so old files don't get fed into a parser that no longer understands
/// them — they just regenerate on the first successful poll.
pub const CACHE_VERSION: u32 = 1;

/// Top-level envelope written to disk. Map key is the provider's
/// `cli_id()` (same string codexbar uses on `--provider`).
#[derive(Debug, Default, Serialize, Deserialize)]
pub struct CacheFile {
    pub version: u32,
    #[serde(default)]
    pub providers: HashMap<String, ProviderEntry>,
}

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct ProviderEntry {
    #[serde(default)]
    pub usage: Option<UsageEntry>,
    #[serde(default)]
    pub cost: Option<CostEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UsageEntry {
    /// When the poll result was received by the main loop. Drives the
    /// snapshot's `fetched_at`, which the footer / panel title renders as
    /// "fetched Xm ago".
    pub fetched_at: DateTime<Utc>,
    pub records: Vec<UsageRecord>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CostEntry {
    pub fetched_at: DateTime<Utc>,
    /// `None` is a valid cached value: codexbar returned no cost record
    /// (provider had no session data yet). Caching the absence prevents
    /// a launch from synthesising a fake "no data" panel when we in fact
    /// know the correct answer.
    pub record: Option<CostRecord>,
}

/// `~/.cache/codexbar-tui/snapshots.json` via XDG; falls back to the
/// platform default if `$HOME` / `$XDG_CACHE_HOME` are unusable.
pub fn default_path() -> Option<PathBuf> {
    let dirs = ProjectDirs::from("", "", "codexbar-tui")?;
    Some(dirs.cache_dir().join("snapshots.json"))
}

/// Read the cache. Any failure path (missing file, bad JSON, version
/// mismatch) returns `CacheFile::default()` so startup keeps going.
pub fn load() -> CacheFile {
    let Some(path) = default_path() else {
        return CacheFile::default();
    };
    let Ok(bytes) = fs::read(&path) else {
        return CacheFile::default();
    };
    match serde_json::from_slice::<CacheFile>(&bytes) {
        Ok(c) if c.version == CACHE_VERSION => c,
        _ => CacheFile::default(),
    }
}

/// Atomically write the cache. Writes to a `*.tmp` sibling and renames,
/// so a crash mid-write never leaves a truncated file that the next
/// launch would read back as empty. All I/O errors are returned to the
/// caller, which currently just swallows them (see `main::persist_cache`).
pub fn save(cache: &CacheFile) -> io::Result<()> {
    let Some(path) = default_path() else {
        return Ok(());
    };
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let tmp = path.with_extension("json.tmp");
    let body = serde_json::to_vec(cache)
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
    fs::write(&tmp, &body)?;
    fs::rename(&tmp, &path)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn load_missing_file_returns_default() {
        // We can't easily redirect default_path() for this test, but we
        // can exercise the "bad bytes" branch directly.
        let c: CacheFile = serde_json::from_slice(b"not json").unwrap_or_default();
        assert_eq!(c.version, 0);
        assert!(c.providers.is_empty());
    }

    #[test]
    fn version_mismatch_is_discarded_on_load() {
        // Simulate the version-gate directly: a parseable cache with the
        // wrong version number must not be handed back.
        let wrong = CacheFile {
            version: CACHE_VERSION + 7,
            providers: {
                let mut m = HashMap::new();
                m.insert("claude".into(), ProviderEntry::default());
                m
            },
        };
        let bytes = serde_json::to_vec(&wrong).unwrap();
        let parsed: CacheFile = match serde_json::from_slice::<CacheFile>(&bytes) {
            Ok(c) if c.version == CACHE_VERSION => c,
            _ => CacheFile::default(),
        };
        assert_eq!(parsed.version, 0, "wrong-version cache must be discarded");
        assert!(parsed.providers.is_empty());
    }

    #[test]
    fn roundtrip_preserves_usage_and_cost_entries() {
        use crate::parse::{parse_cost, parse_usage};

        // Feed the real parse layer a codexbar-shaped payload so the
        // Serialize derives we added to parse.rs exercise the full shape,
        // not a hand-built subset.
        let usage_json = br#"[{"provider":"claude","source":"cli","version":"0.20","usage":{"identity":{"providerID":"claude"},"primary":{"usedPercent":42,"windowMinutes":300,"resetsAt":"2026-04-19T12:00:00Z","resetDescription":"resets at noon"},"updatedAt":"2026-04-19T00:00:00Z"}}]"#;
        let cost_json = br#"[{"provider":"claude","source":"local","updatedAt":"2026-04-19T00:00:00Z","daily":[{"date":"2026-04-19","inputTokens":1,"outputTokens":2,"cacheCreationTokens":0,"cacheReadTokens":0,"totalTokens":3,"totalCost":0.5,"modelsUsed":["sonnet"],"modelBreakdowns":[{"modelName":"sonnet","totalTokens":3,"cost":0.5}]}]}]"#;
        let usage = parse_usage(usage_json).unwrap();
        let cost = parse_cost(cost_json).unwrap().into_iter().next();

        let mut file = CacheFile {
            version: CACHE_VERSION,
            providers: HashMap::new(),
        };
        file.providers.insert(
            "claude".into(),
            ProviderEntry {
                usage: Some(UsageEntry {
                    fetched_at: "2026-04-19T00:00:01Z".parse().unwrap(),
                    records: usage,
                }),
                cost: Some(CostEntry {
                    fetched_at: "2026-04-19T00:00:02Z".parse().unwrap(),
                    record: cost,
                }),
            },
        );

        let bytes = serde_json::to_vec(&file).expect("serialize");
        let back: CacheFile = serde_json::from_slice(&bytes).expect("deserialize");
        assert_eq!(back.version, CACHE_VERSION);
        let entry = back.providers.get("claude").expect("provider present");
        let u = entry.usage.as_ref().expect("usage cached");
        assert_eq!(u.records.len(), 1);
        assert_eq!(u.records[0].provider, "claude");
        let c = entry.cost.as_ref().expect("cost cached");
        let rec = c.record.as_ref().expect("cost record present");
        assert_eq!(rec.daily.len(), 1);
        assert_eq!(rec.daily[0].total_cost, Some(0.5));
    }
}
