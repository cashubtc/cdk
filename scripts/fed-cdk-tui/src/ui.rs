use ratatui::layout::{Alignment, Constraint, Layout, Margin, Rect};
use ratatui::style::{Color, Modifier, Style, Stylize};
use ratatui::text::{Line, Span};
use ratatui::widgets::{
    Block, BorderType, Borders, Cell, Clear, Padding, Paragraph, Row, Sparkline, Table, Tabs, Wrap,
};
use ratatui::Frame;

use crate::app::{short, ActivityRecord, App, FormField, MemberState, Modal, Tab};

const BG: Color = Color::Rgb(10, 14, 20);
const PANEL: Color = Color::Rgb(18, 24, 33);
const MUTED: Color = Color::Rgb(122, 137, 153);
const TEXT: Color = Color::Rgb(224, 232, 240);
const CYAN: Color = Color::Rgb(69, 202, 230);
const GREEN: Color = Color::Rgb(74, 222, 128);
const AMBER: Color = Color::Rgb(251, 191, 36);
const RED: Color = Color::Rgb(248, 113, 113);
const MAGENTA: Color = Color::Rgb(232, 121, 249);

pub fn render(frame: &mut Frame<'_>, app: &mut App) {
    frame.render_widget(
        Block::default().style(Style::default().bg(BG)),
        frame.area(),
    );
    let area = frame.area();
    let layout = Layout::vertical([
        Constraint::Length(3),
        Constraint::Length(3),
        Constraint::Length(6),
        Constraint::Min(10),
        Constraint::Length(2),
    ])
    .split(area);

    render_header(frame, layout[0], app);
    render_tabs(frame, layout[1], app);
    render_summary(frame, layout[2], app);
    match app.tab {
        Tab::Overview => render_overview(frame, layout[3], app),
        Tab::Members => render_members(frame, layout[3], app),
        Tab::Activity => render_activity(frame, layout[3], app),
    }
    render_footer(frame, layout[4], app);

    if let Some((message, success, _)) = &app.toast {
        render_toast(frame, area, message, *success);
    }
    if let Some(modal) = &app.modal {
        render_modal(frame, area, modal);
    }
}

fn render_header(frame: &mut Frame<'_>, area: Rect, app: &App) {
    let federation = app
        .federation_id()
        .map(|id| short(&id, 20))
        .unwrap_or_else(|| "not connected".to_string());
    let mint_url = app
        .local_mint_url()
        .map(|url| short(&url, 30))
        .unwrap_or_default();
    let healthy = app.healthy_members();
    let total = app.members.len();
    let live_color = if total > 0 && healthy == total {
        GREEN
    } else {
        AMBER
    };
    let active_operations = app.active_operation_count();
    let busy = if active_operations > 0 {
        format!("  RUNNING {active_operations}")
    } else {
        String::new()
    };

    let line = Line::from(vec![
        Span::styled(" FED", Style::default().fg(CYAN).bold()),
        Span::styled(" CDK ", Style::default().fg(TEXT).bold()),
        Span::styled("federated wallet", Style::default().fg(MUTED)),
        Span::styled("  /  ", Style::default().fg(Color::DarkGray)),
        Span::styled(federation, Style::default().fg(TEXT)),
        Span::styled(
            if mint_url.is_empty() {
                String::new()
            } else {
                format!("  @ {mint_url}")
            },
            Style::default().fg(MUTED),
        ),
        Span::styled("  ", Style::default()),
        Span::styled("● ", Style::default().fg(live_color)),
        Span::styled(
            if total == 0 {
                "OFFLINE".to_string()
            } else {
                format!("LIVE {healthy}/{total}")
            },
            Style::default().fg(live_color).bold(),
        ),
        Span::styled(busy, Style::default().fg(MAGENTA).bold()),
    ]);
    frame.render_widget(
        Paragraph::new(line)
            .block(panel_block().borders(Borders::BOTTOM))
            .alignment(Alignment::Left),
        area,
    );
}

fn render_tabs(frame: &mut Frame<'_>, area: Rect, app: &App) {
    let titles = Tab::ALL
        .iter()
        .enumerate()
        .map(|(index, tab)| format!(" {} {} ", index + 1, tab.label()))
        .collect::<Vec<_>>();
    let selected = Tab::ALL
        .iter()
        .position(|tab| *tab == app.tab)
        .unwrap_or_default();
    frame.render_widget(
        Tabs::new(titles)
            .select(selected)
            .style(Style::default().fg(MUTED).bg(BG))
            .highlight_style(Style::default().fg(BG).bg(CYAN).bold())
            .divider("  ")
            .padding(" ", " "),
        area,
    );
}

fn render_summary(frame: &mut Frame<'_>, area: Rect, app: &App) {
    let chunks = Layout::horizontal([
        Constraint::Ratio(1, 4),
        Constraint::Ratio(1, 4),
        Constraint::Ratio(1, 4),
        Constraint::Ratio(1, 4),
    ])
    .spacing(1)
    .split(area);
    summary_card(
        frame,
        chunks[0],
        "AVAILABLE",
        &format!("{} sat", app.available),
        &format!("pending {}  reserved {}", app.pending, app.reserved),
        GREEN,
    );
    let quorum = app
        .threshold_summary()
        .map(|(members, signatures, consensus)| {
            (
                format!("{signatures} / {members}"),
                format!("consensus {consensus}"),
            )
        })
        .unwrap_or_else(|| ("--".to_string(), "join a federation".to_string()));
    summary_card(
        frame,
        chunks[1],
        "SIGNING QUORUM",
        &quorum.0,
        &quorum.1,
        CYAN,
    );
    summary_card(
        frame,
        chunks[2],
        "MEMBER HEALTH",
        &format!("{} / {}", app.healthy_members(), app.members.len()),
        "verified public metadata",
        if app.healthy_members() == app.members.len() && !app.members.is_empty() {
            GREEN
        } else {
            AMBER
        },
    );
    summary_card(
        frame,
        chunks[3],
        "NETWORK I/O",
        &format!("{} req", app.total_requests()),
        &format_bytes(app.total_bytes()),
        MAGENTA,
    );
}

fn summary_card(
    frame: &mut Frame<'_>,
    area: Rect,
    label: &str,
    value: &str,
    detail: &str,
    accent: Color,
) {
    let lines = vec![
        Line::from(Span::styled(label, Style::default().fg(MUTED))),
        Line::from(Span::styled(value, Style::default().fg(accent).bold())),
        Line::from(Span::styled(detail, Style::default().fg(MUTED))),
    ];
    frame.render_widget(
        Paragraph::new(lines)
            .block(panel_block().padding(Padding::horizontal(1)))
            .alignment(Alignment::Left),
        area,
    );
}

fn render_overview(frame: &mut Frame<'_>, area: Rect, app: &App) {
    if app.runtime().is_none() {
        render_empty(frame, area);
        return;
    }

    let chunks = if area.width >= 105 {
        Layout::horizontal([Constraint::Percentage(64), Constraint::Percentage(36)])
            .spacing(1)
            .split(area)
    } else {
        Layout::vertical([Constraint::Percentage(58), Constraint::Percentage(42)])
            .spacing(1)
            .split(area)
    };
    render_member_table(frame, chunks[0], app, true);
    render_member_detail(frame, chunks[1], app.selected_member());
}

fn render_members(frame: &mut Frame<'_>, area: Rect, app: &App) {
    if app.runtime().is_none() {
        render_empty(frame, area);
        return;
    }
    let chunks = Layout::vertical([Constraint::Percentage(60), Constraint::Percentage(40)])
        .spacing(1)
        .split(area);
    render_member_table(frame, chunks[0], app, false);
    render_member_detail(frame, chunks[1], app.selected_member());
}

fn render_member_table(frame: &mut Frame<'_>, area: Rect, app: &App, compact: bool) {
    let header = Row::new(vec![
        Cell::from("STATE"),
        Cell::from("MEMBER"),
        Cell::from("ENDPOINT"),
        Cell::from("HTTP"),
        Cell::from("LATENCY"),
        Cell::from("SUCCESS"),
        Cell::from("KEYSETS"),
    ])
    .style(Style::default().fg(CYAN).bold())
    .height(1);

    let rows = app.members.values().enumerate().map(|(index, member)| {
        let selected = index == app.selected_member;
        let state = if member.online {
            "● READY"
        } else {
            "○ DOWN"
        };
        let state_color = if member.online { GREEN } else { RED };
        let endpoint = if compact {
            short(&member.url, 31)
        } else {
            short(&member.url, 48)
        };
        let style = if selected {
            Style::default().bg(Color::Rgb(29, 41, 55)).fg(TEXT)
        } else {
            Style::default().fg(TEXT)
        };
        Row::new(vec![
            Cell::from(state).style(Style::default().fg(state_color).bold()),
            Cell::from(format!("#{:02}", member.member_id.get())),
            Cell::from(endpoint),
            Cell::from(
                member
                    .http_status
                    .map(|status| status.to_string())
                    .unwrap_or_else(|| "--".to_string()),
            ),
            Cell::from(format!("{} ms", member.latency_ms)),
            Cell::from(format!("{}%", member.success_rate())),
            Cell::from(member.active_keysets.to_string()),
        ])
        .style(style)
        .height(1)
    });

    let widths = [
        Constraint::Length(9),
        Constraint::Length(8),
        Constraint::Min(22),
        Constraint::Length(7),
        Constraint::Length(10),
        Constraint::Length(9),
        Constraint::Length(8),
    ];
    frame.render_widget(
        Table::new(rows, widths)
            .header(header)
            .column_spacing(1)
            .block(panel_block().title(" Members  ↑/↓ or j/k "))
            .row_highlight_style(Style::default().add_modifier(Modifier::BOLD)),
        area,
    );
}

fn render_member_detail(frame: &mut Frame<'_>, area: Rect, member: Option<&MemberState>) {
    let Some(member) = member else {
        frame.render_widget(
            Paragraph::new("Waiting for member data...")
                .style(Style::default().fg(MUTED))
                .block(panel_block().title(" Member detail ")),
            area,
        );
        return;
    };
    let chunks = Layout::vertical([Constraint::Min(8), Constraint::Length(3)])
        .split(area.inner(Margin::new(1, 1)));
    frame.render_widget(
        panel_block().title(format!(" Member {} ", member.member_id)),
        area,
    );

    let status_color = if member.online { GREEN } else { RED };
    let request_summary = format!(
        "{} ok / {} failed / {} total",
        member.success_count, member.failure_count, member.request_count
    );
    let response_data = format_bytes(member.response_bytes);
    let mint_methods = advertised_methods(&member.mint_methods);
    let melt_methods = advertised_methods(&member.melt_methods);
    let lines = vec![
        key_value(
            "State",
            if member.online {
                "READY"
            } else {
                "UNAVAILABLE"
            },
            status_color,
        ),
        key_value("Mint methods", &mint_methods, CYAN),
        key_value("Melt methods", &melt_methods, CYAN),
        key_value("Endpoint", &member.url, TEXT),
        key_value(
            "Mint",
            member.mint_name.as_deref().unwrap_or("not advertised"),
            TEXT,
        ),
        key_value(
            "Version",
            member.version.as_deref().unwrap_or("unknown"),
            TEXT,
        ),
        key_value(
            "Roster metadata",
            if member.metadata_matches {
                "MATCH"
            } else {
                "MISMATCH"
            },
            if member.metadata_matches { GREEN } else { RED },
        ),
        key_value("Requests", &request_summary, TEXT),
        key_value("Response data", &response_data, MAGENTA),
        key_value(
            "Last error",
            member.last_error.as_deref().unwrap_or("none"),
            if member.last_error.is_some() {
                RED
            } else {
                MUTED
            },
        ),
    ];
    frame.render_widget(Paragraph::new(lines).wrap(Wrap { trim: true }), chunks[0]);

    let samples = member.latency_history.iter().copied().collect::<Vec<_>>();
    frame.render_widget(
        Sparkline::default()
            .block(
                Block::default()
                    .title(" latency history ")
                    .title_style(Style::default().fg(MUTED)),
            )
            .data(&samples)
            .style(Style::default().fg(CYAN)),
        chunks[1],
    );
}

fn render_activity(frame: &mut Frame<'_>, area: Rect, app: &App) {
    let max_rows = area.height.saturating_sub(3) as usize;
    let rows = app.activities.iter().take(max_rows).map(activity_row);
    let header = Row::new([
        "SEQ", "UTC", "TYPE", "MEMBER", "RESULT", "LATENCY", "BYTES", "DETAIL",
    ])
    .style(Style::default().fg(CYAN).bold());
    let widths = [
        Constraint::Length(6),
        Constraint::Length(9),
        Constraint::Length(9),
        Constraint::Length(8),
        Constraint::Length(8),
        Constraint::Length(10),
        Constraint::Length(9),
        Constraint::Min(28),
    ];
    frame.render_widget(
        Table::new(rows, widths)
            .header(header)
            .column_spacing(1)
            .block(panel_block().title(" Live request / response activity ")),
        area,
    );
}

fn activity_row(activity: &ActivityRecord) -> Row<'static> {
    let result = match activity.ok {
        Some(true) => Cell::from("OK").style(Style::default().fg(GREEN).bold()),
        Some(false) => Cell::from("FAIL").style(Style::default().fg(RED).bold()),
        None => Cell::from("...").style(Style::default().fg(AMBER)),
    };
    Row::new(vec![
        Cell::from(activity.sequence.to_string()),
        Cell::from(format_time(activity.at)),
        Cell::from(activity.kind).style(Style::default().fg(MAGENTA)),
        Cell::from(
            activity
                .member_id
                .map(|member| format!("#{:02}", member.get()))
                .unwrap_or_else(|| "--".to_string()),
        ),
        result,
        Cell::from(
            activity
                .latency_ms
                .map(|latency| format!("{latency} ms"))
                .unwrap_or_else(|| "--".to_string()),
        ),
        Cell::from(
            activity
                .bytes
                .map(|bytes| format_bytes(bytes as u64))
                .unwrap_or_else(|| "--".to_string()),
        ),
        Cell::from(activity.message.clone()),
    ])
    .style(Style::default().fg(TEXT))
}

fn render_empty(frame: &mut Frame<'_>, area: Rect) {
    let content = vec![
        Line::from(Span::styled(
            "No federation connected",
            Style::default().fg(TEXT).bold(),
        )),
        Line::from(""),
        Line::from(Span::styled(
            "Press f and enter a federation-public.json path or cashufed invite.",
            Style::default().fg(MUTED),
        )),
        Line::from(Span::styled(
            "The roster, threshold, BLS keysets, and setup transcript are validated first.",
            Style::default().fg(CYAN),
        )),
    ];
    frame.render_widget(
        Paragraph::new(content)
            .alignment(Alignment::Center)
            .block(
                panel_block()
                    .title(" Federation setup ")
                    .padding(Padding::new(2, 2, 2, 1)),
            )
            .wrap(Wrap { trim: true }),
        centered_rect(72, 12, area),
    );
}

fn render_footer(frame: &mut Frame<'_>, area: Rect, app: &App) {
    let connected = app.runtime().is_some();
    let command_color = if connected { TEXT } else { Color::DarkGray };
    let methods = app
        .runtime()
        .map(|runtime| runtime.payment_capabilities())
        .unwrap_or_default();
    let method_color = |supported: bool| {
        if !connected || !supported {
            Color::DarkGray
        } else {
            TEXT
        }
    };
    let line = Line::from(vec![
        shortcut(
            "m",
            "mint",
            method_color(methods.mint_bolt11 || methods.mint_onchain),
        ),
        shortcut(
            "p",
            "melt",
            method_color(methods.melt_bolt11 || methods.melt_onchain),
        ),
        shortcut("s", "send", command_color),
        shortcut("r", "receive", command_color),
        shortcut("c", "copy", TEXT),
        shortcut("?", "help", TEXT),
        shortcut("q", "quit", TEXT),
    ]);
    frame.render_widget(
        Paragraph::new(line)
            .style(Style::default().bg(PANEL))
            .alignment(Alignment::Center),
        area,
    );
}

fn render_toast(frame: &mut Frame<'_>, area: Rect, message: &str, success: bool) {
    let width = area.width.saturating_sub(4).min(64);
    let toast_area = Rect::new(
        area.right().saturating_sub(width + 2),
        area.y.saturating_add(3),
        width,
        3,
    );
    let color = if success { GREEN } else { RED };
    frame.render_widget(Clear, toast_area);
    frame.render_widget(
        Paragraph::new(short(message, width.saturating_sub(4) as usize))
            .style(Style::default().fg(TEXT).bg(PANEL))
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .border_type(BorderType::Rounded)
                    .border_style(Style::default().fg(color))
                    .padding(Padding::horizontal(1)),
            ),
        toast_area,
    );
}

fn render_modal(frame: &mut Frame<'_>, area: Rect, modal: &Modal) {
    let modal_area = match modal {
        Modal::Payload { .. } => centered_rect(80, 18, area),
        Modal::Help => centered_rect(72, 20, area),
        Modal::Form(form) => centered_rect(
            76,
            if form.has_separate_amount_field() {
                17
            } else if form.payment_methods.is_empty() {
                11
            } else {
                14
            },
            area,
        ),
    };
    frame.render_widget(Clear, modal_area);
    match modal {
        Modal::Form(form) => {
            let mut lines = Vec::new();
            if !form.payment_methods.is_empty() {
                let mut method_line = vec![Span::styled("Method  ", Style::default().fg(MUTED))];
                for method in &form.payment_methods {
                    let selected = Some(*method) == form.selected_payment_method();
                    let style = if selected {
                        Style::default().fg(PANEL).bg(CYAN).bold()
                    } else {
                        Style::default().fg(MUTED)
                    };
                    method_line.push(Span::styled(format!(" {} ", method.label()), style));
                    method_line.push(Span::raw("  "));
                }
                lines.push(Line::from(method_line));
                lines.push(Line::from(Span::styled(
                    if form.has_separate_amount_field() {
                        "←/→ switches method   Tab or ↑/↓ switches field"
                    } else {
                        "←/→ or Tab switches payment method"
                    },
                    Style::default().fg(AMBER),
                )));
                lines.push(Line::from(""));
            }
            lines.push(form_field_label(
                form.label(),
                form.focused_field == FormField::Primary,
            ));
            lines.push(Line::from(""));
            lines.push(form_field_value(
                form.field_value(FormField::Primary),
                "type or paste here",
            ));
            if form.has_separate_amount_field() {
                lines.push(Line::from(""));
                lines.push(form_field_label(
                    "Amount (sat)",
                    form.focused_field == FormField::Amount,
                ));
                lines.push(form_field_value(
                    form.field_value(FormField::Amount),
                    "enter amount",
                ));
            }
            lines.push(Line::from(""));
            lines.push(Line::from(Span::styled(
                form.hint(),
                Style::default().fg(MUTED),
            )));
            lines.push(Line::from(Span::styled(
                if form.has_separate_amount_field() {
                    "Enter next/submit   Esc cancel   Ctrl-U clear field"
                } else {
                    "Enter submit   Esc cancel   Ctrl-U clear"
                },
                Style::default().fg(AMBER),
            )));
            frame.render_widget(
                Paragraph::new(lines)
                    .wrap(Wrap { trim: false })
                    .block(modal_block(form.title)),
                modal_area,
            );
        }
        Modal::Payload { title, content } => {
            let lines = vec![
                Line::from(Span::styled(content.clone(), Style::default().fg(TEXT))),
                Line::from(""),
                Line::from(Span::styled(
                    "c copy   m/p/s/r start another operation   Enter/Esc close",
                    Style::default().fg(AMBER),
                )),
            ];
            frame.render_widget(
                Paragraph::new(lines)
                    .wrap(Wrap { trim: false })
                    .block(modal_block(title)),
                modal_area,
            );
        }
        Modal::Help => {
            let lines = vec![
                Line::from(vec![key_span("1/2/3"), Span::raw(" switch views")]),
                Line::from(vec![key_span("↑/↓ j/k"), Span::raw(" select member")]),
                Line::from(vec![
                    key_span("m"),
                    Span::raw(" mint; select advertised method in dialog"),
                ]),
                Line::from(vec![
                    key_span("p"),
                    Span::raw(" melt; select advertised method in dialog"),
                ]),
                Line::from(vec![key_span("s"), Span::raw(" online swap and create token")]),
                Line::from(vec![key_span("r"), Span::raw(" receive pasted token")]),
                Line::from(vec![
                    key_span("c"),
                    Span::raw(" copy latest payment request or token"),
                ]),
                Line::from(vec![key_span("f"), Span::raw(" join when disconnected")]),
                Line::from(vec![key_span("q"), Span::raw(" quit safely")]),
                Line::from(""),
                Line::from(Span::styled(
                    "Member health requires successful info/keyset responses and matching federation metadata.",
                    Style::default().fg(MUTED),
                )),
                Line::from(Span::styled("Enter/Esc close", Style::default().fg(AMBER))),
            ];
            frame.render_widget(
                Paragraph::new(lines)
                    .wrap(Wrap { trim: true })
                    .block(modal_block("Keyboard")),
                modal_area,
            );
        }
    }
}

fn panel_block<'a>() -> Block<'a> {
    Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(Color::Rgb(48, 62, 78)))
        .style(Style::default().bg(PANEL).fg(TEXT))
}

fn modal_block<'a>(title: &'a str) -> Block<'a> {
    panel_block()
        .title(format!(" {title} "))
        .title_style(Style::default().fg(CYAN).bold())
        .border_style(Style::default().fg(CYAN))
        .padding(Padding::new(2, 2, 1, 1))
}

fn key_value<'a>(key: &'a str, value: &'a str, color: Color) -> Line<'a> {
    Line::from(vec![
        Span::styled(format!("{key:<17}"), Style::default().fg(MUTED)),
        Span::styled(value, Style::default().fg(color)),
    ])
}

fn form_field_label(label: &str, focused: bool) -> Line<'_> {
    let marker = if focused { "▶ " } else { "  " };
    let color = if focused { CYAN } else { MUTED };
    Line::from(Span::styled(
        format!("{marker}{label}"),
        Style::default().fg(color).bold(),
    ))
}

fn form_field_value<'a>(value: &'a str, placeholder: &'a str) -> Line<'a> {
    if value.is_empty() {
        Line::from(Span::styled(
            placeholder,
            Style::default().fg(Color::DarkGray),
        ))
    } else {
        Line::from(Span::styled(value, Style::default().fg(TEXT)))
    }
}

fn advertised_methods(methods: &[String]) -> String {
    if methods.is_empty() {
        "none advertised".to_string()
    } else {
        methods.join(", ")
    }
}

fn shortcut<'a>(key: &'a str, label: &'a str, color: Color) -> Span<'a> {
    Span::styled(
        format!("  {key} {label}  "),
        Style::default().fg(color).bg(PANEL),
    )
}

fn key_span(key: &str) -> Span<'_> {
    Span::styled(format!("{key:<10}"), Style::default().fg(CYAN).bold())
}

fn centered_rect(width: u16, height: u16, area: Rect) -> Rect {
    let width = width.min(area.width.saturating_sub(2));
    let height = height.min(area.height.saturating_sub(2));
    let x = area.x + area.width.saturating_sub(width) / 2;
    let y = area.y + area.height.saturating_sub(height) / 2;
    Rect::new(x, y, width, height)
}

fn format_time(timestamp: u64) -> String {
    let seconds = timestamp % 86_400;
    format!(
        "{:02}:{:02}:{:02}",
        seconds / 3_600,
        (seconds % 3_600) / 60,
        seconds % 60
    )
}

fn format_bytes(bytes: u64) -> String {
    if bytes >= 1_048_576 {
        format!("{:.1} MiB", bytes as f64 / 1_048_576.0)
    } else if bytes >= 1_024 {
        format!("{:.1} KiB", bytes as f64 / 1_024.0)
    } else {
        format!("{bytes} B")
    }
}
