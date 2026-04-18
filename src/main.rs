mod config;
mod merge;
mod parse;
mod poll;
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

    let mut state = AppState::new(cfg.providers.clone(), cfg.intervals.clone());
    if let Some(path) = cfg_path {
        state.set_status(format!("loaded config: {}", path.display()));
    } else {
        state.set_status("using default config (no ~/.config/codexbar-tui/config.toml)");
    }

    let (rx, handles) = start_workers(
        &cfg.providers,
        cfg.intervals.usage,
        cfg.intervals.cost,
    );

    let mut terminal = setup_terminal()?;
    let loop_result = run_event_loop(&mut terminal, &mut state, rx, &handles);
    restore_terminal(&mut terminal)?;

    shutdown(handles);

    loop_result
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
    let mut caches: std::collections::HashMap<ProviderId, ProviderCache> =
        state.providers.iter().map(|p| (*p, ProviderCache::default())).collect();

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
            caches.entry(provider).or_default().usage = Some(records);
            rebuild(provider, state, caches);
            state.clear_status();
        }
        PollEvent::Cost { provider, record } => {
            caches.entry(provider).or_default().cost = record;
            rebuild(provider, state, caches);
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
            state.set_status(format!(
                "{} {}: {message}",
                provider.label(),
                which
            ));
        }
    }
}

fn rebuild(
    provider: ProviderId,
    state: &mut AppState,
    caches: &std::collections::HashMap<ProviderId, ProviderCache>,
) {
    let Some(cache) = caches.get(&provider) else {
        return;
    };
    let Some(usage) = &cache.usage else {
        return;
    };
    let now = Utc::now();
    let today: NaiveDate = chrono::Local::now().date_naive();
    let snap = build_snapshot(provider, usage, cache.cost.as_ref(), today, now);
    state.apply_snapshot(snap);
}
