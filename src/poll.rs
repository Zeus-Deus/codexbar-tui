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
    /// Wake up and re-poll immediately (then resume normal cadence).
    Refresh,
    /// Exit the worker thread.
    Quit,
}

/// Handle on a running worker. Dropping it does not kill the worker; send
/// `WorkerCmd::Quit` for a clean exit.
pub struct WorkerHandle {
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

    for &p in providers {
        handles.push(spawn_worker(p, Command::Usage, usage_interval, ev_tx.clone()));
        handles.push(spawn_worker(p, Command::Cost, cost_interval, ev_tx.clone()));
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
    let join = thread::Builder::new()
        .name(format!("poll-{:?}-{:?}", provider, command))
        .spawn(move || worker_loop(provider, command, interval, ev_tx, cmd_rx))
        .expect("spawn worker thread");
    WorkerHandle { tx: cmd_tx, join }
}

fn worker_loop(
    provider: ProviderId,
    command: Command,
    interval: Duration,
    ev_tx: Sender<PollEvent>,
    cmd_rx: Receiver<WorkerCmd>,
) {
    loop {
        run_once(provider, command, &ev_tx);

        // Wait for the next tick, but listen for manual refresh / quit.
        match cmd_rx.recv_timeout(interval) {
            Ok(WorkerCmd::Refresh) => continue, // loop around and re-poll now
            Ok(WorkerCmd::Quit) => return,
            Err(RecvTimeoutError::Timeout) => continue, // normal cadence
            Err(RecvTimeoutError::Disconnected) => return, // main dropped its sender
        }
    }
}

fn run_once(provider: ProviderId, command: Command, ev_tx: &Sender<PollEvent>) {
    let cli_id = provider.cli_id();
    let result = match command {
        Command::Usage => spawn::usage_cli(cli_id, None).map(Output::Usage),
        Command::Cost => spawn::cost(cli_id, None).map(Output::Cost),
    };

    let event = match result {
        Ok(Output::Usage(out)) => match parse::parse_usage(&out.stdout) {
            Ok(records) => PollEvent::Usage { provider, records },
            Err(e) => PollEvent::Error {
                provider,
                command,
                message: format!("usage parse: {e}"),
            },
        },
        Ok(Output::Cost(out)) => match parse::parse_cost(&out.stdout) {
            Ok(mut recs) => {
                // cost emits one record per provider at top-level; we
                // requested --provider <id>, so take the first.
                let record = recs.drain(..).next();
                PollEvent::Cost { provider, record }
            }
            Err(e) => PollEvent::Error {
                provider,
                command,
                message: format!("cost parse: {e}"),
            },
        },
        Err(e) => PollEvent::Error {
            provider,
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

    #[test]
    fn broadcast_refresh_delivers_to_every_worker() {
        // Build a pair of pseudo-workers: each is a thread that receives
        // a single message and shuts down. We only exercise the tx side.
        let mut handles = Vec::new();
        for _ in 0..3 {
            let (tx, rx) = mpsc::channel::<WorkerCmd>();
            let join = thread::spawn(move || match rx.recv() {
                Ok(WorkerCmd::Refresh) => {}
                other => panic!("expected Refresh, got {other:?}"),
            });
            handles.push(WorkerHandle { tx, join });
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
            let (tx, rx) = mpsc::channel::<WorkerCmd>();
            let join = thread::spawn(move || loop {
                match rx.recv() {
                    Ok(WorkerCmd::Quit) => return,
                    Ok(WorkerCmd::Refresh) => continue,
                    Err(_) => return,
                }
            });
            handles.push(WorkerHandle { tx, join });
        }
        shutdown(handles);
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
