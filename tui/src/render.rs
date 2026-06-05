use ratatui::{
  Frame,
  layout::{Constraint, Direction, Layout, Rect},
  style::{Color, Modifier, Style},
  text::{Line, Span, Text},
  widgets::{
    Block, BorderType, Borders, List, ListItem, ListState, Paragraph, Scrollbar,
    ScrollbarOrientation, ScrollbarState, Wrap, Padding,
  },
};
use store::Transaction;

use crate::app::{App, Focus};

const C_BG: Color = Color::Rgb(10, 10, 10);
const C_PANEL: Color = Color::Rgb(10, 10, 10);
const C_BORDER: Color = Color::Rgb(40, 40, 40);
const C_BORDER_FOCUS: Color = Color::Rgb(200, 200, 200);
const C_TEXT: Color = Color::Rgb(220, 220, 220);
const C_DIM: Color = Color::Rgb(120, 120, 120);
const C_SELECTED_BG: Color = Color::Rgb(50, 50, 50);
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
const C_STATUS_PENDING: Color = Color::Rgb(140, 140, 140);

const C_HEADER_NAME: Color = Color::Rgb(160, 160, 160);

pub fn draw(frame: &mut Frame, app: &App) {
  let area = frame.area();

  let bg = Block::default().style(Style::default().bg(C_BG));
  frame.render_widget(bg, area);

  let root_chunks = Layout::default()
    .direction(Direction::Vertical)
    .constraints([Constraint::Min(0), Constraint::Length(1)])
    .split(area);

  let content_area = root_chunks[0];
  let status_area = root_chunks[1];

  let panes = Layout::default()
    .direction(Direction::Horizontal)
    .constraints([Constraint::Percentage(35), Constraint::Percentage(65)])
    .split(content_area);

  draw_list(frame, app, panes[0]);
  draw_detail(frame, app, panes[1]);
  draw_status_bar(frame, app, status_area);
}

fn draw_list(frame: &mut Frame, app: &App, area: Rect) {
  let is_focused = app.focus == Focus::List;
  let border_color = if is_focused { C_BORDER_FOCUS } else { C_BORDER };

  let block = Block::default()
    .title(Span::styled(
      " Transactions ",
      Style::default()
        .fg(if is_focused { C_BORDER_FOCUS } else { C_DIM })
        .add_modifier(Modifier::BOLD),
    ))
    .borders(Borders::ALL)
    .border_type(BorderType::Plain)
    .border_style(Style::default().fg(border_color))
    .style(Style::default().bg(C_PANEL))
    .padding(Padding::new(1, 1, 0, 0));

  let items: Vec<ListItem> = app.transactions.iter().map(|tx| list_item(tx)).collect();

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
    .highlight_symbol("  "); 

  frame.render_stateful_widget(list, area, &mut state);
}

fn list_item(tx: &Transaction) -> ListItem<'_> {
  let method_color = method_color(&tx.request.method);
  let (status_str, status_color) = match &tx.response {
    Some(r) => (format!("{}", r.status), status_color(r.status)),
    None => ("···".to_string(), C_STATUS_PENDING),
  };

  let duration_str = match tx.duration_ms {
    Some(ms) => if ms < 1000 { format!("{ms}ms") } else { format!("{:.1}s", ms as f64 / 1000.0) },
    None => String::new(),
  };

  let path = tx.path();
  let max_path = 25usize;
  let path_display = if path.len() > max_path {
    format!("{}…", &path[..max_path.saturating_sub(1)])
  } else {
    path.to_string()
  };

  let line = Line::from(vec![
    Span::styled(format!("{:<6}", &tx.request.method), Style::default().fg(method_color).add_modifier(Modifier::BOLD)),
    Span::styled(format!("{:>3} ", status_str), Style::default().fg(status_color)),
    Span::styled(path_display, Style::default().fg(C_TEXT)),
    Span::styled(format!(" {}", duration_str), Style::default().fg(C_DIM)),
  ]);

  ListItem::new(line)
}

fn draw_detail(frame: &mut Frame, app: &App, area: Rect) {
  let is_focused = app.focus == Focus::Detail;
  let border_color = if is_focused { C_BORDER_FOCUS } else { C_BORDER };

  let block = Block::default()
    .title(Span::styled(
      " Detail ",
      Style::default()
        .fg(if is_focused { C_BORDER_FOCUS } else { C_DIM })
        .add_modifier(Modifier::BOLD),
    ))
    .borders(Borders::ALL)
    .border_type(BorderType::Plain)
    .border_style(Style::default().fg(border_color))
    .style(Style::default().bg(C_PANEL))
    .padding(Padding::new(2, 2, 1, 1));

  let inner = block.inner(area);
  frame.render_widget(block, area);

  match app.selected_transaction() {
    None => {
      let placeholder = Paragraph::new(Text::styled(
        "\nNo transactions captured yet.",
        Style::default().fg(C_DIM),
      ));
      frame.render_widget(placeholder, inner);
    }
    Some(tx) => {
      let lines = build_detail_lines(tx);
      let total_lines = lines.len() as u16;

      let para = Paragraph::new(lines)
        .wrap(Wrap { trim: false })
        .scroll((app.detail_scroll, 0));
      frame.render_widget(para, inner);

      if total_lines > inner.height {
        let mut scrollbar_state = ScrollbarState::new(total_lines as usize).position(app.detail_scroll as usize);
        let scrollbar = Scrollbar::new(ScrollbarOrientation::VerticalRight)
          .begin_symbol(None)
          .end_symbol(None)
          .track_symbol(Some("│"))
          .thumb_symbol("▌");
        frame.render_stateful_widget(scrollbar, inner, &mut scrollbar_state);
      }
    }
  }
}

fn build_detail_lines(tx: &Transaction) -> Vec<Line<'static>> {
  let mut lines: Vec<Line<'static>> = Vec::new();

  lines.push(section_header("REQUEST"));
  lines.push(kv_line("Method", tx.request.method.clone(), method_color(&tx.request.method)));
  lines.push(kv_line("URL", tx.request.url.clone(), C_TEXT));
  lines.push(Line::raw(""));

  if !tx.request.headers.is_empty() {
    lines.push(sub_header("Headers"));
    for (name, value) in &tx.request.headers {
      lines.push(header_line(name.clone(), value.clone()));
    }
    lines.push(Line::raw(""));
  }

  if !tx.request.body.is_empty() {
    lines.push(sub_header("Body"));
    lines.push(body_line(&tx.request.body));
    lines.push(Line::raw(""));
  }

  lines.push(section_header("RESPONSE"));
  match &tx.response {
    None => {
      lines.push(Line::from(Span::styled("pending...", Style::default().fg(C_STATUS_PENDING).add_modifier(Modifier::ITALIC))));
    }
    Some(resp) => {
      lines.push(kv_line("Status", resp.status.to_string(), status_color(resp.status)));
      if let Some(ms) = tx.duration_ms {
        lines.push(kv_line("Duration", format!("{ms} ms"), C_DIM));
      }
      lines.push(Line::raw(""));

      if !resp.headers.is_empty() {
        lines.push(sub_header("Headers"));
        for (name, value) in &resp.headers {
          lines.push(header_line(name.clone(), value.clone()));
        }
        lines.push(Line::raw(""));
      }

      if !resp.body.is_empty() {
        lines.push(sub_header("Body"));
        lines.push(body_line(&resp.body));
      }
    }
  }

  lines
}

fn draw_status_bar(frame: &mut Frame, app: &App, area: Rect) {
  let count = app.transactions.len();
  let focus_hint = match app.focus {
    Focus::List => "TAB → Detail",
    Focus::Detail => "TAB → List",
  };
  
  let text = Line::from(vec![
    Span::styled(" HERMES ", Style::default().fg(C_BG).bg(C_BORDER_FOCUS).add_modifier(Modifier::BOLD)),
    Span::styled(format!("  {count} captured  "), Style::default().fg(C_DIM).bg(C_PANEL)),
    Span::styled(format!("  ↑↓/jk navigate  {focus_hint}  q quit "), Style::default().fg(C_DIM).bg(C_PANEL)),
  ]);

  let bar = Paragraph::new(text).style(Style::default().bg(C_PANEL));
  frame.render_widget(bar, area);
}

fn method_color(method: &str) -> Color {
  match method.to_ascii_uppercase().as_str() {
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
  Line::from(vec![Span::styled(
    title,
    Style::default().fg(C_BORDER_FOCUS).add_modifier(Modifier::BOLD),
  )])
}

fn sub_header(title: &'static str) -> Line<'static> {
  Line::from(Span::styled(
    title,
    Style::default().fg(C_DIM),
  ))
}

fn kv_line(key: &str, value: String, value_color: Color) -> Line<'static> {
  Line::from(vec![
    Span::styled(format!("{:<8} ", key), Style::default().fg(C_DIM)),
    Span::styled(value, Style::default().fg(value_color)),
  ])
}

fn header_line(name: String, value: String) -> Line<'static> {
  Line::from(vec![
    Span::styled(format!("{:<20} ", name), Style::default().fg(C_HEADER_NAME)),
    Span::styled(value, Style::default().fg(C_TEXT)),
  ])
}

fn body_line(body: &[u8]) -> Line<'static> {
  let text = String::from_utf8_lossy(body).into_owned();
  Line::from(Span::styled(text, Style::default().fg(C_TEXT)))
}
