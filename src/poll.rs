//! Background polling of codexbar.
//!
//! One `std::thread::spawn` per `(provider, command)` pair. Each thread runs
//! its codexbar subprocess, parses the output, and posts a `PollEvent` to
//! the main loop over an `mpsc::Sender`. Between polls it sleeps for the
//! configured interval, waking early if the main loop sends a refresh
//! request over a per-worker `mpsc::Receiver<WorkerCmd>`.
//!
//! The main loop:
//!   - owns the event `Receiver<PollEvent>` and drains it between renders;
//!   - keeps a `Vec<Sender<WorkerCmd>>` for manual-refresh (the `r` key).
//!
//! Design notes:
//!   - No tokio. `std::thread` + `mpsc` is sufficient at N=4 workers
//!     (Claude usage, Claude cost, Codex usage, Codex cost).
//!   - Workers never block on anything that can't be interrupted: the
//!     wait-between-polls uses `recv_timeout`, so a sent `WorkerCmd::Refresh`
//!     or `WorkerCmd::Quit` unblocks immediately.
//!   - A busy subprocess call (up to 30 s) blocks the worker thread but
//!     cannot wedge the main thread.

use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::thread;
use std::time::Duration;

use crate::merge::ProviderId;
use crate::parse::{self, CostRecord, UsageRecord};
use crate::spawn::{self, SpawnError};
use crate::state::Command;

/// Payload shipped from a worker thread back to the main loop.
#[derive(Debug)]
pub enum PollEvent {
    /// The worker just ran and produced parsed records.
    Usage {
        provider: ProviderId,
        records: Vec<UsageRecord>,
    },
    Cost {
        provider: ProviderId,
        record: Option<CostRecord>,
    },
    /// The worker couldn't complete the call (timeout, spawn failure, etc).
    /// The main loop may show this in the status line.
    Error {
        provider: ProviderId,
        command: Command,
        message: String,
    },
}

/// Commands the main loop sends to a worker.
#[derive(Debug, Clone, Copy)]
pub enum WorkerCmd {
    /// Wake up and re-poll immediately; also clears the paused flag so
    /// normal cadence resumes.
    Refresh,
    /// Stop re-polling until a `Refresh` (or `Quit`) arrives. Workers that
    /// returned an unrecoverable error (auth missing, provider not
    /// supported on this platform, …) get paused so they stop re-spawning
    /// `codexbar` subprocesses — critical for Codex, whose CLI opens a
    /// browser tab every time it's invoked without `~/.codex/auth.json`.
    Pause,
    /// Exit the worker thread.
    Quit,
}

/// Handle on a running worker. Dropping it does not kill the worker; send
/// `WorkerCmd::Quit` for a clean exit. The `provider` + `command` fields
/// let callers target a single worker (e.g. to pause it after an auth
/// failure) rather than broadcasting.
pub struct WorkerHandle {
    pub provider: ProviderId,
    pub command: Command,
    pub tx: Sender<WorkerCmd>,
    pub join: thread::JoinHandle<()>,
}

/// Start one worker per (provider, command) pair. Returns the event
/// receiver (to drain in the render loop) plus the worker handles (to
/// broadcast refresh / quit).
pub fn start_workers(
    providers: &[ProviderId],
    usage_interval: Duration,
    cost_interval: Duration,
) -> (Receiver<PollEvent>, Vec<WorkerHandle>) {
    let (ev_tx, ev_rx) = mpsc::channel::<PollEvent>();
    let mut handles = Vec::with_capacity(providers.len() * 2);

    for p in providers {
        handles.push(spawn_worker(
            p.clone(),
            Command::Usage,
            usage_interval,
            ev_tx.clone(),
        ));
        handles.push(spawn_worker(
            p.clone(),
            Command::Cost,
            cost_interval,
            ev_tx.clone(),
        ));
    }
    // Dropping the spare ev_tx lets the main loop notice when every worker
    // has exited (channel closes).
    drop(ev_tx);
    (ev_rx, handles)
}

/// Tell every worker to refresh now. Best-effort: if a worker has already
/// exited, its send fails silently.
pub fn broadcast_refresh(handles: &[WorkerHandle]) {
    for h in handles {
        let _ = h.tx.send(WorkerCmd::Refresh);
    }
}

/// Pause a single worker identified by `(provider, command)`. Used when
/// the main loop classifies a poll result as unrecoverable (AuthMissing
/// etc.) so we don't keep hammering a provider that can't possibly
/// succeed. Best-effort on send. Safe to call repeatedly — a second Pause
/// to an already-paused worker is a no-op.
pub fn pause_worker(handles: &[WorkerHandle], provider: &ProviderId, command: Command) {
    for h in handles {
        if &h.provider == provider && h.command == command {
            let _ = h.tx.send(WorkerCmd::Pause);
            return;
        }
    }
}

/// Tell every worker to quit and wait for them. Best-effort on send.
pub fn shutdown(handles: Vec<WorkerHandle>) {
    for h in &handles {
        let _ = h.tx.send(WorkerCmd::Quit);
    }
    for h in handles {
        let _ = h.join.join();
    }
}

fn spawn_worker(
    provider: ProviderId,
    command: Command,
    interval: Duration,
    ev_tx: Sender<PollEvent>,
) -> WorkerHandle {
    let (cmd_tx, cmd_rx) = mpsc::channel::<WorkerCmd>();
    let provider_for_thread = provider.clone();
    let join = thread::Builder::new()
        .name(format!("poll-{:?}-{:?}", provider, command))
        .spawn(move || worker_loop(provider_for_thread, command, interval, ev_tx, cmd_rx))
        .expect("spawn worker thread");
    WorkerHandle {
        provider,
        command,
        tx: cmd_tx,
        join,
    }
}

fn worker_loop(
    provider: ProviderId,
    command: Command,
    interval: Duration,
    ev_tx: Sender<PollEvent>,
    cmd_rx: Receiver<WorkerCmd>,
) {
    let mut paused = false;
    loop {
        if !paused {
            run_once(&provider, command, &ev_tx);
        }

        // When paused we wait indefinitely for a control message (don't
        // burn cycles waking every `interval` seconds just to re-sleep).
        // When active we wait up to `interval` then re-poll on timeout.
        let result = if paused {
            cmd_rx.recv().map_err(|_| RecvTimeoutError::Disconnected)
        } else {
            cmd_rx.recv_timeout(interval)
        };

        match result {
            Ok(WorkerCmd::Refresh) => {
                paused = false;
                continue; // re-poll immediately at the top of the loop
            }
            Ok(WorkerCmd::Pause) => {
                paused = true;
                continue; // next iteration skips run_once
            }
            Ok(WorkerCmd::Quit) => return,
            Err(RecvTimeoutError::Timeout) => continue, // normal cadence
            Err(RecvTimeoutError::Disconnected) => return, // main dropped its sender
        }
    }
}

fn run_once(provider: &ProviderId, command: Command, ev_tx: &Sender<PollEvent>) {
    let cli_id = provider.cli_id();
    let result = match command {
        Command::Usage => spawn::usage_cli(cli_id, None).map(Output::Usage),
        Command::Cost => spawn::cost(cli_id, None).map(Output::Cost),
    };

    let event = match result {
        Ok(Output::Usage(out)) => match parse::parse_usage(&out.stdout) {
            Ok(records) => PollEvent::Usage {
                provider: provider.clone(),
                records,
            },
            Err(e) => PollEvent::Error {
                provider: provider.clone(),
                command,
                message: format!("usage parse: {e}"),
            },
        },
        Ok(Output::Cost(out)) => match parse::parse_cost(&out.stdout) {
            Ok(mut recs) => {
                // cost emits one record per provider at top-level; we
                // requested --provider <id>, so take the first.
                let record = recs.drain(..).next();
                PollEvent::Cost {
                    provider: provider.clone(),
                    record,
                }
            }
            Err(e) => PollEvent::Error {
                provider: provider.clone(),
                command,
                message: format!("cost parse: {e}"),
            },
        },
        Err(e) => PollEvent::Error {
            provider: provider.clone(),
            command,
            message: spawn_err_message(e),
        },
    };

    // If the main loop has dropped the receiver, the send fails and the
    // worker naturally exits on the next cmd_rx check anyway.
    let _ = ev_tx.send(event);
}

enum Output {
    Usage(spawn::Output),
    Cost(spawn::Output),
}

fn spawn_err_message(e: SpawnError) -> String {
    match e {
        SpawnError::NotFound(_) => "codexbar not on PATH".to_string(),
        SpawnError::Timeout(d) => format!("timed out after {:?}", d),
        SpawnError::Io(e) => format!("io: {e}"),
    }
}

#[cfg(test)]
mod tests {
    //! We can't exercise the full worker without codexbar on PATH, so the
    //! tests here just lock in the small, non-IO helpers and the
    //! refresh-broadcast round trip.
    use super::*;

    fn fake_handle(
        provider: &str,
        command: Command,
        logic: impl FnOnce(Receiver<WorkerCmd>) + Send + 'static,
    ) -> WorkerHandle {
        let (tx, rx) = mpsc::channel::<WorkerCmd>();
        let join = thread::spawn(move || logic(rx));
        WorkerHandle {
            provider: ProviderId::new(provider),
            command,
            tx,
            join,
        }
    }

    #[test]
    fn broadcast_refresh_delivers_to_every_worker() {
        let mut handles = Vec::new();
        for _ in 0..3 {
            handles.push(fake_handle("x", Command::Usage, |rx| match rx.recv() {
                Ok(WorkerCmd::Refresh) => {}
                other => panic!("expected Refresh, got {other:?}"),
            }));
        }
        broadcast_refresh(&handles);
        for h in handles {
            h.join.join().unwrap();
        }
    }

    #[test]
    fn shutdown_quits_each_worker() {
        let mut handles = Vec::new();
        for _ in 0..2 {
            handles.push(fake_handle("x", Command::Usage, |rx| loop {
                match rx.recv() {
                    Ok(WorkerCmd::Quit) => return,
                    Ok(WorkerCmd::Refresh | WorkerCmd::Pause) => continue,
                    Err(_) => return,
                }
            }));
        }
        shutdown(handles);
    }

    #[test]
    fn pause_worker_targets_the_matching_handle() {
        // Three workers: (claude, usage), (claude, cost), (codex, usage).
        // Pausing (codex, usage) must wake ONLY that thread and leave the
        // other two still blocked on their receiver.
        use std::sync::{Arc, Mutex};
        let received: Arc<Mutex<Vec<(String, Command)>>> = Arc::new(Mutex::new(Vec::new()));

        fn watcher(
            label: (String, Command),
            bag: Arc<Mutex<Vec<(String, Command)>>>,
        ) -> impl FnOnce(Receiver<WorkerCmd>) + Send + 'static {
            move |rx: Receiver<WorkerCmd>| match rx.recv() {
                Ok(WorkerCmd::Pause) => bag.lock().unwrap().push(label),
                Ok(other) => panic!("expected Pause, got {other:?}"),
                Err(_) => {}
            }
        }

        let h1 = fake_handle(
            "claude",
            Command::Usage,
            watcher(("claude".into(), Command::Usage), received.clone()),
        );
        let h2 = fake_handle(
            "claude",
            Command::Cost,
            watcher(("claude".into(), Command::Cost), received.clone()),
        );
        let h3 = fake_handle(
            "codex",
            Command::Usage,
            watcher(("codex".into(), Command::Usage), received.clone()),
        );

        pause_worker(&[h1, h2, h3][..], &ProviderId::new("codex"), Command::Usage);

        // Give the targeted worker a beat to wake, record, and exit.
        // Then close the other two by dropping their handles (which drops
        // their Sender, causing the `rx.recv()` to Err and the thread to
        // return cleanly so the test doesn't leak threads).
        thread::sleep(Duration::from_millis(50));
        let got = received.lock().unwrap().clone();
        assert_eq!(got, vec![("codex".to_string(), Command::Usage)]);
    }

    #[test]
    fn spawn_err_message_has_not_found_hint() {
        let m = spawn_err_message(SpawnError::NotFound(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            "x",
        )));
        assert!(m.contains("PATH"));
    }
}
