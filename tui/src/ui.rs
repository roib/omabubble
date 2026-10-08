//! Rendering. Reads the `App` and writes back only render metrics
//! (scroll clamping, total lines, pane height).

use crate::app::{App, Focus, Item, PendState, Status, Target, View, attachment_label, preview};
use chrono::{DateTime, Datelike, Local, TimeZone};
use proto::{Kind, Message};
use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Style, Stylize};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Block, Clear, List, ListItem, Paragraph};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

const LIST_WIDTH: u16 = 34;
const TIME_COL: usize = 7; // "10:42  "
/// Reserved for time + marker on sent messages ("10:42 SMS"), so text ends at
/// the same column whether or not a marker is present.
const STATUS_COL: usize = 9;
const MAX_INPUT_ROWS: usize = 5;

pub fn draw(f: &mut Frame, app: &mut App) {
    let [main, input_area, status_area] = {
        let width = f.area().width.saturating_sub(LIST_WIDTH + 2).max(10) as usize;
        let rows = input_layout(&app.input, app.cursor, width)
            .0
            .len()
            .clamp(1, MAX_INPUT_ROWS) as u16;
        Layout::vertical([
            Constraint::Min(5),
            Constraint::Length(rows + 2),
            Constraint::Length(1),
        ])
        .areas(f.area())
    };
    let [list_area, msg_area] =
        Layout::horizontal([Constraint::Length(LIST_WIDTH), Constraint::Min(20)]).areas(main);

    draw_list(f, app, list_area);
    draw_messages(f, app, msg_area);
    draw_input(f, app, input_area);
    draw_status(f, app, status_area);
    if app.popup.is_some() {
        draw_popup(f, app);
    }
    match app.emoji.as_ref().map(|p| p.inline.is_some()) {
        Some(true) => draw_emoji_inline(f, app, input_area),
        Some(false) => draw_emoji_picker(f, app),
        None => {}
    }
}

fn border(focused: bool) -> Style {
    if focused {
        Style::new().fg(Color::Cyan)
    } else {
        Style::new().fg(Color::DarkGray)
    }
}

// ------------------------------------------------------------------ chat list

fn draw_list(f: &mut Frame, app: &mut App, area: Rect) {
    let block = Block::bordered()
        .title(" Chats ")
        .border_style(border(app.focus == Focus::List));
    app.rects.list = block.inner(area);
    let inner_w = area.width.saturating_sub(2) as usize;
    if app.chats.is_empty() {
        let msg = if app.status == Status::Connected {
            "no chats"
        } else {
            "loading…"
        };
        f.render_widget(Paragraph::new(msg.dark_gray()).block(block), area);
        return;
    }
    let now = Local::now();
    let items: Vec<ListItem> = app
        .chats
        .iter()
        .map(|c| {
            let time = c
                .last
                .as_ref()
                .map(|m| list_time(m.ts, &now))
                .unwrap_or_default();
            let unread = c.unread > 0;
            let mut name = c.display.clone();
            if c.service == "SMS" {
                name.push_str(" [SMS]");
            }
            if c.unread > 0 {
                name = format!("({}) {name}", c.unread);
            }
            let marker = if unread { "* " } else { "  " };
            let name_w = inner_w.saturating_sub(2 + time.width() + 1);
            let name = truncate(&name, name_w);
            let pad = inner_w.saturating_sub(2 + name.width() + time.width());
            let name_style = if unread {
                Style::new().bold()
            } else {
                Style::new()
            };
            let l1 = Line::from(vec![
                Span::styled(marker, Style::new().fg(Color::Yellow).bold()),
                Span::styled(name, name_style),
                Span::raw(" ".repeat(pad)),
                Span::styled(time, Style::new().dark_gray()),
            ]);
            let prev = c.last.as_ref().map(preview).unwrap_or_default();
            let l2 = Line::from(Span::styled(
                format!("  {}", truncate(&prev, inner_w.saturating_sub(2))),
                Style::new().dark_gray(),
            ));
            ListItem::new(Text::from(vec![l1, l2]))
        })
        .collect();
    let list = List::new(items)
        .block(block)
        .highlight_style(Style::new().bg(Color::DarkGray));
    app.list_state.select(Some(app.selected));
    f.render_stateful_widget(list, area, &mut app.list_state);
}

// ------------------------------------------------------------------ messages

fn draw_messages(f: &mut Frame, app: &mut App, area: Rect) {
    let focused = app.focus == Focus::Messages;
    let title = match app.view.as_ref().map(|v| &v.target) {
        Some(Target::Chat(_)) => app
            .chat_for_view()
            .map(|c| {
                // A named 1:1 chat keeps its handles visible in the title.
                let handles = c.handles.join(" / ");
                let who = if c.is_group || c.display == handles {
                    c.display.clone()
                } else {
                    format!("{} · {handles}", c.display)
                };
                format!(" {who}{} ", if c.service == "SMS" { " [SMS]" } else { "" })
            })
            .unwrap_or_else(|| " … ".into()),
        Some(Target::Draft(h)) => format!(" {h} (new) "),
        None => " imsg ".into(),
    };
    let mut block = Block::bordered().title(title).border_style(border(focused));
    if focused {
        let mode = if app.view.as_ref().is_some_and(|v| v.anchor.is_some()) {
            " RANGE "
        } else {
            " SELECT "
        };
        block = block.title_top(Line::from(mode.black().on_cyan()).right_aligned());
    }
    let inner = block.inner(area);
    app.rects.messages = inner;
    f.render_widget(block, area);

    if app.view.is_none() {
        let hint = vec![
            Line::from(""),
            Line::from(
                "Select a chat and press Enter, or press n for a new conversation.".dark_gray(),
            ),
        ];
        f.render_widget(Paragraph::new(hint).centered(), inner);
        return;
    }

    let width = inner.width as usize;
    let height = inner.height as usize;
    let (mut lines, ranges) = build_lines(app, width);

    let v = app.view.as_mut().unwrap();
    let total = lines.len();
    if v.appended && v.scroll > 0 && total > v.total_lines {
        // Keep the viewport still while new messages arrive below it.
        v.scroll += total - v.total_lines;
    }
    v.appended = false;
    v.total_lines = total;
    v.height = height;

    // Highlight the selection (or range) in the Messages pane.
    if focused && let Some(sel) = v.sel {
        let (a, b) = match v.anchor {
            Some(a) => (a.min(sel), a.max(sel)),
            None => (sel, sel),
        };
        for (s, e) in ranges.iter().take(b + 1).skip(a) {
            for line in &mut lines[*s..*e] {
                *line = std::mem::take(line).patch_style(Style::new().bg(Color::DarkGray));
            }
        }
        if v.follow_sel
            && let Some(&(s, e)) = ranges.get(sel)
        {
            follow(v, s, e);
        }
    }
    v.follow_sel = false;
    v.scroll = v.scroll.min(total.saturating_sub(height));

    let start = total.saturating_sub(height + v.scroll);
    let end = (start + height).min(total);
    let mut visible: Vec<Line> = lines.drain(start..end).collect();
    // Short conversations sit at the bottom, like a chat.
    let pad = height.saturating_sub(visible.len());
    if pad > 0 {
        let mut padded = vec![Line::from(""); pad];
        padded.append(&mut visible);
        visible = padded;
    }
    v.first_line = start;
    v.pad = pad;
    v.ranges = ranges;
    f.render_widget(Paragraph::new(visible), inner);
}

fn follow(v: &mut View, s: usize, e: usize) {
    let (total, height) = (v.total_lines, v.height);
    let top = total.saturating_sub(height + v.scroll);
    if s < top {
        v.scroll = total.saturating_sub(height + s);
    } else if e > top + height {
        v.scroll = total.saturating_sub(e);
    }
}

/// All lines of the open conversation, plus each item's [start, end) line range.
fn build_lines(app: &App, width: usize) -> (Vec<Line<'static>>, Vec<(usize, usize)>) {
    let v = app.view.as_ref().unwrap();
    let is_group = app.chat_for_view().is_some_and(|c| c.is_group);
    let mut lines: Vec<Line<'static>> = Vec::new();
    let mut ranges = Vec::new();

    if v.loading {
        lines.push(Line::from("loading…".dark_gray()).centered());
    } else if v.no_more || v.messages.is_empty() {
        lines.push(Line::from("— start of conversation —".dark_gray()).centered());
    }

    // Messages shows the receipt under your latest sent message only.
    let items = app.items();
    let last_mine = items.iter().rposition(
        |it| matches!(it, Item::Msg(m) if m.from_me && !m.unsent && m.tapback.is_none()),
    );

    let mut last_day = None;
    for (i, item) in items.into_iter().enumerate() {
        let ts = match &item {
            Item::Msg(m) => m.ts,
            Item::Pending(p) => p.ts,
        };
        let day = local(ts).map(|t| t.date_naive());
        if day != last_day {
            last_day = day;
            if let Some(t) = local(ts) {
                let label = format!(" {} ", t.format("%a %b %-d"));
                let side = width.saturating_sub(label.width()) / 2;
                let rule = "─".repeat(side.min(20));
                lines.push(Line::from(format!("{rule}{label}{rule}").dark_gray()).centered());
            }
        }
        let start = lines.len();
        match item {
            Item::Msg(m) if m.from_me => outgoing(
                &mut lines,
                width,
                &body(m),
                status_of_msg(m),
                m.edited,
                m.service == "SMS",
            ),
            Item::Msg(m) => {
                let who = match (&m.sender, is_group) {
                    (Some(s), true) => Some(app.label(s)),
                    _ => None,
                };
                incoming(&mut lines, width, m, who)
            }
            Item::Pending(p) => {
                let (word, style) = match &p.state {
                    PendState::Sending => ("sending…", Style::new().dark_gray()),
                    PendState::Failed(_) => ("✗ failed", Style::new().red()),
                    PendState::Unconfirmed => ("? unconfirmed", Style::new().yellow()),
                };
                let status = vec![Span::styled(word, style)];
                outgoing(
                    &mut lines,
                    width,
                    &Body::Text(p.text.clone(), vec![]),
                    status,
                    false,
                    false,
                );
            }
        }
        if Some(i) == last_mine
            && let Item::Msg(m) = &item
            && let Some(text) = receipt(m)
        {
            let style = if m.is_error {
                Style::new().red()
            } else {
                Style::new().dark_gray()
            };
            lines.push(Line::from(Span::styled(text, style)).right_aligned());
        }
        ranges.push((start, lines.len()));
    }
    (lines, ranges)
}

enum Body {
    Text(String, Vec<String>),
    Dim(String),
}

fn body(m: &Message) -> Body {
    if m.unsent {
        return Body::Dim(
            if m.from_me {
                "⊘ you unsent a message"
            } else {
                "⊘ unsent a message"
            }
            .into(),
        );
    }
    if let Some(t) = &m.tapback {
        let who = if m.from_me { "you " } else { "" };
        return Body::Dim(format!("{who}{} to “{}”", t.emoji, t.target_text));
    }
    let atts = m.attachments.iter().map(attachment_label).collect();
    let text = if m.kind == Kind::Attachment {
        String::new()
    } else {
        m.text.clone()
    };
    Body::Text(text, atts)
}

/// "Read 10:42" beats "Delivered"; nothing for failed or unreceipted messages.
fn receipt(m: &Message) -> Option<String> {
    if m.is_error {
        return Some("Not Delivered".into());
    }
    match (m.read, m.delivered) {
        (Some(t), _) => Some(format!("Read {}", hhmm(t))),
        (None, Some(_)) if m.service == "SMS" => Some("Sent as Text Message".into()),
        (None, Some(_)) => Some("Delivered".into()),
        (None, None) if m.service == "SMS" => Some("Sent as Text Message".into()),
        (None, None) => None,
    }
}

/// Compact per-message marker after the time: ✗ not delivered, SMS text
/// message, ✓ delivered, ✓✓ read; nothing while only sent.
fn marker(m: &Message) -> Option<(&'static str, Style)> {
    if m.is_error {
        Some(("✗", Style::new().red()))
    } else if m.service == "SMS" {
        Some(("SMS", Style::new().green()))
    } else if m.read.is_some() {
        Some(("✓✓", Style::new().fg(Color::LightBlue)))
    } else if m.delivered.is_some() {
        Some(("✓", Style::new().dark_gray()))
    } else {
        None
    }
}

fn status_of_msg(m: &Message) -> Vec<Span<'static>> {
    let mut spans = vec![Span::styled(hhmm(m.ts), Style::new().dark_gray())];
    if let Some((mark, style)) = marker(m) {
        spans.push(Span::raw(" "));
        spans.push(Span::styled(mark, style));
    }
    spans
}

fn outgoing(
    lines: &mut Vec<Line<'static>>,
    width: usize,
    body: &Body,
    status: Vec<Span<'static>>,
    edited: bool,
    sms: bool,
) {
    let status_text_w: usize = status.iter().map(|s| s.content.width()).sum();
    let status_w = status_text_w.max(STATUS_COL) + 2;
    let max_w = (width * 3 / 4).max(20).min(width);
    let edited_w = if edited { " (edited)".width() } else { 0 };
    let text_w = max_w.saturating_sub(status_w + edited_w).max(1);
    // Blue for iMessage, green for a text message, like Messages.
    let own = Style::new().fg(if sms { Color::Green } else { Color::LightBlue });

    let mut rows: Vec<Vec<Span<'static>>> = Vec::new();
    match body {
        Body::Dim(s) => rows.push(vec![Span::styled(
            s.clone(),
            Style::new().dark_gray().italic(),
        )]),
        Body::Text(text, atts) => {
            if !text.is_empty() {
                for l in textwrap::wrap(text, text_w) {
                    rows.push(vec![Span::styled(l.into_owned(), own)]);
                }
            }
            for a in atts {
                rows.push(vec![Span::styled(truncate(a, text_w), Style::new().cyan())]);
            }
        }
    }
    if rows.is_empty() {
        rows.push(vec![Span::raw("")]);
    }
    let last = rows.len() - 1;
    for (i, mut spans) in rows.into_iter().enumerate() {
        if i == last {
            if edited {
                spans.push(Span::styled(" (edited)", Style::new().dark_gray()));
            }
            spans.push(Span::raw("  "));
            spans.extend(status.iter().cloned());
            spans.push(Span::raw(
                " ".repeat(STATUS_COL.saturating_sub(status_text_w)),
            ));
        } else {
            spans.push(Span::raw(" ".repeat(status_w)));
        }
        let used: usize = spans.iter().map(|s| s.content.width()).sum();
        spans.insert(0, Span::raw(" ".repeat(width.saturating_sub(used))));
        lines.push(Line::from(spans));
    }
}

/// `who` is the sender label to prefix in group chats.
fn incoming(lines: &mut Vec<Line<'static>>, width: usize, m: &Message, who: Option<String>) {
    let time = Span::styled(
        format!("{:<TIME_COL$}", hhmm(m.ts)),
        Style::new().dark_gray(),
    );
    let sender = who.map(|s| format!("[{s}] "));
    let sender_w = sender.as_ref().map_or(0, |s| s.width());
    let text_w = width.saturating_sub(TIME_COL).max(1);
    let indent = " ".repeat(TIME_COL);

    let mut rows: Vec<Vec<Span<'static>>> = Vec::new();
    match body(m) {
        Body::Dim(s) => rows.push(vec![Span::styled(s, Style::new().dark_gray().italic())]),
        Body::Text(text, atts) => {
            if !text.is_empty() {
                let first_indent = " ".repeat(sender_w);
                let opts = textwrap::Options::new(text_w).initial_indent(&first_indent);
                for (i, l) in textwrap::wrap(&text, opts).into_iter().enumerate() {
                    let l = if i == 0 {
                        l[first_indent.len().min(l.len())..].to_string()
                    } else {
                        l.into_owned()
                    };
                    rows.push(vec![Span::raw(l)]);
                }
            }
            for a in atts {
                rows.push(vec![Span::styled(
                    truncate(&a, text_w),
                    Style::new().cyan(),
                )]);
            }
        }
    }
    if rows.is_empty() {
        rows.push(vec![Span::raw("")]);
    }
    let last = rows.len() - 1;
    for (i, mut spans) in rows.into_iter().enumerate() {
        if i == 0 {
            if let Some(s) = &sender {
                spans.insert(
                    0,
                    Span::styled(
                        s.clone(),
                        Style::new().fg(handle_color(m.sender.as_deref().unwrap_or(""))),
                    ),
                );
            }
            spans.insert(0, time.clone());
        } else {
            spans.insert(0, Span::raw(indent.clone()));
        }
        if i == last && m.edited {
            spans.push(Span::styled(" (edited)", Style::new().dark_gray()));
        }
        lines.push(Line::from(spans));
    }
}

fn handle_color(h: &str) -> Color {
    const PALETTE: [Color; 6] = [
        Color::Magenta,
        Color::Green,
        Color::Yellow,
        Color::LightRed,
        Color::LightGreen,
        Color::LightMagenta,
    ];
    let hash = h
        .bytes()
        .fold(0usize, |a, b| a.wrapping_mul(31).wrapping_add(b as usize));
    PALETTE[hash % PALETTE.len()]
}

// ------------------------------------------------------------------ input, status, popup

fn draw_input(f: &mut Frame, app: &mut App, area: Rect) {
    app.rects.input = area;
    let picker_open = app.emoji.as_ref().is_some_and(|p| p.inline.is_none());
    let focused = app.focus == Focus::Input && app.popup.is_none() && !picker_open;
    let title = match app.view.as_ref().map(|v| &v.target) {
        Some(Target::Chat(_)) => format!(
            " Message → {} ",
            app.chat_for_view().map_or("…", |c| c.display.as_str())
        ),
        Some(Target::Draft(h)) => format!(" Message → {h} (new) "),
        None => " Message ".into(),
    };
    let block = Block::bordered()
        .title(truncate(&title, area.width.saturating_sub(4) as usize))
        .border_style(border(focused));
    let inner = block.inner(area);
    let (rows, (crow, ccol)) = input_layout(&app.input, app.cursor, inner.width.max(1) as usize);
    let first = (crow + 1).saturating_sub(inner.height as usize);
    let text: Vec<Line> = rows
        .into_iter()
        .skip(first)
        .take(inner.height as usize)
        .map(Line::from)
        .collect();
    f.render_widget(Paragraph::new(text).block(block), area);
    if focused {
        f.set_cursor_position((inner.x + ccol as u16, inner.y + (crow - first) as u16));
    }
}

/// Wraps the input into visual rows; returns rows and the cursor's (row, col).
pub fn input_layout(text: &str, cursor: usize, width: usize) -> (Vec<String>, (usize, usize)) {
    let width = width.max(1);
    let mut rows = vec![String::new()];
    let mut col = 0;
    let mut cur = None;
    for (i, ch) in text.char_indices() {
        let w = ch.width().unwrap_or(0);
        if ch != '\n' && col + w > width {
            rows.push(String::new());
            col = 0;
        }
        if i == cursor {
            cur = Some((rows.len() - 1, col));
        }
        if ch == '\n' {
            rows.push(String::new());
            col = 0;
            continue;
        }
        rows.last_mut().unwrap().push(ch);
        col += w;
    }
    let mut cur = cur.unwrap_or((rows.len() - 1, col));
    if cur.1 >= width {
        rows.push(String::new());
        cur = (rows.len() - 1, 0);
    }
    (rows, cur)
}

fn draw_status(f: &mut Frame, app: &App, area: Rect) {
    let now = std::time::Instant::now();
    let mut left = match &app.status {
        Status::Connected => vec![Span::styled(
            format!(" ● connected: {} ", app.host_label),
            Style::new().light_blue(),
        )],
        Status::Connecting => vec![Span::styled(
            format!(" ◌ connecting to {}… ", app.host_label),
            Style::new().yellow(),
        )],
        Status::Disconnected { reason, retry_at } => {
            let secs = retry_at.saturating_duration_since(now).as_secs_f32().ceil() as u64;
            vec![Span::styled(
                format!(" ✖ disconnected ({reason}): reconnecting in {secs}s, r now "),
                Style::new().white().on_red(),
            )]
        }
        Status::Fatal(msg) => vec![Span::styled(
            format!(" ✖ {msg} "),
            Style::new().white().on_red(),
        )],
    };
    if let Some((msg, _, _)) = &app.flash {
        left.push(Span::styled(
            format!(" ✓ {msg} "),
            Style::new().black().on_light_blue(),
        ));
    }
    let emoji_hint = match app.emoji.as_ref().map(|p| p.inline.is_some()) {
        Some(true) => Some("Tab/↑↓ choose  Enter insert  Esc close"),
        Some(false) => Some("type to search  ↑↓ choose  Enter insert  Esc close"),
        None => None,
    };
    let hints = match (app.popup.is_some(), app.focus) {
        _ if emoji_hint.is_some() => emoji_hint.unwrap(),
        (true, _) => "Enter next/open  Tab switch  Esc cancel",
        (_, Focus::List) => "j/k move  Enter open  n new  Tab focus  q quit",
        (_, Focus::Messages) => {
            if app.selected_pending_visible() {
                "Enter resend  x discard  j/k select  y copy  Esc back"
            } else {
                "j/k select  y copy  Y +meta  v range  PgUp older  Esc back"
            }
        }
        (_, Focus::Input) => "Enter send  Alt-Enter newline  PgUp/PgDn scroll  Esc back",
    };
    let used: usize = left.iter().map(|s| s.content.width()).sum();
    let avail = (area.width as usize).saturating_sub(used + 1);
    if hints.width() <= avail {
        left.push(Span::raw(" ".repeat(avail - hints.width())));
        left.push(Span::styled(hints, Style::new().dark_gray()));
    }
    f.render_widget(Paragraph::new(Line::from(left)), area);
}

fn draw_popup(f: &mut Frame, app: &App) {
    let Some(p) = &app.popup else { return };
    let area = f.area();
    let w = 56.min(area.width);
    let h = 7.min(area.height);
    let rect = Rect {
        x: area.x + (area.width - w) / 2,
        y: area.y + (area.height - h) / 2,
        width: w,
        height: h,
    };
    f.render_widget(Clear, rect);
    let block = Block::bordered()
        .title(" New conversation ")
        .border_style(Style::new().fg(Color::Cyan));
    let inner = block.inner(rect);
    let field_w = (inner.width as usize).saturating_sub(10);
    let tail = |s: &str| {
        let w = s.width();
        if w <= field_w {
            s.to_string()
        } else {
            s.chars()
                .rev()
                .scan(0, |acc, c| {
                    *acc += c.width().unwrap_or(0);
                    (*acc < field_w).then_some(c)
                })
                .collect::<Vec<_>>()
                .into_iter()
                .rev()
                .collect()
        }
    };
    let label = |i: usize, s: &'static str| {
        if p.field == i {
            s.cyan().bold()
        } else {
            s.into()
        }
    };
    let to = tail(&p.to);
    let msg = tail(&p.msg.replace('\n', "⏎"));
    let lines = vec![
        Line::from(vec![label(0, "To:      "), Span::raw(to.clone())]),
        Line::from(vec![label(1, "Message: "), Span::raw(msg.clone())]),
        Line::from(""),
        Line::from("phone number or email; Enter: next / open".dark_gray()),
    ];
    f.render_widget(Paragraph::new(lines).block(block), rect);
    let (row, col) = if p.field == 0 {
        (0, to.width())
    } else {
        (1, msg.width())
    };
    f.set_cursor_position((inner.x + 9 + col as u16, inner.y + row));
}

// ------------------------------------------------------------------ emoji

fn emoji_rows(app: &App, with_names: bool) -> Vec<Line<'static>> {
    let sel = app.emoji.as_ref().map_or(0, |p| p.sel);
    let query = app.emoji.as_ref().map_or("", |p| p.query.as_str());
    app.emoji_hits()
        .into_iter()
        .enumerate()
        .map(|(i, e)| {
            let label = if with_names {
                e.name().to_string()
            } else {
                crate::emoji::label(e, query)
            };
            let line = Line::from(vec![
                Span::raw(if i == sel { " ▶ " } else { "   " }),
                Span::raw(e.as_str().to_string()),
                Span::raw("  "),
                Span::styled(label, Style::new().dark_gray()),
            ]);
            if i == sel {
                line.patch_style(Style::new().bg(Color::DarkGray))
            } else {
                line
            }
        })
        .collect()
}

/// Small list just above the input, under the text being completed.
fn draw_emoji_inline(f: &mut Frame, app: &App, input_area: Rect) {
    let rows = emoji_rows(app, false);
    if rows.is_empty() {
        return;
    }
    let w = 34.min(f.area().width);
    let h = (rows.len() as u16 + 2).min(input_area.y);
    let inner_w = input_area.width.saturating_sub(2).max(1) as usize;
    let start = app.emoji.as_ref().and_then(|p| p.inline).unwrap_or(0);
    let (_, (_, col)) = input_layout(&app.input, start, inner_w);
    let x = (input_area.x + 1 + col as u16).min(f.area().width.saturating_sub(w));
    let rect = Rect {
        x,
        y: input_area.y.saturating_sub(h),
        width: w,
        height: h,
    };
    f.render_widget(Clear, rect);
    let block = Block::bordered().border_style(Style::new().fg(Color::Cyan));
    f.render_widget(Paragraph::new(rows).block(block), rect);
}

fn draw_emoji_picker(f: &mut Frame, app: &App) {
    let Some(p) = &app.emoji else { return };
    let area = f.area();
    let recent = p.query.is_empty() && !app.recents.is_empty();
    let mut lines = vec![
        Line::from(vec!["Search: ".cyan().bold(), Span::raw(p.query.clone())]),
        Line::from(""),
    ];
    if recent {
        lines.push(Line::from("Recent".dark_gray()));
    }
    let rows = emoji_rows(app, true);
    if rows.is_empty() {
        lines.push(Line::from("  no matches".dark_gray()));
    }
    lines.extend(rows);
    let w = 48.min(area.width);
    let h = (lines.len() as u16 + 2).min(area.height);
    let rect = Rect {
        x: area.x + (area.width - w) / 2,
        y: area.y + (area.height - h) / 2,
        width: w,
        height: h,
    };
    f.render_widget(Clear, rect);
    let block = Block::bordered()
        .title(" Emoji ")
        .border_style(Style::new().fg(Color::Cyan));
    let inner = block.inner(rect);
    f.render_widget(Paragraph::new(lines).block(block), rect);
    f.set_cursor_position((inner.x + 8 + p.query.width() as u16, inner.y));
}

// ------------------------------------------------------------------ helpers

fn local(ts: i64) -> Option<DateTime<Local>> {
    Local.timestamp_opt(ts, 0).single()
}

fn hhmm(ts: i64) -> String {
    local(ts)
        .map(|t| t.format("%H:%M").to_string())
        .unwrap_or_default()
}

/// Today: 10:42; within a week: Mon; older: Oct 7.
fn list_time(ts: i64, now: &DateTime<Local>) -> String {
    let Some(t) = local(ts) else {
        return String::new();
    };
    let days = (now.date_naive() - t.date_naive()).num_days();
    if days == 0 {
        t.format("%H:%M").to_string()
    } else if (1..7).contains(&days) {
        t.format("%a").to_string()
    } else if t.year() == now.year() {
        t.format("%b %-d").to_string()
    } else {
        t.format("%b %-d %Y").to_string()
    }
}

fn truncate(s: &str, max: usize) -> String {
    let s = s.replace('\n', " ");
    if s.width() <= max {
        return s;
    }
    let mut out = String::new();
    let mut w = 0;
    for c in s.chars() {
        let cw = c.width().unwrap_or(0);
        if w + cw + 1 > max {
            break;
        }
        out.push(c);
        w += cw;
    }
    out.push('…');
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn input_wraps_by_display_width() {
        let (rows, cur) = input_layout("ab😀cd", 9, 4);
        assert_eq!(rows, vec!["ab😀", "cd"]);
        assert_eq!(cur, (1, 2));
        let (rows, cur) = input_layout("hi\nthere", 3, 10);
        assert_eq!(rows, vec!["hi", "there"]);
        assert_eq!(cur, (1, 0));
        let (_, cur) = input_layout("abcd", 4, 4);
        assert_eq!(cur, (1, 0), "cursor after a full row moves down");
    }

    #[test]
    fn receipt_text() {
        let mut m = Message {
            rowid: 1,
            chat_id: 1,
            sender: None,
            from_me: true,
            text: "x".into(),
            ts: 0,
            service: "iMessage".into(),
            is_error: false,
            edited: false,
            unsent: false,
            delivered: None,
            read: None,
            kind: Kind::Text,
            attachments: vec![],
            tapback: None,
        };
        assert_eq!(receipt(&m), None);
        m.delivered = Some(1_791_369_185);
        assert_eq!(receipt(&m).as_deref(), Some("Delivered"));
        m.read = Some(1_791_369_240);
        assert!(receipt(&m).unwrap().starts_with("Read "));
        m.is_error = true;
        assert_eq!(receipt(&m).as_deref(), Some("Not Delivered"));
        m.is_error = false;
        m.read = None;
        m.service = "SMS".into();
        assert_eq!(receipt(&m).as_deref(), Some("Sent as Text Message"));
        m.delivered = None;
        assert_eq!(receipt(&m).as_deref(), Some("Sent as Text Message"));
    }

    #[test]
    fn markers_follow_delivery_state() {
        let mut m = Message {
            rowid: 1,
            chat_id: 1,
            sender: None,
            from_me: true,
            text: "x".into(),
            ts: 0,
            service: "iMessage".into(),
            is_error: false,
            edited: false,
            unsent: false,
            delivered: None,
            read: None,
            kind: Kind::Text,
            attachments: vec![],
            tapback: None,
        };
        assert_eq!(marker(&m), None, "sent, no receipt yet");
        m.delivered = Some(1);
        assert_eq!(marker(&m).map(|(s, _)| s), Some("✓"));
        m.read = Some(2);
        assert_eq!(marker(&m).map(|(s, _)| s), Some("✓✓"));
        m.service = "SMS".into();
        assert_eq!(marker(&m).map(|(s, _)| s), Some("SMS"));
        m.is_error = true;
        assert_eq!(marker(&m).map(|(s, _)| s), Some("✗"));
        let line_text: String = status_of_msg(&m)
            .iter()
            .map(|s| s.content.to_string())
            .collect();
        assert!(line_text.ends_with(" ✗"));
    }

    #[test]
    fn truncates_wide_chars() {
        assert_eq!(truncate("héllo wörld", 6), "héllo…");
        assert_eq!(truncate("😀😀😀", 5), "😀😀…");
        assert_eq!(truncate("short", 10), "short");
    }

    #[test]
    fn text_ends_at_the_same_column_with_or_without_marker() {
        let render = |status: Vec<Span<'static>>| {
            let mut lines = Vec::new();
            outgoing(
                &mut lines,
                60,
                &Body::Text("same text".into(), vec![]),
                status,
                false,
                false,
            );
            let s: String = lines[0]
                .spans
                .iter()
                .map(|sp| sp.content.to_string())
                .collect();
            s.find("same text").unwrap()
        };
        let plain = render(vec![Span::raw("10:33")]);
        let sms = render(vec![Span::raw("10:33"), Span::raw(" "), Span::raw("SMS")]);
        let read = render(vec![Span::raw("10:33"), Span::raw(" "), Span::raw("✓✓")]);
        assert_eq!(plain, sms);
        assert_eq!(plain, read);
    }

    #[test]
    fn emoji_wrapping_keeps_width() {
        let mut lines = Vec::new();
        outgoing(
            &mut lines,
            40,
            &Body::Text("🙌 ".repeat(20), vec![]),
            vec![Span::raw("10:00")],
            true,
            false,
        );
        for l in &lines {
            assert!(l.width() <= 40, "{l:?} is {} wide", l.width());
        }
        assert!(lines.len() > 1);
    }
}
