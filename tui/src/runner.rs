use std::io;

use Direction::{Down, Up};
use crossterm::{
  execute,
  terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use ratatui::{Terminal, backend::CrosstermBackend};
use store::Transaction;
use tokio::sync::mpsc;

use crate::app::{App, Direction};
use crate::event::{Action, TuiEvent, key_action, spawn_event_task};
use crate::render::draw;

pub async fn run_tui(proxy_rx: mpsc::UnboundedReceiver<Transaction>) -> io::Result<()> {
  enable_raw_mode()?;
  let mut stdout = io::stdout();
  execute!(stdout, EnterAlternateScreen)?;
  let backend = CrosstermBackend::new(stdout);
  let mut terminal = Terminal::new(backend)?;
  terminal.clear()?;

  let result = event_loop(&mut terminal, proxy_rx).await;

  disable_raw_mode()?;
  execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
  terminal.show_cursor()?;

  result
}

async fn event_loop(
  terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
  proxy_rx: mpsc::UnboundedReceiver<Transaction>,
) -> io::Result<()> {
  let mut app = App::new();
  let mut events = spawn_event_task(proxy_rx, 200);

  loop {
    terminal.draw(|f| draw(f, &app))?;

    let Some(event) = events.recv().await else {
      break;
    };

    match event {
      TuiEvent::NewTransaction(tx) => {
        app.push(tx);
      }
      TuiEvent::Key(key) => {
        if let Some(action) = key_action(&key) {
          handle_action(&mut app, action);
        }
      }
      TuiEvent::Resize => {}
      TuiEvent::Tick => {}
    }

    if app.should_quit {
      break;
    }
  }

  Ok(())
}

fn handle_action(app: &mut App, action: Action) {
  use crate::app::Focus;
  match action {
    Action::Quit => app.should_quit = true,
    Action::Up => match app.focus {
      Focus::List => app.move_selection(Up),
      Focus::Detail => app.scroll_detail(Up),
    },
    Action::Down => match app.focus {
      Focus::List => app.move_selection(Down),
      Focus::Detail => app.scroll_detail(Down),
    },
    Action::ToggleFocus => app.toggle_focus(),
    Action::PageUp => {
      for _ in 0..10 {
        match app.focus {
          Focus::List => app.move_selection(Up),
          Focus::Detail => app.scroll_detail(Up),
        }
      }
    }
    Action::PageDown => {
      for _ in 0..10 {
        match app.focus {
          Focus::List => app.move_selection(Down),
          Focus::Detail => app.scroll_detail(Down),
        }
      }
    }
  }
}
