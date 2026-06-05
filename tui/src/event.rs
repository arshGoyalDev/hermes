use crossterm::event::{Event as CrosstermEvent, EventStream, KeyCode, KeyEvent, KeyModifiers};
use store::Transaction;
use tokio::sync::mpsc;
use tokio_stream::StreamExt;

pub enum TuiEvent {
  NewTransaction(Transaction),
  Key(KeyEvent),
  Resize,
  Tick,
}

pub fn spawn_event_task(
  proxy_rx: mpsc::UnboundedReceiver<Transaction>,
  tick_ms: u64,
) -> mpsc::UnboundedReceiver<TuiEvent> {
  let (tx, rx) = mpsc::unbounded_channel::<TuiEvent>();

  let tx_term = tx.clone();
  tokio::spawn(async move {
    let mut reader = EventStream::new();
    while let Some(maybe_event) = reader.next().await {
      match maybe_event {
        Ok(CrosstermEvent::Key(key)) => {
          let _ = tx_term.send(TuiEvent::Key(key));
        }
        Ok(CrosstermEvent::Resize(_, _)) => {
          let _ = tx_term.send(TuiEvent::Resize);
        }
        _ => {}
      }
    }
  });

  let tx_proxy = tx.clone();
  tokio::spawn(async move {
    let mut proxy_rx = proxy_rx;
    while let Some(transaction) = proxy_rx.recv().await {
      if tx_proxy
        .send(TuiEvent::NewTransaction(transaction))
        .is_err()
      {
        break;
      }
    }
  });

  tokio::spawn(async move {
    let mut interval = tokio::time::interval(std::time::Duration::from_millis(tick_ms));
    loop {
      interval.tick().await;
      if tx.send(TuiEvent::Tick).is_err() {
        break;
      }
    }
  });

  rx
}

pub fn key_action(key: &KeyEvent) -> Option<Action> {
  match key.code {
    KeyCode::Char('q') => Some(Action::Quit),
    KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => Some(Action::Quit),
    KeyCode::Up | KeyCode::Char('k') => Some(Action::Up),
    KeyCode::Down | KeyCode::Char('j') => Some(Action::Down),
    KeyCode::Tab => Some(Action::ToggleFocus),
    KeyCode::PageUp => Some(Action::PageUp),
    KeyCode::PageDown => Some(Action::PageDown),
    _ => None,
  }
}

#[derive(Debug, Clone, Copy)]
pub enum Action {
  Quit,
  Up,
  Down,
  ToggleFocus,
  PageUp,
  PageDown,
}
