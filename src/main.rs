mod config;
mod merge;
mod parse;
mod poll;
mod providers;
mod spawn;
mod state;
mod ui;

use std::io;
use std::sync::mpsc::{Receiver, TryRecvError};
use std::time::{Duration, Instant};

use chrono::{NaiveDate, Utc};
use crossterm::{
    event::{self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyEventKind},
    execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use ratatui::{Terminal, backend::CrosstermBackend};

use crate::config::Config;
use crate::merge::{ProviderId, build_snapshot};
use crate::parse::{CostRecord, UsageRecord};
use crate::poll::{PollEvent, WorkerHandle, broadcast_refresh, shutdown, start_workers};
use crate::state::{AppState, Command};

const RENDER_TICK: Duration = Duration::from_millis(1000);

/// Per-provider cache of the latest successful records, kept on the main
/// thread so we can rebuild a ProviderSnapshot whenever either the usage
/// records or the cost record changes.
#[derive(Default)]
struct ProviderCache {
    usage: Option<Vec<UsageRecord>>,
    cost: Option<CostRecord>,
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

    let (rx, handles) = start_workers(&providers, cfg.intervals.usage, cfg.intervals.cost);

    let mut terminal = setup_terminal()?;
    let loop_result = run_event_loop(&mut terminal, &mut state, rx, &handles);
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
) -> Result<(), Box<dyn std::error::Error>> {
    let mut caches: std::collections::HashMap<ProviderId, ProviderCache> = state
        .providers
        .iter()
        .map(|p| (p.clone(), ProviderCache::default()))
        .collect();

    let mut last_tick = Instant::now();
    loop {
        if state.should_quit {
            return Ok(());
        }

        // Drain any poll events accumulated since the last tick.
        loop {
            match rx.try_recv() {
                Ok(ev) => apply_event(ev, state, &mut caches),
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
    caches: &mut std::collections::HashMap<ProviderId, ProviderCache>,
) {
    match ev {
        PollEvent::Usage { provider, records } => {
            caches.entry(provider.clone()).or_default().usage = Some(records);
            rebuild(&provider, state, caches);
            state.clear_status();
        }
        PollEvent::Cost { provider, record } => {
            caches.entry(provider.clone()).or_default().cost = record;
            rebuild(&provider, state, caches);
            state.clear_status();
        }
        PollEvent::Error {
            provider,
            command,
            message,
        } => {
            let which = match command {
                Command::Usage => "usage",
                Command::Cost => "cost",
            };
            state.set_status(format!("{} {}: {message}", provider.label(), which));
        }
    }
}

fn rebuild(
    provider: &ProviderId,
    state: &mut AppState,
    caches: &std::collections::HashMap<ProviderId, ProviderCache>,
) {
    let Some(cache) = caches.get(provider) else {
        return;
    };
    let Some(usage) = &cache.usage else {
        return;
    };
    let now = Utc::now();
    // IMPORTANT: DailyCost.date is bucketed by codexbar in local time (honors
    // $TZ; see docs/cli-reference/schema.md). Always ask for today in LOCAL
    // time here -- using Utc::now() would pick the wrong bucket whenever the
    // user's local date differs from the UTC date (up to ~24h drift).
    let today: NaiveDate = chrono::Local::now().date_naive();
    let snap = build_snapshot(provider.clone(), usage, cache.cost.as_ref(), today, now);
    state.apply_snapshot(snap);
}
