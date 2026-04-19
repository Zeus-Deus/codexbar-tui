//! Shared application state the renderer reads and the poller mutates.
//!
//! Deliberately not `Arc<Mutex<...>>`-wrapped here: the poller owns a
//! `Sender<PollEvent>`, the main loop owns the `AppState` directly and
//! applies incoming events between renders. That way the render path
//! never contends for a lock and we don't need interior mutability.

use std::collections::{HashMap, HashSet};
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
    // Held on the state so a future `r`-with-cadence override or a
    // config-reload path can read current tick intervals without
    // re-threading them through the scheduler. Main only references the
    // RENDER_TICK / SPINNER_TICK constants today.
    #[allow(dead_code)]
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
    /// When `true`, the renderer displays every provider regardless of
    /// health. When `false` (default), only providers that are healthy or
    /// still waiting for their first poll are rendered; error-state
    /// providers are hidden so the main screen only shows actionable data.
    /// Toggled by the `a` key in the event loop.
    pub show_all: bool,
    /// Providers whose on-screen data came from the on-disk cache and
    /// has NOT been confirmed by a live poll yet. The renderer pulses
    /// these panels' gauges with `Modifier::DIM` so the user can see
    /// at a glance "this is old data, a refresh is in flight".
    ///
    /// Populated at startup for every provider hydrated from
    /// `cache.rs`; cleared per-provider by main.rs when a live
    /// `PollEvent::Usage` arrives (usage is what drives the bars, so
    /// that's the moment "what's on screen" becomes live).
    pub provisional: HashSet<ProviderId>,
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
            show_all: false,
            provisional: HashSet::new(),
            should_quit: false,
        }
    }

    pub fn set_empty_reason<S: Into<String>>(&mut self, msg: S) {
        self.empty_reason = Some(msg.into());
    }

    pub fn toggle_show_all(&mut self) {
        self.show_all = !self.show_all;
    }

    /// Flag a provider as showing cached-not-live data. Called once per
    /// hydrated provider at startup.
    pub fn mark_provisional(&mut self, provider: &ProviderId) {
        self.provisional.insert(provider.clone());
    }

    /// Called from main.rs when a live `PollEvent::Usage` replaces the
    /// bars' cached values. A second call for an already-live provider
    /// is a no-op.
    pub fn mark_live(&mut self, provider: &ProviderId) {
        self.provisional.remove(provider);
    }

    pub fn is_provisional(&self, provider: &ProviderId) -> bool {
        self.provisional.contains(provider)
    }

    /// True while at least one panel is still painting cached data.
    /// The event loop ORs this with `AnimState::is_animating` to pick
    /// its render cadence: ~60 Hz while either is true (so the pulse
    /// looks smooth), back to 1 Hz once everything is both live and
    /// settled.
    pub fn has_provisional(&self) -> bool {
        !self.provisional.is_empty()
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

    #[test]
    fn show_all_defaults_off_and_toggles() {
        let mut s = AppState::new(
            vec![ProviderId::new("claude")],
            RefreshIntervals::default(),
        );
        assert!(!s.show_all, "default must be filtered view");
        s.toggle_show_all();
        assert!(s.show_all);
        s.toggle_show_all();
        assert!(!s.show_all);
    }

    #[test]
    fn provisional_lifecycle_mark_then_clear() {
        let claude = ProviderId::new("claude");
        let codex = ProviderId::new("codex");
        let mut s = AppState::new(
            vec![claude.clone(), codex.clone()],
            RefreshIntervals::default(),
        );
        assert!(!s.has_provisional(), "fresh state has no provisional marks");

        // Simulate the startup hydrate path.
        s.mark_provisional(&claude);
        s.mark_provisional(&codex);
        assert!(s.is_provisional(&claude));
        assert!(s.is_provisional(&codex));
        assert!(s.has_provisional());

        // First live usage lands for claude — only claude stops pulsing.
        s.mark_live(&claude);
        assert!(!s.is_provisional(&claude));
        assert!(s.is_provisional(&codex), "codex still cached");
        assert!(s.has_provisional());

        // Clearing already-live claude is a no-op (regression for the
        // case where cost arrives before usage and later events try to
        // clear twice).
        s.mark_live(&claude);
        assert!(!s.is_provisional(&claude));

        // Once codex also goes live, the adaptive-tick signal drops.
        s.mark_live(&codex);
        assert!(!s.has_provisional());
    }
}
