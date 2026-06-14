mod replay;

use clap::{Parser, Subcommand};
use proxy::ProxyConfig;
use scripts::ScriptEngine;
use store::{Transaction, TransactionStore};
use tokio::sync::mpsc;

/// Hermes — a programmable HTTP traffic inspector & replay tool.
#[derive(Parser, Debug)]
#[command(name = "hermes", version, about, long_about = None)]
struct Cli {
  #[command(subcommand)]
  command: Commands,
}

#[derive(Subcommand, Debug)]
enum Commands {
  /// Start the MITM proxy and the live traffic TUI (default).
  Run {
    /// Address to listen on.
    #[arg(long, default_value = "127.0.0.1:8080")]
    bind: String,

    /// Directory to persist captured sessions.
    #[arg(long, default_value = ".hermes-sessions")]
    db: String,

    /// Directory of `.rhai` transform scripts to load.
    #[arg(long, default_value = ".hermes-scripts")]
    scripts: String,
  },

  /// Replay a previously captured transaction by its UUID.
  Replay {
    /// Transaction UUID (shown in the TUI or printed by `hermes list`).
    id: String,

    /// Session database directory.
    #[arg(long, default_value = ".hermes-sessions")]
    db: String,

    /// Print the diff between the original and replayed response.
    #[arg(long, default_value_t = true)]
    diff: bool,
  },

  /// List all captured transactions stored in the database.
  List {
    /// Session database directory.
    #[arg(long, default_value = ".hermes-sessions")]
    db: String,
  },
}

#[tokio::main]
async fn main() -> std::io::Result<()> {
  let cli = Cli::parse();

  match cli.command {
    Commands::Run { bind, db, scripts } => cmd_run(bind, db, scripts).await,
    Commands::Replay { id, db, diff } => cmd_replay(id, db, diff).await,
    Commands::List { db } => cmd_list(db),
  }
}

// ── `hermes run` ──────────────────────────────────────────────────────────────

async fn cmd_run(bind: String, db: String, scripts_dir: String) -> std::io::Result<()> {
  // ── Redirect stderr → log file before entering the TUI ────────────────────
  // Proxy errors and script warnings would otherwise bleed through the
  // alternate screen and corrupt the display.
  let log_path = format!("{}.log", db.trim_end_matches('/'));
  redirect_stderr(&log_path)?;
  // This eprintln now goes to the log file (stderr is already redirected).
  eprintln!("=== Hermes session started ===");

  // ── Session store ──────────────────────────────────────────────────────────
  let store = TransactionStore::open(&db)
    .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e.to_string()))?;

  // ── Script engine ──────────────────────────────────────────────────────────
  let engine = ScriptEngine::new().load_dir(&scripts_dir);

  if engine.is_empty() {
    eprintln!(
      "[scripts] no scripts found in '{}' — running in passthrough mode.",
      scripts_dir
    );
  }

  // ── Fan-out channel: proxy → TUI + sled ───────────────────────────────────
  let (tx_tui, rx_tui) = mpsc::unbounded_channel::<Transaction>();
  let (proxy_tx, mut proxy_relay_rx) = mpsc::unbounded_channel::<Transaction>();
  let (tx_store_in, mut rx_store) = mpsc::unbounded_channel::<Transaction>();

  // Relay: one transaction goes to both the TUI and the sled writer.
  tokio::spawn(async move {
    while let Some(tx) = proxy_relay_rx.recv().await {
      let _ = tx_tui.send(tx.clone());
      let _ = tx_store_in.send(tx);
    }
  });

  // Sled writer task.
  tokio::spawn(async move {
    while let Some(tx) = rx_store.recv().await {
      if let Err(e) = store.save(&tx) {
        eprintln!("[store] failed to persist {}: {}", tx.id, e);
      }
    }
  });

  // ── Proxy task ─────────────────────────────────────────────────────────────
  let config = ProxyConfig {
    bind_addr: bind.parse().unwrap_or_else(|_| {
      eprintln!("Invalid bind address '{}', using default.", bind);
      "127.0.0.1:8080".parse().unwrap()
    }),
  };

  tokio::spawn(async move {
    if let Err(e) = proxy::run(config, proxy_tx, engine).await {
      eprintln!("[proxy] fatal error: {}", e);
    }
  });

  // ── TUI (main task — owns the terminal) ───────────────────────────────────
  tui::run_tui(rx_tui).await
}

// ── `hermes replay` ───────────────────────────────────────────────────────────

async fn cmd_replay(id_str: String, db: String, show_diff: bool) -> std::io::Result<()> {
  let id = uuid::Uuid::parse_str(&id_str)
    .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidInput, e.to_string()))?;

  let store = TransactionStore::open(&db)
    .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e.to_string()))?;

  let tx = store
    .get(id)
    .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e.to_string()))?
    .ok_or_else(|| {
      std::io::Error::new(
        std::io::ErrorKind::NotFound,
        format!("Transaction {} not found in {}", id_str, db),
      )
    })?;

  replay::replay(&tx, show_diff).await
}

// ── `hermes list` ─────────────────────────────────────────────────────────────

fn cmd_list(db: String) -> std::io::Result<()> {
  let store = TransactionStore::open(&db)
    .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e.to_string()))?;

  let txns = store
    .all()
    .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e.to_string()))?;

  if txns.is_empty() {
    println!("No transactions stored in '{}'.", db);
    return Ok(());
  }

  println!(
    "{:<36}  {:<6}  {:<7}  {}",
    "ID", "STATUS", "METHOD", "URL"
  );
  println!("{}", "─".repeat(90));

  for tx in &txns {
    let status = tx
      .response
      .as_ref()
      .map(|r| r.status.to_string())
      .unwrap_or_else(|| "—".to_string());
    println!(
      "{:<36}  {:<6}  {:<7}  {}",
      tx.id, status, tx.request.method, tx.request.url
    );
  }

  println!("\n{} transaction(s) total.", txns.len());
  Ok(())
}

// ── Stderr redirect ───────────────────────────────────────────────────────────

/// Point file-descriptor 2 (stderr) at `path` so that proxy/script `eprintln!`
/// calls go to a log file instead of bleeding through the alternate screen.
///
/// Call this **before** `tui::run_tui` — once raw mode is active, anything
/// written to fd 2 corrupts the display.
#[cfg(unix)]
fn redirect_stderr(path: &str) -> std::io::Result<()> {
  use std::fs::OpenOptions;
  use std::os::unix::io::IntoRawFd;

  // Tell the user where to look before we switch to the TUI.
  println!("Proxy logs → {path}  (tail -f {path} in another terminal)");

  let file = OpenOptions::new()
    .create(true)
    .append(true)
    .open(path)?;

  let new_fd = file.into_raw_fd();

  // SAFETY: new_fd is a freshly opened, valid file descriptor.
  // dup2(new_fd, 2) atomically replaces stderr with our log file.
  let rc = unsafe { libc::dup2(new_fd, 2) };
  unsafe { libc::close(new_fd) };

  if rc == -1 {
    Err(std::io::Error::last_os_error())
  } else {
    Ok(())
  }
}

#[cfg(not(unix))]
fn redirect_stderr(_path: &str) -> std::io::Result<()> {
  Ok(()) // no-op on non-Unix; errors will print normally
}
