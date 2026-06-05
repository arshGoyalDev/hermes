use store::Transaction;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Direction {
  Up,
  Down,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Focus {
  List,
  Detail,
}

pub struct App {
  pub transactions: Vec<Transaction>,
  pub selected: usize,
  pub detail_scroll: u16,
  pub focus: Focus,
  pub should_quit: bool,
}

impl App {
  pub fn new() -> Self {
    Self {
      transactions: Vec::new(),
      selected: 0,
      detail_scroll: 0,
      focus: Focus::List,
      should_quit: false,
    }
  }

  pub fn push(&mut self, tx: Transaction) {
    let was_at_end = self.selected + 1 >= self.transactions.len().max(1);
    self.transactions.push(tx);
    if was_at_end && self.transactions.len() > 1 {
      self.selected = self.transactions.len() - 1;
      self.detail_scroll = 0;
    }
  }

  pub fn move_selection(&mut self, dir: Direction) {
    let len = self.transactions.len();
    if len == 0 {
      return;
    }
    match dir {
      Direction::Up => {
        if self.selected > 0 {
          self.selected -= 1;
          self.detail_scroll = 0;
        }
      }
      Direction::Down => {
        if self.selected + 1 < len {
          self.selected += 1;
          self.detail_scroll = 0;
        }
      }
    }
  }

  pub fn scroll_detail(&mut self, dir: Direction) {
    match dir {
      Direction::Up => {
        self.detail_scroll = self.detail_scroll.saturating_sub(1);
      }
      Direction::Down => {
        self.detail_scroll = self.detail_scroll.saturating_add(1);
      }
    }
  }

  pub fn toggle_focus(&mut self) {
    self.focus = match self.focus {
      Focus::List => Focus::Detail,
      Focus::Detail => Focus::List,
    };
    self.detail_scroll = 0;
  }

  pub fn selected_transaction(&self) -> Option<&Transaction> {
    self.transactions.get(self.selected)
  }
}

impl Default for App {
  fn default() -> Self {
    Self::new()
  }
}
