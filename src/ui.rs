//! ratatui rendering.
//!
//! One vertical panel per enabled provider. Each panel contains:
//!   * title bar (provider label + health indicator)
//!   * session quota bar (5h)
//!   * weekly bar (7d)
//!   * weekly opus bar (Claude only, when present)
//!   * reset countdowns (computed every frame from resets_at - now)
//!   * cost today / last-30-days
//!   * top 3 models by cost
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
    widgets::{Block, Borders, Gauge, Paragraph, Wrap},
};

use crate::merge::{ModelShare, ProviderHealth, ProviderId, ProviderSnapshot, QuotaBar};
use crate::state::AppState;

pub fn draw(f: &mut Frame, state: &AppState, now: DateTime<Utc>) {
    let size = f.area();
    let [body, footer] = vertical_split(size, [Constraint::Min(1), Constraint::Length(1)]);

    if state.providers.is_empty() {
        draw_empty_state(f, body, state.empty_reason.as_deref());
    } else {
        let constraints: Vec<Constraint> = state
            .providers
            .iter()
            .map(|_| Constraint::Percentage(100 / state.providers.len().max(1) as u16))
            .collect();
        let columns = Layout::default()
            .direction(Direction::Horizontal)
            .constraints(constraints)
            .split(body);
        for (slot, provider) in columns.iter().zip(state.providers.iter()) {
            draw_provider(f, *slot, provider, state.snapshot(provider), now);
        }
    }

    draw_footer(f, footer, state);
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
// Provider panel
// ---------------------------------------------------------------------------

fn draw_provider(
    f: &mut Frame,
    area: Rect,
    provider: &ProviderId,
    snapshot: Option<&ProviderSnapshot>,
    now: DateTime<Utc>,
) {
    let (title_text, title_style) = panel_title(provider, snapshot);
    let block = Block::default()
        .title(title_text)
        .title_style(title_style)
        .borders(Borders::ALL);
    let inner = block.inner(area);
    f.render_widget(block, area);

    let Some(snap) = snapshot else {
        let p = Paragraph::new(Line::from(Span::styled(
            "waiting for first poll…",
            Style::default().fg(Color::DarkGray),
        )))
        .alignment(Alignment::Center);
        f.render_widget(p, inner);
        return;
    };

    if !matches!(snap.health, ProviderHealth::Ok) {
        let p = Paragraph::new(health_message(&snap.health, snap.last_error.as_deref()))
            .wrap(Wrap { trim: true })
            .alignment(Alignment::Center);
        f.render_widget(p, inner);
        return;
    }

    // Healthy panel layout — dynamic, one 3-row bar per present window:
    //   N x quota bars  (3 each)
    //   spacer          (1)
    //   cost line       (1)
    //   models          (0..=3)
    //   fill            (>=1)
    //   fetched-ago     (1)
    let bar_rows = snap.windows.len() as u16 * 3;
    let model_rows = models_rows(&snap.top_models_today) as u16;
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(bar_rows),
            Constraint::Length(1), // spacer
            Constraint::Length(1), // cost line
            Constraint::Length(model_rows),
            Constraint::Min(1),    // fill
            Constraint::Length(1), // fetched-ago
        ])
        .split(inner);

    // Split the bar-region into one Rect per window, stacked.
    if !snap.windows.is_empty() {
        let bar_constraints: Vec<Constraint> =
            snap.windows.iter().map(|_| Constraint::Length(3)).collect();
        let bar_slots = Layout::default()
            .direction(Direction::Vertical)
            .constraints(bar_constraints)
            .split(rows[0]);
        for (slot, bar) in bar_slots.iter().zip(snap.windows.iter()) {
            draw_bar(f, *slot, bar, now);
        }
    }
    draw_cost_line(f, rows[2], snap);
    draw_models(f, rows[3], &snap.top_models_today);
    draw_footer_line(f, rows[5], snap, now);
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

fn health_message(health: &ProviderHealth, last_error: Option<&str>) -> Vec<Line<'static>> {
    match health {
        ProviderHealth::AuthMissing => vec![
            Line::from(Span::styled(
                "Authentication missing",
                Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD),
            )),
            Line::from(""),
            Line::from("Run the provider's CLI login, e.g.:"),
            Line::from(Span::styled("  codex login", Style::default().fg(Color::Cyan))),
            Line::from(Span::styled("  claude  (then /login)", Style::default().fg(Color::Cyan))),
        ],
        ProviderHealth::NotSupportedOnLinux => vec![
            Line::from(Span::styled(
                "Not supported on Linux",
                Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
            )),
            Line::from(""),
            Line::from("codexbar --source auto is macOS-only."),
            Line::from("Use --source cli (the TUI does this by default)."),
        ],
        ProviderHealth::Stale { since } => vec![
            Line::from(Span::styled(
                "Stale snapshot",
                Style::default().fg(Color::Yellow),
            )),
            Line::from(format!("last good {}", fmt_utc(*since))),
        ],
        ProviderHealth::Error { message } => vec![
            Line::from(Span::styled(
                "Error",
                Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
            )),
            Line::from(Span::raw(message.clone())),
        ],
        ProviderHealth::Ok => vec![Line::from(last_error.unwrap_or("").to_string())],
    }
}

// ---------------------------------------------------------------------------
// Quota bar
// ---------------------------------------------------------------------------

fn draw_bar(f: &mut Frame, area: Rect, bar: &QuotaBar, now: DateTime<Utc>) {
    let [head, gauge] = vertical_split(area, [Constraint::Length(1), Constraint::Length(2)]);

    // Label comes straight from the window itself (e.g. "5h", "weekly",
    // "Xd"). No provider-specific labeling lives in the renderer.
    let mut spans = vec![
        Span::styled(
            bar.window_label.clone(),
            Style::default().add_modifier(Modifier::BOLD),
        ),
    ];
    if let Some(countdown) = countdown_text(bar.resets_at, now) {
        spans.push(Span::raw("  "));
        spans.push(Span::styled(
            countdown,
            Style::default().fg(Color::Cyan),
        ));
    } else if let Some(hint) = &bar.reset_hint {
        spans.push(Span::raw("  "));
        spans.push(Span::styled(
            hint.clone(),
            Style::default().fg(Color::DarkGray),
        ));
    }
    let header = Paragraph::new(Line::from(spans));
    f.render_widget(header, head);

    let pct = bar.used_percent.min(100);
    let color = bar_color(pct);
    let g = Gauge::default()
        .gauge_style(Style::default().fg(color))
        .percent(pct as u16)
        .label(Span::styled(
            format!("{pct}%"),
            Style::default().add_modifier(Modifier::BOLD),
        ));
    f.render_widget(g, gauge);
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
        format!("{days}d {hours}h left")
    } else if hours > 0 {
        format!("{hours}h {minutes}m left")
    } else if minutes > 0 {
        format!("{minutes}m {seconds}s left")
    } else {
        format!("{seconds}s left")
    };
    Some(body)
}

// ---------------------------------------------------------------------------
// Cost line + model breakdown
// ---------------------------------------------------------------------------

fn draw_cost_line(f: &mut Frame, area: Rect, snap: &ProviderSnapshot) {
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
        Span::raw("   "),
        Span::styled("30d ", Style::default().add_modifier(Modifier::BOLD)),
        Span::styled(month, Style::default().fg(Color::Cyan)),
    ]);
    f.render_widget(Paragraph::new(line), area);
}

fn models_rows(models: &[ModelShare]) -> usize {
    if models.is_empty() { 1 } else { models.len() }
}

fn draw_models(f: &mut Frame, area: Rect, models: &[ModelShare]) {
    if models.is_empty() {
        f.render_widget(
            Paragraph::new(Line::from(Span::styled(
                "(no models today)",
                Style::default().fg(Color::DarkGray),
            ))),
            area,
        );
        return;
    }
    let lines: Vec<Line> = models
        .iter()
        .map(|m| {
            Line::from(vec![
                Span::styled(
                    format!("{:>3}%  ", m.percent_of_day),
                    Style::default().fg(Color::Cyan),
                ),
                Span::styled(
                    truncate(&m.model, 24).to_string(),
                    Style::default().add_modifier(Modifier::BOLD),
                ),
                Span::raw("  "),
                Span::styled(
                    format!("${:.2}", m.cost),
                    Style::default().fg(Color::Green),
                ),
            ])
        })
        .collect();
    f.render_widget(Paragraph::new(lines), area);
}

fn truncate(s: &str, max: usize) -> &str {
    if s.len() <= max {
        s
    } else {
        &s[..max]
    }
}

fn draw_footer_line(f: &mut Frame, area: Rect, snap: &ProviderSnapshot, now: DateTime<Utc>) {
    let age = now - snap.fetched_at;
    let ago = if age < ChronoDuration::seconds(2) {
        "just now".to_string()
    } else if age < ChronoDuration::minutes(1) {
        format!("{}s ago", age.num_seconds())
    } else if age < ChronoDuration::hours(1) {
        format!("{}m ago", age.num_minutes())
    } else {
        format!("{}h ago", age.num_hours())
    };
    let p = Paragraph::new(Line::from(vec![
        Span::styled("fetched ", Style::default().fg(Color::DarkGray)),
        Span::styled(ago, Style::default().fg(Color::DarkGray)),
    ]))
    .alignment(Alignment::Right);
    f.render_widget(p, area);
}

// ---------------------------------------------------------------------------
// Global footer (below all panels)
// ---------------------------------------------------------------------------

fn draw_footer(f: &mut Frame, area: Rect, state: &AppState) {
    let msg = state
        .status_line
        .clone()
        .unwrap_or_else(|| "q: quit   r: refresh".to_string());
    let p = Paragraph::new(Line::from(Span::styled(
        msg,
        Style::default().fg(Color::DarkGray),
    )))
    .alignment(Alignment::Left);
    f.render_widget(p, area);
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

        assert_eq!(countdown_text(Some(h5), now), Some("5h 0m left".into()));
        assert_eq!(countdown_text(Some(d2), now), Some("2d 0h left".into()));
        assert_eq!(countdown_text(Some(m1), now), Some("1m 10s left".into()));
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
}
