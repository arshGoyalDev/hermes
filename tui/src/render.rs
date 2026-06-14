use ratatui::{
  Frame,
  layout::{Constraint, Direction, Layout, Rect},
  style::{Color, Modifier, Style},
  text::{Line, Span, Text},
  widgets::{
    Block, BorderType, Borders, List, ListItem, ListState, Padding, Paragraph,
    Scrollbar, ScrollbarOrientation, ScrollbarState,
  },
};
use store::Transaction;

use crate::app::{App, Focus};

// ── Palette ───────────────────────────────────────────────────────────────────
const C_BG: Color = Color::Rgb(10, 10, 10);
const C_PANEL: Color = Color::Rgb(10, 10, 10);
const C_BORDER: Color = Color::Rgb(40, 40, 40);
const C_BORDER_FOCUS: Color = Color::Rgb(200, 200, 200);
const C_TEXT: Color = Color::Rgb(220, 220, 220);
const C_DIM: Color = Color::Rgb(110, 110, 110);
const C_SELECTED_BG: Color = Color::Rgb(45, 45, 45);
const C_SELECTED_FG: Color = Color::Rgb(255, 255, 255);

const C_METHOD_GET: Color = Color::Rgb(143, 220, 151);
const C_METHOD_POST: Color = Color::Rgb(100, 180, 255);
const C_METHOD_PUT: Color = Color::Rgb(255, 200, 80);
const C_METHOD_DELETE: Color = Color::Rgb(255, 100, 100);
const C_METHOD_OTHER: Color = Color::Rgb(200, 120, 255);

const C_STATUS_2XX: Color = Color::Rgb(143, 220, 151);
const C_STATUS_3XX: Color = Color::Rgb(100, 180, 255);
const C_STATUS_4XX: Color = Color::Rgb(255, 200, 80);
const C_STATUS_5XX: Color = Color::Rgb(255, 100, 100);
const C_STATUS_PENDING: Color = Color::Rgb(130, 130, 130);

const C_HEADER_NAME: Color = Color::Rgb(150, 150, 150);
const C_KEY: Color = Color::Rgb(130, 130, 130);

/// Max chars for any single displayed line in the detail panel.
const MAX_LINE: usize = 200;
/// Max body lines to render (prevents giant HTML pages from hanging the TUI).
const MAX_BODY_LINES: usize = 300;

// ── Entry point ───────────────────────────────────────────────────────────────

pub fn draw(frame: &mut Frame, app: &App) {
  let area = frame.area();

  // Root background.
  frame.render_widget(
    Block::default().style(Style::default().bg(C_BG)),
    area,
  );

  let root = Layout::default()
    .direction(Direction::Vertical)
    .constraints([Constraint::Min(0), Constraint::Length(1)])
    .split(area);

  let panes = Layout::default()
    .direction(Direction::Horizontal)
    .constraints([Constraint::Percentage(35), Constraint::Percentage(65)])
    .split(root[0]);

  draw_list(frame, app, panes[0]);
  draw_detail(frame, app, panes[1]);
  draw_status_bar(frame, app, root[1]);
}

// ── Left panel — transaction list ─────────────────────────────────────────────

fn draw_list(frame: &mut Frame, app: &App, area: Rect) {
  let focused = app.focus == Focus::List;
  let border_style = Style::default().fg(if focused { C_BORDER_FOCUS } else { C_BORDER });

  let block = Block::default()
    .title(Span::styled(
      " Transactions ",
      Style::default()
        .fg(if focused { C_BORDER_FOCUS } else { C_DIM })
        .add_modifier(Modifier::BOLD),
    ))
    .borders(Borders::ALL)
    .border_type(BorderType::Plain)
    .border_style(border_style)
    .style(Style::default().bg(C_PANEL))
    .padding(Padding::horizontal(1));

  let items: Vec<ListItem> = app.transactions.iter().map(list_item).collect();

  let mut state = ListState::default();
  if !app.transactions.is_empty() {
    state.select(Some(app.selected));
  }

  let list = List::new(items)
    .block(block)
    .highlight_style(
      Style::default()
        .bg(C_SELECTED_BG)
        .fg(C_SELECTED_FG)
        .add_modifier(Modifier::BOLD),
    )
    .highlight_symbol("▶ ");

  frame.render_stateful_widget(list, area, &mut state);
}

fn list_item(tx: &Transaction) -> ListItem<'_> {
  let mc = method_color(&tx.request.method);

  let (status_str, sc) = match &tx.response {
    Some(r) => (format!("{}", r.status), status_color(r.status)),
    None => ("···".to_string(), C_STATUS_PENDING),
  };

  let dur = match tx.duration_ms {
    Some(ms) if ms < 1000 => format!(" {ms}ms"),
    Some(ms) => format!(" {:.1}s", ms as f64 / 1000.0),
    None => String::new(),
  };

  // Show host + truncated path.
  let host = tx.host();
  let path = tx.path();
  let avail = 28usize.saturating_sub(host.len());
  let path_d = if path.len() > avail {
    format!("{}…", &path[..avail.saturating_sub(1)])
  } else {
    path.to_string()
  };

  let line = Line::from(vec![
    Span::styled(format!("{:<5} ", &tx.request.method), Style::default().fg(mc).add_modifier(Modifier::BOLD)),
    Span::styled(format!("{:<3} ", status_str), Style::default().fg(sc)),
    Span::styled(format!("{}{}", host, path_d), Style::default().fg(C_TEXT)),
    Span::styled(dur, Style::default().fg(C_DIM)),
  ]);

  ListItem::new(line)
}

// ── Right panel — detail view ─────────────────────────────────────────────────

fn draw_detail(frame: &mut Frame, app: &App, area: Rect) {
  let focused = app.focus == Focus::Detail;
  let border_style = Style::default().fg(if focused { C_BORDER_FOCUS } else { C_BORDER });

  let block = Block::default()
    .title(Span::styled(
      " Detail ",
      Style::default()
        .fg(if focused { C_BORDER_FOCUS } else { C_DIM })
        .add_modifier(Modifier::BOLD),
    ))
    .borders(Borders::ALL)
    .border_type(BorderType::Plain)
    .border_style(border_style)
    .style(Style::default().bg(C_PANEL))
    .padding(Padding::new(2, 1, 1, 1));

  let inner = block.inner(area);
  frame.render_widget(block, area);

  match app.selected_transaction() {
    None => {
      frame.render_widget(
        Paragraph::new(Text::styled(
          "\nNo transactions captured yet.\nBrowse through the proxy to see traffic.",
          Style::default().fg(C_DIM),
        )),
        inner,
      );
    }
    Some(tx) => {
      // Build lines — NO Wrap widget; we split manually so bodies never bleed
      // into adjacent sections.
      let lines = build_detail_lines(tx, inner.width as usize);
      let total = lines.len() as u16;

      let para = Paragraph::new(lines)
        // NO .wrap() — we own every line break ourselves
        .scroll((app.detail_scroll, 0));
      frame.render_widget(para, inner);

      if total > inner.height {
        let mut ss = ScrollbarState::new(total as usize)
          .position(app.detail_scroll as usize);
        let scrollbar = Scrollbar::new(ScrollbarOrientation::VerticalRight)
          .begin_symbol(None)
          .end_symbol(None)
          .track_symbol(Some("│"))
          .thumb_symbol("▌");
        frame.render_stateful_widget(scrollbar, inner, &mut ss);
      }
    }
  }
}

/// Build the list of `Line`s for the detail panel.
///
/// `panel_width` is used to hard-wrap body lines so they never overflow into
/// the next section — this is the root fix for the "messy UI" problem.
fn build_detail_lines(tx: &Transaction, panel_width: usize) -> Vec<Line<'static>> {
  let mut out: Vec<Line<'static>> = Vec::new();

  // ── REQUEST ────────────────────────────────────────────────────────────────
  out.push(section_header("REQUEST"));
  out.push(kv(
    "Method",
    tx.request.method.clone(),
    method_color(&tx.request.method),
  ));
  // URL — may be very long; split across lines.
  for line in split_value(&tx.request.url, panel_width.saturating_sub(10)) {
    out.push(kv("URL", line, C_TEXT));
  }
  out.push(Line::raw(""));

  if !tx.request.headers.is_empty() {
    out.push(sub_header("Headers"));
    for (name, value) in &tx.request.headers {
      out.push(hdr_line(name.clone(), value.clone(), panel_width));
    }
    out.push(Line::raw(""));
  }

  if !tx.request.body.is_empty() {
    out.push(sub_header("Body"));
    out.extend(body_lines(&tx.request.body, panel_width));
    out.push(Line::raw(""));
  }

  // ── RESPONSE ───────────────────────────────────────────────────────────────
  out.push(section_header("RESPONSE"));
  match &tx.response {
    None => {
      out.push(Line::from(Span::styled(
        "pending…",
        Style::default()
          .fg(C_STATUS_PENDING)
          .add_modifier(Modifier::ITALIC),
      )));
    }
    Some(resp) => {
      out.push(kv("Status", resp.status.to_string(), status_color(resp.status)));
      if let Some(ms) = tx.duration_ms {
        out.push(kv("Duration", format!("{ms} ms"), C_DIM));
      }
      out.push(Line::raw(""));

      if !resp.headers.is_empty() {
        out.push(sub_header("Headers"));
        for (name, value) in &resp.headers {
          out.push(hdr_line(name.clone(), value.clone(), panel_width));
        }
        out.push(Line::raw(""));
      }

      if !resp.body.is_empty() {
        out.push(sub_header("Body"));
        out.extend(body_lines(&resp.body, panel_width));
      }
    }
  }

  out
}

// ── Status bar ────────────────────────────────────────────────────────────────

fn draw_status_bar(frame: &mut Frame, app: &App, area: Rect) {
  let count = app.transactions.len();
  let hint = match app.focus {
    Focus::List => "TAB→detail",
    Focus::Detail => "TAB→list",
  };
  let line = Line::from(vec![
    Span::styled(
      " HERMES ",
      Style::default()
        .fg(C_BG)
        .bg(C_BORDER_FOCUS)
        .add_modifier(Modifier::BOLD),
    ),
    Span::styled(
      format!("  {count} captured  ↑↓/jk  {hint}  q quit "),
      Style::default().fg(C_DIM).bg(C_PANEL),
    ),
  ]);
  frame.render_widget(
    Paragraph::new(line).style(Style::default().bg(C_PANEL)),
    area,
  );
}

// ── Helpers ───────────────────────────────────────────────────────────────────

fn method_color(m: &str) -> Color {
  match m.to_ascii_uppercase().as_str() {
    "GET" => C_METHOD_GET,
    "POST" => C_METHOD_POST,
    "PUT" | "PATCH" => C_METHOD_PUT,
    "DELETE" => C_METHOD_DELETE,
    _ => C_METHOD_OTHER,
  }
}

fn status_color(code: u16) -> Color {
  match code {
    200..=299 => C_STATUS_2XX,
    300..=399 => C_STATUS_3XX,
    400..=499 => C_STATUS_4XX,
    500..=599 => C_STATUS_5XX,
    _ => C_STATUS_PENDING,
  }
}

fn section_header(title: &'static str) -> Line<'static> {
  Line::from(Span::styled(
    title,
    Style::default()
      .fg(C_BORDER_FOCUS)
      .add_modifier(Modifier::BOLD),
  ))
}

fn sub_header(title: &'static str) -> Line<'static> {
  Line::from(Span::styled(title, Style::default().fg(C_DIM)))
}

/// A `key: value` line — key is fixed-width, value is sanitized.
fn kv(key: &str, value: String, vc: Color) -> Line<'static> {
  Line::from(vec![
    Span::styled(format!("{:<10}", key), Style::default().fg(C_KEY)),
    Span::styled(sanitize(&value), Style::default().fg(vc)),
  ])
}

/// A header `Name   Value` line.
///
/// Both the name and value are truncated to fixed budgets so that long header
/// names (e.g. `Access-Control-Expose-Headers` = 30 chars) don't push the
/// value off-screen.  Values are also sanitized to remove control chars.
fn hdr_line(name: String, value: String, panel_width: usize) -> Line<'static> {
  const NAME_W: usize = 26;
  // Truncate long header names so the value column is always reachable.
  let name = trunc(sanitize(&name), NAME_W);
  // Budget: panel minus the name column minus a 1-char separator.
  let val_budget = panel_width.saturating_sub(NAME_W + 1).max(10);
  let value = trunc(sanitize(&value), val_budget);

  Line::from(vec![
    Span::styled(format!("{:<NAME_W$}", name), Style::default().fg(C_HEADER_NAME)),
    Span::styled(value, Style::default().fg(C_TEXT)),
  ])
}

/// Render a body as multiple `Line`s safe for TUI display.
///
/// Binary bodies (detected by sniffing for null bytes or high non-printable
/// content) are **never** rendered as text — they contain ESC sequences that
/// corrupt the entire terminal display.  Text bodies are sanitized to remove
/// any embedded control characters before they reach the renderer.
fn body_lines(body: &[u8], width: usize) -> Vec<Line<'static>> {
  // ── Binary guard ──────────────────────────────────────────────────────────
  if is_binary(body) {
    let mime_hint = sniff_mime(body);
    return vec![Line::from(Span::styled(
      format!("({mime_hint} — {} bytes, not rendered)", body.len()),
      Style::default().fg(C_DIM).add_modifier(Modifier::ITALIC),
    ))];
  }

  let raw = String::from_utf8_lossy(body).into_owned();
  let line_width = (width.saturating_sub(2)).max(40).min(MAX_LINE);
  let mut out: Vec<Line<'static>> = Vec::new();
  let total_src_lines = raw.lines().count();

  for (i, src_line) in raw.lines().enumerate() {
    if i >= MAX_BODY_LINES {
      out.push(Line::from(Span::styled(
        format!(
          "… {} more lines — scroll or use `hermes replay` to view full body",
          total_src_lines - MAX_BODY_LINES
        ),
        Style::default().fg(C_DIM).add_modifier(Modifier::ITALIC),
      )));
      break;
    }

    // Sanitize first, THEN hard-wrap — order matters.
    let safe = sanitize(src_line);
    if safe.is_empty() {
      out.push(Line::raw(""));
      continue;
    }
    let chars: Vec<char> = safe.chars().collect();
    let mut pos = 0;
    while pos < chars.len() {
      let end = (pos + line_width).min(chars.len());
      let chunk: String = chars[pos..end].iter().collect();
      out.push(Line::from(Span::styled(chunk, Style::default().fg(C_TEXT))));
      pos = end;
    }
  }

  if out.is_empty() {
    out.push(Line::from(Span::styled(
      "(empty body)",
      Style::default().fg(C_DIM).add_modifier(Modifier::ITALIC),
    )));
  }
  out
}

/// Split a potentially very long string into chunks of at most `width` chars,
/// returned as owned Strings (needed for `'static` lifetimes in Lines).
fn split_value(value: &str, width: usize) -> Vec<String> {
  let w = width.max(20);
  let chars: Vec<char> = value.chars().collect();
  if chars.len() <= w {
    return vec![value.to_string()];
  }
  let mut out = Vec::new();
  let mut pos = 0;
  while pos < chars.len() {
    let end = (pos + w).min(chars.len());
    out.push(chars[pos..end].iter().collect());
    pos = end;
  }
  out
}

/// Truncate a string to at most `max` chars, appending `…`.
fn trunc(mut s: String, max: usize) -> String {
  if max < 4 {
    return s;
  }
  let count = s.chars().count();
  if count > max {
    let byte_end = s
      .char_indices()
      .nth(max - 1)
      .map(|(i, _)| i)
      .unwrap_or(s.len());
    s.truncate(byte_end);
    s.push('…');
  }
  s
}

// ── Safety helpers ─────────────────────────────────────────────────────────────

/// Replace control characters (including ESC `\x1b`) with safe substitutes.
///
/// **This must be called on every string before it reaches a Ratatui `Span`.**
/// A single `\x1b[2J` (clear-screen) embedded in a header value or response
/// body will wipe the entire terminal and corrupt the TUI completely.
fn sanitize(s: &str) -> String {
  s.chars()
    .map(|c| match c {
      '\t' => ' ',                                         // tab → space
      c if c < '\x20' => '·',                             // C0 controls (incl. ESC 0x1b)
      '\x7f' => '·',                                      // DEL
      c if (0x80u32..=0x9fu32).contains(&(c as u32)) => '·', // C1 controls
      c => c,
    })
    .collect()
}

/// Return `true` if `data` looks like binary content that should not be
/// rendered as text.
///
/// Heuristic: any null byte, OR more than 5% non-printable bytes in the first
/// 512 bytes of the body.
fn is_binary(data: &[u8]) -> bool {
  if data.is_empty() {
    return false;
  }
  // Null bytes are unambiguous.
  if data.contains(&0x00) {
    return true;
  }
  let sample = &data[..data.len().min(512)];
  let non_print = sample
    .iter()
    .filter(|&&b| b < 0x09 || (b > 0x0d && b < 0x20) || b == 0x7f)
    .count();
  // >5% non-printable → binary
  non_print * 20 > sample.len()
}

/// Quick MIME sniff so the binary-body placeholder can say "image/png — N bytes"
/// instead of just "binary — N bytes".
fn sniff_mime(data: &[u8]) -> &'static str {
  match data {
    [0x89, b'P', b'N', b'G', ..] => "image/png",
    [0xff, 0xd8, 0xff, ..] => "image/jpeg",
    [b'G', b'I', b'F', ..] => "image/gif",
    [b'R', b'I', b'F', b'F', ..] => "image/webp",
    [0x1f, 0x8b, ..] => "application/gzip",
    [b'P', b'K', 0x03, 0x04, ..] => "application/zip",
    [b'%', b'P', b'D', b'F', ..] => "application/pdf",
    _ => "binary",
  }
}
