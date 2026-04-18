//! ratatui rendering.
//!
//! One horizontal ROW per provider: each panel is a bordered, 3-line-tall
//! strip that spans the full terminal width. Inside the strip we lay out
//! content horizontally — windows (inline bars), then cost, then top model.
//! This trades the old column-per-provider layout (which turned into
//! postage-stamp columns once codexbar enabled more than a handful of
//! providers) for a scannable vertical list.
//!
//! No theme-file parsing: we emit `Color::{Red,Yellow,Green,Cyan,...}` and
//! let the user's terminal theme resolve them. See
//! docs/omarchy-integration.md section (d).

use chrono::{DateTime, Duration as ChronoDuration, Utc};
use ratatui::{
    Frame,
    layout::{Alignment, Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph, Wrap},
};

use crate::merge::{ModelShare, ProviderHealth, ProviderId, ProviderSnapshot, QuotaBar};
use crate::state::AppState;

/// Per-provider row height: 1 line top border (carries the title), 1 content
/// line, 1 line bottom border. 16 providers at 3 lines each = 48 lines,
/// which fits on a standard Omarchy terminal (50+ rows). On shorter
/// terminals the bottom providers get clipped by ratatui — acceptable for
/// v1; the user can trim via `hidden_providers` in config.toml.
const PROVIDER_ROW_HEIGHT: u16 = 3;

pub fn draw(f: &mut Frame, state: &AppState, now: DateTime<Utc>) {
    let size = f.area();
    let [body, footer] = vertical_split(size, [Constraint::Min(1), Constraint::Length(1)]);

    // Filter to the providers the user should see right now:
    //   * show_all on  → every configured provider
    //   * show_all off → only Ok / not-yet-polled (hides errors + auth-missing
    //                    so the main screen only contains actionable data).
    let visible: Vec<&ProviderId> = state
        .providers
        .iter()
        .filter(|p| is_visible(state.snapshot(p), state.show_all))
        .collect();
    let hidden = state.providers.len().saturating_sub(visible.len());

    if state.providers.is_empty() {
        draw_empty_state(f, body, state.empty_reason.as_deref());
    } else if visible.is_empty() {
        draw_all_hidden_state(f, body, state.providers.len(), state.show_all);
    } else {
        let constraints: Vec<Constraint> = visible
            .iter()
            .map(|_| Constraint::Length(PROVIDER_ROW_HEIGHT))
            .collect();
        let rows = Layout::default()
            .direction(Direction::Vertical)
            .constraints(constraints)
            .split(body);
        for (slot, provider) in rows.iter().zip(visible.iter()) {
            draw_provider_row(f, *slot, provider, state.snapshot(provider), now);
        }
    }

    draw_footer(f, footer, state, hidden);
}

/// Decide whether a provider panel should be visible in the current mode.
///
/// Default (`show_all == false`):
///   * `None` snapshot → show (transient "waiting for first poll…" state;
///     hiding during startup would make the screen flash).
///   * `Ok` → show.
///   * any other health → hide.
///
/// `show_all == true` always shows.
fn is_visible(snap: Option<&ProviderSnapshot>, show_all: bool) -> bool {
    if show_all {
        return true;
    }
    match snap {
        None => true,
        Some(s) => matches!(s.health, ProviderHealth::Ok),
    }
}

/// Body message shown when every configured provider is filtered out —
/// typical for a fresh Linux install where only Claude (or nothing) is
/// actually authed.
fn draw_all_hidden_state(f: &mut Frame, area: Rect, total: usize, show_all: bool) {
    let lines: Vec<Line> = if show_all {
        vec![
            Line::from(Span::styled(
                "No providers are healthy yet.",
                Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD),
            )),
            Line::from(""),
            Line::from("Resolve the per-panel errors above to see data here."),
        ]
    } else {
        vec![
            Line::from(Span::styled(
                "No working providers.",
                Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD),
            )),
            Line::from(""),
            Line::from(format!(
                "codexbar reports {total} providers configured, but none returned data."
            )),
            Line::from(Span::styled(
                "Press 'a' to see all of them with their auth/error state.",
                Style::default().fg(Color::Cyan),
            )),
        ]
    };
    let p = Paragraph::new(lines)
        .alignment(Alignment::Center)
        .wrap(Wrap { trim: true });
    f.render_widget(p, area);
}

/// Body-level empty-state renderer. Called when AppState.providers is empty.
/// The text comes from `state.empty_reason`, which main.rs populates at
/// startup based on `ProviderSource`. The generic fallback is only used if
/// main.rs somehow left it unset (shouldn't happen; kept for defensiveness).
fn draw_empty_state(f: &mut Frame, area: Rect, reason: Option<&str>) {
    let lines: Vec<Line> = match reason {
        Some(msg) => msg
            .split('\n')
            .map(|s| Line::from(s.to_string()))
            .collect(),
        None => vec![Line::from("No providers to show.".to_string())],
    };
    let p = Paragraph::new(lines)
        .alignment(Alignment::Center)
        .wrap(Wrap { trim: true });
    f.render_widget(p, area);
}

fn vertical_split<const N: usize>(area: Rect, constraints: [Constraint; N]) -> [Rect; N] {
    let rects = Layout::default()
        .direction(Direction::Vertical)
        .constraints(constraints)
        .split(area);
    let mut out = [Rect::default(); N];
    for (i, r) in rects.iter().enumerate() {
        out[i] = *r;
    }
    out
}

// ---------------------------------------------------------------------------
// Provider row
// ---------------------------------------------------------------------------

fn draw_provider_row(
    f: &mut Frame,
    area: Rect,
    provider: &ProviderId,
    snapshot: Option<&ProviderSnapshot>,
    now: DateTime<Utc>,
) {
    let (title_text, title_style) = panel_title(provider, snapshot);
    let fetched_label = snapshot.map(|s| fetched_ago(s, now));

    let mut block = Block::default()
        .title(title_text)
        .title_style(title_style)
        .borders(Borders::ALL);
    if let Some(label) = &fetched_label {
        // Tuck the staleness stamp into the bottom border on the right
        // edge so the single content line stays clean.
        block = block.title_bottom(
            Line::from(Span::styled(
                format!(" {label} "),
                Style::default().fg(Color::DarkGray),
            ))
            .right_aligned(),
        );
    }
    let inner = block.inner(area);
    f.render_widget(block, area);

    let Some(snap) = snapshot else {
        render_single_line(
            f,
            inner,
            Line::from(Span::styled(
                " waiting for first poll…",
                Style::default().fg(Color::DarkGray),
            )),
        );
        return;
    };

    if !matches!(snap.health, ProviderHealth::Ok) {
        render_single_line(f, inner, health_line(&snap.health, snap.last_error.as_deref()));
        return;
    }

    // Healthy row: horizontal split into bars | cost | top model.
    //   - bars: take whatever's left after the two right-hand fixed columns.
    //   - cost: compact "Today $x.xx   30d $y.yy" stamp.
    //   - top model: "claude-haiku-4-5  100%  $84.76".
    let cols = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Min(30),     // bars -- greedy
            Constraint::Length(26),  // cost
            Constraint::Length(34),  // top model
        ])
        .split(inner);

    draw_inline_bars(f, cols[0], &snap.windows, now);
    draw_inline_cost(f, cols[1], snap);
    draw_inline_top_model(f, cols[2], &snap.top_models_today);
}

fn render_single_line(f: &mut Frame, area: Rect, line: Line<'_>) {
    f.render_widget(
        Paragraph::new(line).alignment(Alignment::Left).wrap(Wrap { trim: true }),
        area,
    );
}

fn panel_title(provider: &ProviderId, snap: Option<&ProviderSnapshot>) -> (String, Style) {
    let (glyph, color) = match snap.map(|s| &s.health) {
        Some(ProviderHealth::Ok) => ("●", Color::Green),
        Some(ProviderHealth::Stale { .. }) => ("●", Color::Yellow),
        Some(ProviderHealth::AuthMissing) => ("●", Color::Yellow),
        Some(ProviderHealth::NotSupportedOnLinux) => ("●", Color::Red),
        Some(ProviderHealth::Error { .. }) => ("●", Color::Red),
        None => ("○", Color::DarkGray),
    };
    let text = format!(" {glyph} {} ", provider.label());
    (text, Style::default().fg(color).add_modifier(Modifier::BOLD))
}

/// One-line health message for non-Ok panels. Error / auth-missing /
/// not-supported each collapse to a single actionable string that fits
/// inside the one content line a row panel has.
fn health_line<'a>(health: &'a ProviderHealth, last_error: Option<&'a str>) -> Line<'a> {
    match health {
        ProviderHealth::AuthMissing => Line::from(vec![
            Span::styled(
                " Auth missing ",
                Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                "— run the provider CLI login (e.g. `codex login`, `claude` then /login)",
                Style::default().fg(Color::DarkGray),
            ),
        ]),
        ProviderHealth::NotSupportedOnLinux => Line::from(vec![
            Span::styled(
                " Not supported on Linux ",
                Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                "— codexbar's web source is macOS-only for this provider",
                Style::default().fg(Color::DarkGray),
            ),
        ]),
        ProviderHealth::Stale { since } => Line::from(vec![
            Span::styled(
                " Stale snapshot ",
                Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD),
            ),
            Span::raw(format!("— last good {}", fmt_utc(*since))),
        ]),
        ProviderHealth::Error { message } => Line::from(vec![
            Span::styled(
                " Error ",
                Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
            ),
            Span::raw(format!("— {}", truncate(message, 200))),
        ]),
        ProviderHealth::Ok => Line::from(Span::raw(last_error.unwrap_or(""))),
    }
}

// ---------------------------------------------------------------------------
// Inline content: bars / cost / top model
// ---------------------------------------------------------------------------

fn draw_inline_bars(f: &mut Frame, area: Rect, bars: &[QuotaBar], now: DateTime<Utc>) {
    if bars.is_empty() {
        render_single_line(
            f,
            area,
            Line::from(Span::styled(
                " no quota windows reported",
                Style::default().fg(Color::DarkGray),
            )),
        );
        return;
    }

    // Budget roughly equal space to each bar within the bars column. A
    // "slot" is label (~7) + gauge + pct% + optional countdown.
    let bar_slots = Layout::default()
        .direction(Direction::Horizontal)
        .constraints(
            std::iter::repeat_n(Constraint::Ratio(1, bars.len() as u32), bars.len())
                .collect::<Vec<_>>(),
        )
        .split(area);

    for (slot, bar) in bar_slots.iter().zip(bars.iter()) {
        let pct = bar.used_percent.min(100);
        // Gauge width: whatever's left after label + "  12% " + optional
        // countdown. Keep a floor of 4 characters so something always
        // renders even in a tight column.
        let total = slot.width as usize;
        let countdown = countdown_text(bar.resets_at, now);
        let countdown_len = countdown.as_deref().map(|s| s.len() + 1).unwrap_or(0);
        let overhead = bar.window_label.chars().count() + 1 + 5 + countdown_len + 2;
        let gauge_width = total.saturating_sub(overhead).max(4);

        let mut spans = vec![
            Span::styled(
                bar.window_label.clone(),
                Style::default().add_modifier(Modifier::BOLD),
            ),
            Span::raw(" "),
            Span::styled(
                text_gauge(pct, gauge_width),
                Style::default().fg(bar_color(pct)),
            ),
            Span::raw(" "),
            Span::styled(
                format!("{pct:>3}%"),
                Style::default().add_modifier(Modifier::BOLD),
            ),
        ];
        if let Some(cd) = countdown {
            spans.push(Span::raw(" "));
            spans.push(Span::styled(cd, Style::default().fg(Color::Cyan)));
        }
        f.render_widget(Paragraph::new(Line::from(spans)), *slot);
    }
}

/// Unicode-block text gauge. `pct` is 0..=100; width is the number of block
/// cells between the brackets.
fn text_gauge(pct: u8, width: usize) -> String {
    let filled = (pct as usize * width + 50) / 100; // round-to-nearest
    let filled = filled.min(width);
    let empty = width - filled;
    let mut s = String::with_capacity(width + 2);
    s.push('▕');
    for _ in 0..filled {
        s.push('█');
    }
    for _ in 0..empty {
        s.push('░');
    }
    s.push('▏');
    s
}

fn draw_inline_cost(f: &mut Frame, area: Rect, snap: &ProviderSnapshot) {
    let today = snap
        .cost_today
        .map(|c| format!("${c:.2}"))
        .unwrap_or_else(|| "—".into());
    let month = snap
        .cost_30d
        .map(|c| format!("${c:.2}"))
        .unwrap_or_else(|| "—".into());
    let line = Line::from(vec![
        Span::styled("Today ", Style::default().add_modifier(Modifier::BOLD)),
        Span::styled(today, Style::default().fg(Color::Cyan)),
        Span::raw("  "),
        Span::styled("30d ", Style::default().add_modifier(Modifier::BOLD)),
        Span::styled(month, Style::default().fg(Color::Cyan)),
    ]);
    f.render_widget(Paragraph::new(line), area);
}

fn draw_inline_top_model(f: &mut Frame, area: Rect, models: &[ModelShare]) {
    let line = match models.first() {
        Some(m) => Line::from(vec![
            Span::styled(
                truncate(&m.model, 18).to_string(),
                Style::default().add_modifier(Modifier::BOLD),
            ),
            Span::raw("  "),
            Span::styled(
                format!("{:>3}%", m.percent_of_day),
                Style::default().fg(Color::Cyan),
            ),
            Span::raw("  "),
            Span::styled(
                format!("${:.2}", m.cost),
                Style::default().fg(Color::Green),
            ),
        ]),
        None => Line::from(Span::styled(
            "(no models today)",
            Style::default().fg(Color::DarkGray),
        )),
    };
    f.render_widget(Paragraph::new(line), area);
}

fn bar_color(pct: u8) -> Color {
    match pct {
        0..=59 => Color::Green,
        60..=84 => Color::Yellow,
        _ => Color::Red,
    }
}

/// Format a human-readable countdown from `resets_at - now`. Returns None
/// when `resets_at` is None or already in the past.
fn countdown_text(resets_at: Option<DateTime<Utc>>, now: DateTime<Utc>) -> Option<String> {
    let target = resets_at?;
    let delta = target - now;
    if delta <= ChronoDuration::zero() {
        return None;
    }
    let total_secs = delta.num_seconds();
    let days = total_secs / 86_400;
    let hours = (total_secs % 86_400) / 3600;
    let minutes = (total_secs % 3600) / 60;
    let seconds = total_secs % 60;
    let body = if days > 0 {
        format!("{days}d {hours}h")
    } else if hours > 0 {
        format!("{hours}h {minutes}m")
    } else if minutes > 0 {
        format!("{minutes}m {seconds}s")
    } else {
        format!("{seconds}s")
    };
    Some(body)
}

fn truncate(s: &str, max: usize) -> &str {
    if s.len() <= max {
        s
    } else {
        &s[..max]
    }
}

fn fetched_ago(snap: &ProviderSnapshot, now: DateTime<Utc>) -> String {
    let age = now - snap.fetched_at;
    if age < ChronoDuration::seconds(2) {
        "just now".to_string()
    } else if age < ChronoDuration::minutes(1) {
        format!("{}s ago", age.num_seconds())
    } else if age < ChronoDuration::hours(1) {
        format!("{}m ago", age.num_minutes())
    } else {
        format!("{}h ago", age.num_hours())
    }
}

// ---------------------------------------------------------------------------
// Global footer (below all panels)
// ---------------------------------------------------------------------------

fn draw_footer(f: &mut Frame, area: Rect, state: &AppState, hidden: usize) {
    // Left half: sticky status line from AppState, or the "q / r" hint
    // when nothing is status-worthy. Right half: mode indicator for the
    // `a` toggle so the user can always see how many panels are hidden.
    let cols = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Min(1), Constraint::Length(36)])
        .split(area);

    let status = state
        .status_line
        .clone()
        .unwrap_or_else(|| "q: quit   r: refresh".to_string());
    f.render_widget(
        Paragraph::new(Line::from(Span::styled(
            status,
            Style::default().fg(Color::DarkGray),
        )))
        .alignment(Alignment::Left),
        cols[0],
    );

    let mode = if state.show_all {
        "a: show working only".to_string()
    } else if hidden > 0 {
        format!("{hidden} hidden   a: show all")
    } else {
        "a: show all".to_string()
    };
    f.render_widget(
        Paragraph::new(Line::from(Span::styled(
            mode,
            Style::default().fg(Color::DarkGray),
        )))
        .alignment(Alignment::Right),
        cols[1],
    );
}

fn fmt_utc(t: DateTime<Utc>) -> String {
    t.format("%Y-%m-%d %H:%M UTC").to_string()
}

// ---------------------------------------------------------------------------
// Tests — pure logic functions only. Full ratatui render-loop smoke tests
// would require a TestBackend harness; keeping v1 lean.
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bar_color_thresholds() {
        assert_eq!(bar_color(0), Color::Green);
        assert_eq!(bar_color(59), Color::Green);
        assert_eq!(bar_color(60), Color::Yellow);
        assert_eq!(bar_color(84), Color::Yellow);
        assert_eq!(bar_color(85), Color::Red);
        assert_eq!(bar_color(100), Color::Red);
    }

    #[test]
    fn countdown_human_format() {
        let now: DateTime<Utc> = "2026-04-18T12:00:00Z".parse().unwrap();
        let h5: DateTime<Utc> = "2026-04-18T17:00:00Z".parse().unwrap();
        let d2: DateTime<Utc> = "2026-04-20T12:00:00Z".parse().unwrap();
        let m1: DateTime<Utc> = "2026-04-18T12:01:10Z".parse().unwrap();

        assert_eq!(countdown_text(Some(h5), now), Some("5h 0m".into()));
        assert_eq!(countdown_text(Some(d2), now), Some("2d 0h".into()));
        assert_eq!(countdown_text(Some(m1), now), Some("1m 10s".into()));
    }

    #[test]
    fn countdown_none_when_in_past_or_missing() {
        let now: DateTime<Utc> = "2026-04-18T12:00:00Z".parse().unwrap();
        let past: DateTime<Utc> = "2026-04-17T12:00:00Z".parse().unwrap();
        assert!(countdown_text(Some(past), now).is_none());
        assert!(countdown_text(None, now).is_none());
    }

    #[test]
    fn truncate_handles_short_and_long() {
        assert_eq!(truncate("short", 10), "short");
        assert_eq!(truncate("longer-than-limit", 6), "longer");
    }

    #[test]
    fn text_gauge_is_correct_width_and_shape() {
        // 0% -> all empty.
        assert_eq!(text_gauge(0, 10), "▕░░░░░░░░░░▏");
        // 100% -> all full.
        assert_eq!(text_gauge(100, 10), "▕██████████▏");
        // 50% on width 10 rounds cleanly.
        assert_eq!(text_gauge(50, 10), "▕█████░░░░░▏");
        // Width is preserved for odd percentages (round-to-nearest).
        let g = text_gauge(33, 10);
        let inside: String = g.chars().filter(|&c| c == '█' || c == '░').collect();
        assert_eq!(inside.chars().count(), 10);
    }

    fn snap_with(health: ProviderHealth) -> ProviderSnapshot {
        ProviderSnapshot {
            provider: ProviderId::new("test"),
            fetched_at: Utc::now(),
            upstream_at: None,
            health,
            windows: Vec::new(),
            cost_today: None,
            cost_30d: None,
            top_models_today: Vec::new(),
            last_error: None,
        }
    }

    #[test]
    fn is_visible_default_hides_errors_shows_ok_and_loading() {
        // Default mode (show_all = false)
        assert!(is_visible(None, false), "loading rows stay visible during boot");
        assert!(is_visible(Some(&snap_with(ProviderHealth::Ok)), false));
        assert!(!is_visible(Some(&snap_with(ProviderHealth::AuthMissing)), false));
        assert!(!is_visible(
            Some(&snap_with(ProviderHealth::NotSupportedOnLinux)),
            false
        ));
        assert!(!is_visible(
            Some(&snap_with(ProviderHealth::Error {
                message: "boom".into()
            })),
            false
        ));
        assert!(!is_visible(
            Some(&snap_with(ProviderHealth::Stale {
                since: Utc::now()
            })),
            false
        ));
    }

    #[test]
    fn is_visible_show_all_keeps_everything() {
        assert!(is_visible(None, true));
        assert!(is_visible(Some(&snap_with(ProviderHealth::Ok)), true));
        assert!(is_visible(Some(&snap_with(ProviderHealth::AuthMissing)), true));
        assert!(is_visible(
            Some(&snap_with(ProviderHealth::NotSupportedOnLinux)),
            true
        ));
        assert!(is_visible(
            Some(&snap_with(ProviderHealth::Error {
                message: "x".into()
            })),
            true
        ));
    }

    #[test]
    fn text_gauge_never_overshoots() {
        for pct in 0u8..=100 {
            for width in [4usize, 10, 20] {
                let g = text_gauge(pct, width);
                let inside: String = g.chars().filter(|&c| c == '█' || c == '░').collect();
                assert_eq!(inside.chars().count(), width, "pct={pct} width={width}");
            }
        }
    }
}
