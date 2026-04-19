mod cache;
mod config;
mod merge;
mod parse;
mod poll;
mod providers;
mod spawn;
mod state;
mod ui;

use std::collections::HashMap;
use std::io;
use std::sync::mpsc::{Receiver, TryRecvError};
use std::time::{Duration, Instant};

use chrono::{DateTime, NaiveDate, Utc};
use crossterm::{
    event::{self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyEventKind},
    execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use ratatui::{Terminal, backend::CrosstermBackend};

use crate::config::Config;
use crate::merge::{ProviderHealth, ProviderId, build_snapshot};
use crate::parse::{CostRecord, UsageRecord};
use crate::poll::{
    PollEvent, WorkerHandle, broadcast_refresh, pause_worker, shutdown, start_workers,
};
use crate::state::{AppState, Command};

const RENDER_TICK: Duration = Duration::from_millis(1000);

/// Per-provider cache of the latest successful records, kept on the main
/// thread so we can rebuild a ProviderSnapshot whenever either the usage
/// records or the cost record changes.
///
/// Each half carries its own `fetched_at` because usage and cost are
/// polled on independent cadences (60 s / 300 s by default) and can be
/// sourced from different origins (a fresh poll vs. the on-disk cache we
/// hydrate at startup). `rebuild` stamps the ProviderSnapshot with the
/// **later** of the two so the UI's "fetched Xm ago" line tracks the
/// freshest piece of data the panel shows.
#[derive(Default)]
struct ProviderCache {
    usage: Option<CachedUsage>,
    cost: Option<CachedCost>,
}

#[derive(Clone)]
struct CachedUsage {
    fetched_at: DateTime<Utc>,
    records: Vec<UsageRecord>,
}

#[derive(Clone)]
struct CachedCost {
    fetched_at: DateTime<Utc>,
    record: Option<CostRecord>,
}

impl ProviderCache {
    /// Snapshot's `fetched_at` is the newest of whichever halves exist.
    /// `None` only when both halves are empty (brand-new cache entry),
    /// in which case `rebuild` skips — we have nothing to render yet.
    fn latest_fetched_at(&self) -> Option<DateTime<Utc>> {
        match (&self.usage, &self.cost) {
            (Some(u), Some(c)) => Some(u.fetched_at.max(c.fetched_at)),
            (Some(u), None) => Some(u.fetched_at),
            (None, Some(c)) => Some(c.fetched_at),
            (None, None) => None,
        }
    }
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let (cfg, cfg_path) = match config::load() {
        Ok(v) => v,
        Err(e) => {
            eprintln!("config error: {e}");
            (Config::default(), None)
        }
    };

    let (providers, provider_source) = resolve_providers(&cfg);

    let mut state = AppState::new(providers.clone(), cfg.intervals.clone());
    state.set_status(startup_status(cfg_path.as_deref(), &provider_source, &providers));
    if providers.is_empty() {
        state.set_empty_reason(empty_state_message(&provider_source));
    }

    // Hydrate the main-thread cache from disk BEFORE starting workers and
    // setting up the terminal. `cache::load` is infallible (failure paths
    // collapse to an empty cache), so the only cost is a single small
    // stat + read + JSON parse — negligible compared to the ~15 s first
    // codexbar poll it saves us from having to wait for.
    let disk_cache = cache::load();

    let (rx, handles) = start_workers(&providers, cfg.intervals.usage, cfg.intervals.cost);

    let mut terminal = setup_terminal()?;
    let loop_result = run_event_loop(&mut terminal, &mut state, rx, &handles, disk_cache);
    restore_terminal(&mut terminal)?;

    shutdown(handles);

    loop_result
}

/// Result of the startup handshake with `codexbar config dump`: either a
/// clean read ("used"), a parse / spawn failure we can degrade past, or the
/// empty-list case.
enum ProviderSource {
    /// Post-filter provider set. `dumped` is the raw count from codexbar;
    /// `skipped_web_only` is how many we dropped via `providers::LINUX_WEB_ONLY`;
    /// `hidden` is how many the user denied in `hidden_providers`.
    Used {
        dumped: usize,
        skipped_web_only: usize,
        hidden: usize,
    },
    /// codexbar reachable but the dump contained zero provider entries.
    DumpEmpty,
    /// codexbar missing from PATH or the dump failed to parse; we fall
    /// back to an empty provider set and let the user fix it.
    Unavailable { reason: String },
}

fn resolve_providers(cfg: &Config) -> (Vec<ProviderId>, ProviderSource) {
    // NOTE: do NOT pass `--no-color` here. codexbar v0.20 rejects it on
    // `config dump` (it is accepted as a "global" flag on `usage` / `cost`
    // but not this subcommand) and instead emits
    // `[{"error":{"message":"Unknown option --no-color",...}}]` on stdout,
    // which crashes the parser. --format json output doesn't emit ANSI
    // anyway; --no-color is pointless here.
    match spawn::run_codexbar(
        &["config", "dump", "--format", "json"],
        Some(Duration::from_secs(5)),
    ) {
        Ok(out) => match parse::parse_config_dump(&out.stdout) {
            Ok(dump) => {
                let dumped_ids = dump.ids();
                let dumped = dumped_ids.len();
                // Two-stage filter: (1) drop providers whose only codexbar
                // source mode is web (macOS-gated in v0.20); (2) drop
                // anything in the user's hidden_providers denylist.
                let mut skipped_web_only = 0usize;
                let mut hidden = 0usize;
                let providers: Vec<ProviderId> = dumped_ids
                    .into_iter()
                    .filter(|id| {
                        if providers::is_linux_web_only(id) {
                            skipped_web_only += 1;
                            return false;
                        }
                        if cfg.is_hidden(id) {
                            hidden += 1;
                            return false;
                        }
                        true
                    })
                    .map(ProviderId::new)
                    .collect();
                if dumped == 0 {
                    (providers, ProviderSource::DumpEmpty)
                } else {
                    (
                        providers,
                        ProviderSource::Used {
                            dumped,
                            skipped_web_only,
                            hidden,
                        },
                    )
                }
            }
            Err(e) => (
                Vec::new(),
                ProviderSource::Unavailable {
                    reason: format!("parsing codexbar config dump: {e}"),
                },
            ),
        },
        Err(e) => (
            Vec::new(),
            ProviderSource::Unavailable {
                reason: format!("codexbar config dump failed: {e}"),
            },
        ),
    }
}

fn startup_status(
    cfg_path: Option<&std::path::Path>,
    source: &ProviderSource,
    providers: &[ProviderId],
) -> String {
    let config_part = match cfg_path {
        Some(p) => format!("config: {}", p.display()),
        None => "config: default (no ~/.config/codexbar-tui/config.toml)".to_string(),
    };
    let provider_part = match source {
        ProviderSource::Used {
            dumped,
            skipped_web_only,
            hidden,
        } if *skipped_web_only > 0 || *hidden > 0 => {
            format!(
                "  providers: {} listed, {} web-only skipped, {} hidden -> {} shown",
                dumped,
                skipped_web_only,
                hidden,
                providers.len()
            )
        }
        ProviderSource::Used { dumped, .. } => {
            format!("  providers: {dumped} from codexbar config dump")
        }
        ProviderSource::DumpEmpty => {
            "  providers: codexbar config dump returned no entries".to_string()
        }
        ProviderSource::Unavailable { reason } => format!("  providers: {reason}"),
    };
    format!("{config_part}{provider_part}")
}

/// Multi-line body message shown when we have zero providers to render.
/// The three branches match the three ways the list can be empty:
///   * `Unavailable` — codexbar missing or its config dump was unparseable.
///   * `DumpEmpty` — codexbar reachable but the dump carried zero entries.
///   * `Used {...}` with post-filter emptiness — every provider was either
///     skipped by the Linux web-only list or denied by hidden_providers.
fn empty_state_message(source: &ProviderSource) -> String {
    match source {
        ProviderSource::Unavailable { reason } => format!(
            "codexbar not available: {reason}\n\nInstall the upstream codexbar CLI or check PATH."
        ),
        ProviderSource::DumpEmpty => {
            "codexbar config dump returned no provider entries.\n\nThis usually means ~/.codexbar/config.json is missing or empty.".to_string()
        }
        ProviderSource::Used { .. } => {
            "No providers to show.\n\nEither codexbar lists none usable on Linux, or all of them are hidden in ~/.config/codexbar-tui/config.toml under hidden_providers.".to_string()
        }
    }
}

fn setup_terminal() -> io::Result<Terminal<CrosstermBackend<io::Stdout>>> {
    enable_raw_mode()?;
    let mut out = io::stdout();
    execute!(out, EnterAlternateScreen, EnableMouseCapture)?;
    Terminal::new(CrosstermBackend::new(out))
}

fn restore_terminal(terminal: &mut Terminal<CrosstermBackend<io::Stdout>>) -> io::Result<()> {
    disable_raw_mode()?;
    execute!(
        terminal.backend_mut(),
        LeaveAlternateScreen,
        DisableMouseCapture
    )?;
    terminal.show_cursor()
}

fn run_event_loop(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    state: &mut AppState,
    rx: Receiver<PollEvent>,
    handles: &[WorkerHandle],
    disk: cache::CacheFile,
) -> Result<(), Box<dyn std::error::Error>> {
    // Seed the in-memory cache from whatever we had on disk. Unknown
    // providers in the disk file (e.g. user disabled one since last
    // launch) are silently ignored — `providers.iter()` drives the key
    // set, not the disk file.
    let mut caches: HashMap<ProviderId, ProviderCache> = state
        .providers
        .iter()
        .map(|p| {
            let entry = disk.providers.get(p.cli_id()).cloned().unwrap_or_default();
            (p.clone(), provider_cache_from_disk(entry))
        })
        .collect();

    // Pre-render snapshots for every provider that has ANY cached data.
    // This is the whole point of the disk cache: the first `terminal.draw`
    // below will show real usage bars / costs instead of "waiting for
    // first poll…". Providers with no cache keep their None snapshot and
    // fall back to the loading placeholder, same as before.
    for p in state.providers.clone() {
        rebuild(&p, state, &caches);
    }

    let mut last_tick = Instant::now();
    loop {
        if state.should_quit {
            return Ok(());
        }

        // Drain any poll events accumulated since the last tick.
        loop {
            match rx.try_recv() {
                Ok(ev) => apply_event(ev, state, &mut caches, handles),
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => {
                    // All workers exited — keep running so the user can see
                    // the last snapshots, but flag it in the footer.
                    state.set_status("workers exited; press q to quit");
                    break;
                }
            }
        }

        let now = Utc::now();
        terminal.draw(|f| ui::draw(f, state, now))?;

        // Sleep until the next 1 Hz tick, but handle key events as they
        // arrive (crossterm::event::poll uses millisecond granularity).
        let elapsed = last_tick.elapsed();
        let remaining = RENDER_TICK.saturating_sub(elapsed);
        if event::poll(remaining)? {
            if let Event::Key(k) = event::read()? {
                if k.kind == KeyEventKind::Press {
                    match k.code {
                        KeyCode::Char('q') | KeyCode::Esc => state.quit(),
                        KeyCode::Char('c')
                            if k.modifiers.contains(event::KeyModifiers::CONTROL) =>
                        {
                            state.quit();
                        }
                        KeyCode::Char('r') => {
                            broadcast_refresh(handles);
                            state.set_status("refreshing…");
                        }
                        KeyCode::Char('a') => {
                            state.toggle_show_all();
                            // Drop any sticky startup status so the footer's
                            // left side stops obscuring the mode indicator
                            // the moment the user engages with the toggle.
                            state.clear_status();
                        }
                        _ => {}
                    }
                }
            }
        }
        if last_tick.elapsed() >= RENDER_TICK {
            last_tick = Instant::now();
        }
    }
}

fn apply_event(
    ev: PollEvent,
    state: &mut AppState,
    caches: &mut HashMap<ProviderId, ProviderCache>,
    handles: &[WorkerHandle],
) {
    match ev {
        PollEvent::Usage { provider, records } => {
            caches.entry(provider.clone()).or_default().usage = Some(CachedUsage {
                fetched_at: Utc::now(),
                records,
            });
            rebuild(&provider, state, caches);
            maybe_pause_after(&provider, Command::Usage, state, handles);
            state.clear_status();
            // Persist AFTER rebuild so the new data is definitely in
            // `caches` when we dump it. Write is best-effort; disk errors
            // are swallowed (see persist_cache).
            persist_cache(caches);
        }
        PollEvent::Cost { provider, record } => {
            caches.entry(provider.clone()).or_default().cost = Some(CachedCost {
                fetched_at: Utc::now(),
                record,
            });
            rebuild(&provider, state, caches);
            maybe_pause_after(&provider, Command::Cost, state, handles);
            state.clear_status();
            persist_cache(caches);
        }
        PollEvent::Error {
            provider,
            command,
            message,
        } => {
            // Errors don't mutate the cache, so there's no reason to
            // rewrite the file on this branch — the last known good
            // values stay on disk exactly as they were.
            let which = match command {
                Command::Usage => "usage",
                Command::Cost => "cost",
            };
            state.set_status(format!("{} {}: {message}", provider.label(), which));
        }
    }
}

/// Translate the in-memory cache into the on-disk envelope and write it.
/// Failures are swallowed: the cache is a performance aid, not state the
/// app depends on. Worst case the next launch shows "waiting for first
/// poll…" placeholders, exactly like before this module existed.
fn persist_cache(caches: &HashMap<ProviderId, ProviderCache>) {
    let mut providers: std::collections::HashMap<String, cache::ProviderEntry> =
        std::collections::HashMap::with_capacity(caches.len());
    for (id, entry) in caches {
        providers.insert(id.cli_id().to_string(), provider_cache_to_disk(entry));
    }
    let file = cache::CacheFile {
        version: cache::CACHE_VERSION,
        providers,
    };
    let _ = cache::save(&file);
}

fn provider_cache_from_disk(entry: cache::ProviderEntry) -> ProviderCache {
    ProviderCache {
        usage: entry.usage.map(|u| CachedUsage {
            fetched_at: u.fetched_at,
            records: u.records,
        }),
        cost: entry.cost.map(|c| CachedCost {
            fetched_at: c.fetched_at,
            record: c.record,
        }),
    }
}

fn provider_cache_to_disk(entry: &ProviderCache) -> cache::ProviderEntry {
    cache::ProviderEntry {
        usage: entry.usage.as_ref().map(|u| cache::UsageEntry {
            fetched_at: u.fetched_at,
            records: u.records.clone(),
        }),
        cost: entry.cost.as_ref().map(|c| cache::CostEntry {
            fetched_at: c.fetched_at,
            record: c.record.clone(),
        }),
    }
}

/// Pause a worker after a classify-as-unrecoverable result so we stop
/// re-spawning codexbar for a provider that can't succeed without user
/// action. Critical for Codex: `codexbar usage --provider codex --source
/// cli` invokes `codex` under the hood, and without `~/.codex/auth.json`
/// Codex CLI opens a new browser tab every call — our 60s cadence would
/// otherwise spam the user with login tabs forever.
///
/// The user resumes polling by pressing `r`, which broadcasts `Refresh`
/// to every worker and clears the paused flag.
fn maybe_pause_after(
    provider: &ProviderId,
    command: Command,
    state: &AppState,
    handles: &[WorkerHandle],
) {
    let Some(snap) = state.snapshot(provider) else {
        return;
    };
    let should_pause = matches!(
        snap.health,
        ProviderHealth::AuthMissing
            | ProviderHealth::NotSupportedOnLinux
            | ProviderHealth::Error { .. }
    );
    if should_pause {
        pause_worker(handles, provider, command);
    }
}

fn rebuild(
    provider: &ProviderId,
    state: &mut AppState,
    caches: &HashMap<ProviderId, ProviderCache>,
) {
    let Some(entry) = caches.get(provider) else {
        return;
    };
    let Some(usage) = &entry.usage else {
        return;
    };
    // Snapshot timestamp = the newest piece of data it contains. That's
    // usually `now` right after a live poll (both halves just updated)
    // but on a cache-hydrated launch it's the original poll time from
    // disk, so the UI's "fetched Xm ago" line correctly reports stale
    // data as stale. Falling back to Utc::now() is defensive; with a
    // Some(usage) above we always have at least one timestamp.
    let fetched_at = entry.latest_fetched_at().unwrap_or_else(Utc::now);
    // IMPORTANT: DailyCost.date is bucketed by codexbar in local time (honors
    // $TZ; see docs/cli-reference/schema.md). Always ask for today in LOCAL
    // time here -- using Utc::now() would pick the wrong bucket whenever the
    // user's local date differs from the UTC date (up to ~24h drift). We
    // deliberately use *current* local date, not the cached fetched_at's
    // date, so rehydrating a cache written yesterday still buckets into
    // today's column correctly.
    let today: NaiveDate = chrono::Local::now().date_naive();
    let cost_record = entry.cost.as_ref().and_then(|c| c.record.as_ref());
    let snap = build_snapshot(provider.clone(), &usage.records, cost_record, today, fetched_at);
    state.apply_snapshot(snap);
}
