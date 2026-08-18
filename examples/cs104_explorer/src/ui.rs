// Copyright (c) 2026 Ricardo Olsen / DSC Systems. All rights reserved.
// Source-available under the DSC Systems Source-Available License; see LICENSE.

//! Rendering: the header, the three panels and the footer hint line.

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{
    Block, Borders, Cell, Paragraph, Row, Table, TableState, Tabs, Wrap,
};

use crate::app::{App, Focus, Tab};

const ACCENT: Color = Color::Magenta;
const DIM: Color = Color::DarkGray;

fn title_style() -> Style {
    Style::new().fg(ACCENT).add_modifier(Modifier::BOLD)
}

fn dim() -> Style {
    Style::new().fg(DIM)
}

/// Draw the whole screen.
pub fn draw(frame: &mut Frame, app: &App) {
    let [header, tabs, body, footer] = Layout::vertical([
        Constraint::Length(2),
        Constraint::Length(1),
        Constraint::Min(3),
        Constraint::Length(1),
    ])
    .areas(frame.area());

    draw_header(frame, app, header);
    draw_tabs(frame, app, tabs);

    // The connection editor takes over the body while it has focus.
    if app.focus == Focus::Connection {
        draw_connection(frame, app, body);
    } else {
        match app.tab {
            Tab::Points => draw_points(frame, app, body),
            Tab::Log => draw_log(frame, app, body),
            Tab::Send => draw_form(frame, app, body),
        }
    }

    frame.render_widget(
        Paragraph::new(Line::from(footer_hint(app)).style(Style::new().fg(Color::Gray))),
        footer,
    );
}

fn draw_header(frame: &mut Frame, app: &App, area: Rect) {
    let status = match (app.connected, app.active) {
        (true, true) => Span::styled(
            "● active",
            Style::new().fg(Color::Green).add_modifier(Modifier::BOLD),
        ),
        (true, false) => Span::styled(
            "● connected (STOPDT)",
            Style::new().fg(Color::Yellow).add_modifier(Modifier::BOLD),
        ),
        _ => Span::styled(
            "● disconnected",
            Style::new().fg(Color::Red).add_modifier(Modifier::BOLD),
        ),
    };

    let verbose = if app.verbose.load(std::sync::atomic::Ordering::Relaxed) {
        "on"
    } else {
        "off"
    };

    let lines = vec![
        Line::from(vec![
            Span::styled("IEC 60870-5-104 Explorer", title_style()),
            Span::raw("   "),
            status,
        ]),
        Line::styled(
            format!(
                "server {}  •  common addr {}  •  params 104-wide (COT2/CA2/IOA3)  •  protocol log {verbose}",
                app.addr, app.common_addr
            ),
            dim(),
        ),
    ];
    frame.render_widget(Paragraph::new(lines), area);
}

fn draw_tabs(frame: &mut Frame, app: &App, area: Rect) {
    let titles = ["1 Points", "2 Log", "3 Send Command"];
    let selected = Tab::ALL.iter().position(|t| *t == app.tab).unwrap_or(0);
    let tabs = Tabs::new(titles.map(Line::from).to_vec())
        .select(selected)
        .style(Style::new().fg(Color::Gray))
        .highlight_style(
            Style::new()
                .fg(Color::Black)
                .bg(ACCENT)
                .add_modifier(Modifier::BOLD),
        )
        .divider(" ");
    frame.render_widget(tabs, area);
}

fn panel(title: &str) -> Block<'_> {
    Block::default()
        .borders(Borders::ALL)
        .border_style(dim())
        .title(Span::styled(format!(" {title} "), title_style()))
}

fn draw_points(frame: &mut Frame, app: &App, area: Rect) {
    let header = Row::new(
        ["IOA", "Type", "Value", "Quality", "Cause", "Time", "Cnt"]
            .map(|h| Cell::from(h).style(title_style())),
    )
    .bottom_margin(0);

    let rows: Vec<Row> = app
        .points
        .values()
        .map(|p| {
            Row::new(vec![
                Cell::from(p.ioa.to_string()),
                Cell::from(p.type_name.clone()),
                Cell::from(p.value.clone()),
                Cell::from(p.quality.clone()),
                Cell::from(p.cause.clone()),
                Cell::from(p.time.clone()),
                Cell::from(p.count.to_string()),
            ])
        })
        .collect();

    let title = format!("Points ({})", app.points.len());
    if rows.is_empty() {
        let hint = Paragraph::new(vec![
            Line::raw(""),
            Line::styled(
                "  No points yet. Press 'c' to connect, then 'g' for a general interrogation.",
                dim(),
            ),
        ])
        .block(panel(&title));
        frame.render_widget(hint, area);
        return;
    }

    let widths = [
        Constraint::Length(9),
        Constraint::Length(12),
        Constraint::Length(20),
        Constraint::Length(16),
        Constraint::Length(22),
        Constraint::Length(13),
        Constraint::Length(5),
    ];

    let table = Table::new(rows, widths)
        .header(header)
        .block(panel(&title))
        .row_highlight_style(
            Style::new()
                .fg(Color::Black)
                .bg(ACCENT)
                .add_modifier(Modifier::BOLD),
        );

    let mut state = TableState::default().with_selected(Some(app.selected));
    frame.render_stateful_widget(table, area, &mut state);
}

fn draw_log(frame: &mut Frame, app: &App, area: Rect) {
    // Two border rows are not text.
    let visible = area.height.saturating_sub(2) as usize;
    let total = app.logs.len();

    // `log_scroll` counts lines back from the newest, so new lines keep
    // arriving at the bottom until the user scrolls up.
    let from_bottom = (app.log_scroll as usize).min(total.saturating_sub(visible.min(total)));
    let end = total - from_bottom;
    let start = end.saturating_sub(visible);

    let lines: Vec<Line> = app.logs[start..end]
        .iter()
        .map(|l| {
            let style = if l.contains("[err]") || l.contains("[E]") {
                Style::new().fg(Color::Red)
            } else if l.contains("[tx]") {
                Style::new().fg(Color::Cyan)
            } else if l.contains("[rx]") {
                Style::new().fg(Color::Green)
            } else if l.contains("[W]") {
                Style::new().fg(Color::Yellow)
            } else {
                Style::new().fg(Color::Gray)
            };
            Line::styled(l.clone(), style)
        })
        .collect();

    let title = if from_bottom > 0 {
        format!("Log ({total} lines, scrolled back {from_bottom})")
    } else {
        format!("Log ({total} lines)")
    };

    frame.render_widget(Paragraph::new(lines).block(panel(&title)), area);
}

fn draw_form(frame: &mut Frame, app: &App, area: Rect) {
    let kind = app.kind();
    let editing = app.focus == Focus::Form;

    // Mark the focused row, and show a caret in the field being typed into.
    let label = |index: usize, text: String| -> Line<'static> {
        if editing && app.form_field == index {
            Line::styled(
                format!("> {text}"),
                Style::new().fg(ACCENT).add_modifier(Modifier::BOLD),
            )
        } else {
            Line::styled(format!("  {text}"), Style::new().fg(Color::Gray))
        }
    };
    let field = |index: usize, value: &str| -> String {
        if editing && app.form_field == index {
            format!("{value}_")
        } else {
            value.to_string()
        }
    };

    let mode = if app.form_select { "Select" } else { "Execute" };
    let mut lines = vec![
        label(0, format!("Command  : ‹ {} ›", kind.name)),
        label(1, format!("IOA      : {}", field(1, &app.in_ioa.value))),
        label(2, format!("Value    : {}", field(2, &app.in_value.value))),
        label(3, format!("Mode     : ‹ {mode} ›")),
        label(
            4,
            format!("Qualifier: {}", field(4, &app.in_qualifier.value)),
        ),
        Line::raw(""),
        Line::styled(format!("value — {}", kind.value_hint), dim()),
        Line::styled(
            format!("sends to common address {} (change with 'e')", app.common_addr),
            dim(),
        ),
    ];

    if !editing {
        lines.push(Line::raw(""));
        lines.push(Line::styled(
            "press 'i' to edit and send",
            Style::new().fg(Color::Yellow).add_modifier(Modifier::BOLD),
        ));
    }

    frame.render_widget(
        Paragraph::new(lines)
            .block(panel("Send Command"))
            .wrap(Wrap { trim: false }),
        area,
    );
}

fn draw_connection(frame: &mut Frame, app: &App, area: Rect) {
    let label = |index: usize, text: String| -> Line<'static> {
        if app.conn_field == index {
            Line::styled(
                format!("> {text}_"),
                Style::new().fg(ACCENT).add_modifier(Modifier::BOLD),
            )
        } else {
            Line::styled(format!("  {text}"), Style::new().fg(Color::Gray))
        }
    };

    let lines = vec![
        Line::styled("Connection settings", title_style()),
        Line::raw(""),
        label(0, format!("Server address : {}", app.conn_addr.value)),
        label(1, format!("Common address : {}", app.conn_ca.value)),
        Line::raw(""),
        Line::styled(
            "tcp://host:port, tls://host:port, or plain host:port",
            dim(),
        ),
        Line::styled("enter: apply   •   tab: switch field   •   esc: cancel", dim()),
        Line::styled("then press 'c' to connect", dim()),
    ];

    frame.render_widget(Paragraph::new(lines).block(panel("Connection")), area);
}

/// The hint line, which depends on what has focus.
pub fn footer_hint(app: &App) -> String {
    match app.focus {
        Focus::Connection => "enter apply • tab next field • esc cancel".into(),
        Focus::Form => {
            "↑/↓ or tab: field • ←/→: change option • enter: send • esc: back".into()
        }
        Focus::Main => {
            let mut hint = String::new();
            if app.tab == Tab::Send {
                hint.push_str("i edit & send • ");
            }
            hint.push_str(
                "1/2/3 tabs • c connect • x disconnect • e edit target • \
                 g GI • C counter • y clock • t test • z reset • \
                 s/S startdt/stopdt • v log • ^L/^R clear • q quit",
            );
            hint
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event::channel;
    use crate::form::CMD_KINDS;
    use crate::handler::Point;
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use std::sync::Arc;
    use std::sync::atomic::AtomicBool;

    fn app() -> App {
        let (tx, _rx) = channel();
        App::new(tx, Arc::new(AtomicBool::new(false)), None)
    }

    /// Render into a test backend and return the screen as text.
    fn render(app: &App) -> String {
        let mut terminal = Terminal::new(TestBackend::new(100, 24)).unwrap();
        terminal.draw(|f| draw(f, app)).unwrap();
        let buffer = terminal.backend().buffer().clone();
        (0..buffer.area.height)
            .map(|y| {
                (0..buffer.area.width)
                    .map(|x| buffer[(x, y)].symbol())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn the_header_shows_the_target_and_the_connection_state() {
        let mut a = app();
        let screen = render(&a);
        assert!(screen.contains("IEC 60870-5-104 Explorer"));
        assert!(screen.contains("disconnected"));
        assert!(screen.contains("127.0.0.1:2404"));
        assert!(screen.contains("common addr 1"));

        a.connected = true;
        assert!(render(&a).contains("connected (STOPDT)"));

        a.active = true;
        assert!(render(&a).contains("active"));
    }

    #[test]
    fn the_points_panel_shows_a_hint_when_empty_and_rows_when_not() {
        let mut a = app();
        assert!(render(&a).contains("No points yet"));

        a.points.insert(
            100,
            Point {
                ioa: 100,
                type_name: "M_SP_NA_1".into(),
                value: "on".into(),
                quality: "Good".into(),
                cause: "Spontaneous".into(),
                time: "12:00:00.000".into(),
                count: 3,
            },
        );
        let screen = render(&a);
        assert!(screen.contains("Points (1)"));
        assert!(screen.contains("M_SP_NA_1"));
        assert!(screen.contains("100"));
        assert!(screen.contains("Spontaneous"));
    }

    #[test]
    fn each_panel_renders_without_panicking_at_any_size() {
        let mut a = app();
        a.log("a log line");
        a.points.insert(
            1,
            Point {
                ioa: 1,
                type_name: "M_ME_NC_1".into(),
                value: "22.5".into(),
                quality: "Good".into(),
                cause: "Periodic".into(),
                time: String::new(),
                count: 1,
            },
        );

        for tab in Tab::ALL {
            a.tab = tab;
            for focus in [Focus::Main, Focus::Form, Focus::Connection] {
                a.focus = focus;
                // A cramped terminal must not panic on the layout.
                for (w, h) in [(100u16, 24u16), (40, 10), (20, 6)] {
                    let mut terminal = Terminal::new(TestBackend::new(w, h)).unwrap();
                    terminal.draw(|f| draw(f, &a)).unwrap();
                }
            }
        }
    }

    #[test]
    fn the_log_panel_keeps_the_newest_lines_visible() {
        let mut a = app();
        a.tab = Tab::Log;
        for i in 0..200 {
            a.log(format!("entry {i}"));
        }
        let screen = render(&a);
        assert!(screen.contains("entry 199"), "the newest line must show");
        assert!(!screen.contains("entry 0 "), "old lines scroll away");
    }

    #[test]
    fn the_form_shows_the_selected_command_and_its_value_hint() {
        let mut a = app();
        a.tab = Tab::Send;
        let screen = render(&a);
        assert!(screen.contains(CMD_KINDS[0].name));
        assert!(screen.contains("press 'i' to edit"));

        a.focus = Focus::Form;
        a.form_kind = CMD_KINDS
            .iter()
            .position(|k| k.type_id == rs_iec60870_5::asdu::TypeId::C_SE_NC_1)
            .unwrap();
        let screen = render(&a);
        assert!(screen.contains("Setpoint float"));
        assert!(screen.contains("a number"), "the value hint follows the kind");
    }

    #[test]
    fn the_footer_hint_follows_the_focus() {
        let mut a = app();
        assert!(footer_hint(&a).contains("c connect"));

        a.tab = Tab::Send;
        assert!(footer_hint(&a).starts_with("i edit & send"));

        a.focus = Focus::Form;
        assert!(footer_hint(&a).contains("enter: send"));

        a.focus = Focus::Connection;
        assert!(footer_hint(&a).contains("esc cancel"));
    }
}
