//! Shared application state the renderer reads and the poller mutates.
//!
//! Deliberately not `Arc<Mutex<...>>`-wrapped here: the poller owns a
//! `Sender<PollEvent>`, the main loop owns the `AppState` directly and
//! applies incoming events between renders. That way the render path
//! never contends for a lock and we don't need interior mutability.

use std::collections::HashMap;
use std::time::Duration;

use crate::merge::{ProviderId, ProviderSnapshot};

/// How often each command kind is re-polled. Numbers come from
/// docs/cli-reference/timings.md (~15 s per codexbar call).
#[derive(Debug, Clone)]
pub struct RefreshIntervals {
    pub usage: Duration,
    pub cost: Duration,
}

impl Default for RefreshIntervals {
    fn default() -> Self {
        Self {
            usage: Duration::from_secs(60),
            cost: Duration::from_secs(300),
        }
    }
}

impl RefreshIntervals {
    /// Enforce the "poll floor: 30 s" rule from tui-needs.md.
    pub fn clamped(mut self) -> Self {
        let floor = Duration::from_secs(30);
        if self.usage < floor {
            self.usage = floor;
        }
        if self.cost < floor {
            self.cost = floor;
        }
        self
    }
}

/// Which codexbar call a worker thread ran.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Command {
    Usage,
    Cost,
}

#[derive(Debug)]
pub struct AppState {
    pub providers: Vec<ProviderId>,
    pub intervals: RefreshIntervals,
    pub snapshots: HashMap<ProviderId, ProviderSnapshot>,
    /// Sticky status line at the bottom: "Refreshing Claude usage…",
    /// "codexbar not found", that kind of thing.
    pub status_line: Option<String>,
    /// Body-level message shown only when `providers` is empty. Set at
    /// startup based on why the list is empty (codexbar missing vs. dump
    /// returned nothing vs. everything filtered out). The renderer reads
    /// this INSTEAD of a hardcoded empty-state string.
    pub empty_reason: Option<String>,
    pub should_quit: bool,
}

impl AppState {
    pub fn new(providers: Vec<ProviderId>, intervals: RefreshIntervals) -> Self {
        Self {
            providers,
            intervals: intervals.clamped(),
            snapshots: HashMap::new(),
            status_line: None,
            empty_reason: None,
            should_quit: false,
        }
    }

    pub fn set_empty_reason<S: Into<String>>(&mut self, msg: S) {
        self.empty_reason = Some(msg.into());
    }

    /// Write a fresh snapshot in, replacing any prior one for the provider.
    pub fn apply_snapshot(&mut self, snap: ProviderSnapshot) {
        self.snapshots.insert(snap.provider.clone(), snap);
    }

    pub fn snapshot(&self, provider: &ProviderId) -> Option<&ProviderSnapshot> {
        self.snapshots.get(provider)
    }

    pub fn quit(&mut self) {
        self.should_quit = true;
    }

    pub fn set_status<S: Into<String>>(&mut self, msg: S) {
        self.status_line = Some(msg.into());
    }

    pub fn clear_status(&mut self) {
        self.status_line = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::merge::ProviderHealth;
    use chrono::Utc;

    fn stub(provider: ProviderId) -> ProviderSnapshot {
        ProviderSnapshot {
            provider,
            fetched_at: Utc::now(),
            upstream_at: None,
            health: ProviderHealth::Ok,
            windows: Vec::new(),
            cost_today: None,
            cost_30d: None,
            top_models_today: Vec::new(),
            last_error: None,
        }
    }

    #[test]
    fn apply_snapshot_overwrites_prior() {
        let claude = ProviderId::new("claude");
        let codex = ProviderId::new("codex");
        let mut s = AppState::new(vec![claude.clone(), codex], RefreshIntervals::default());
        s.apply_snapshot(stub(claude.clone()));
        let first_fetched = s.snapshot(&claude).unwrap().fetched_at;
        std::thread::sleep(std::time::Duration::from_millis(2));
        s.apply_snapshot(stub(claude.clone()));
        assert!(s.snapshot(&claude).unwrap().fetched_at >= first_fetched);
        assert_eq!(s.snapshots.len(), 1);
    }

    #[test]
    fn intervals_clamped_to_floor() {
        let i = RefreshIntervals {
            usage: Duration::from_secs(5),
            cost: Duration::from_secs(10),
        }
        .clamped();
        assert_eq!(i.usage, Duration::from_secs(30));
        assert_eq!(i.cost, Duration::from_secs(30));
    }

    #[test]
    fn status_line_round_trips() {
        let mut s = AppState::new(
            vec![ProviderId::new("claude")],
            RefreshIntervals::default(),
        );
        assert!(s.status_line.is_none());
        s.set_status("refreshing");
        assert_eq!(s.status_line.as_deref(), Some("refreshing"));
        s.clear_status();
        assert!(s.status_line.is_none());
    }
}
