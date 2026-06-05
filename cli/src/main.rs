use clap::Parser;
use proxy::ProxyConfig;
use store::{Transaction, TransactionStore};
use tokio::sync::mpsc;

#[derive(Parser, Debug)]
#[command(name = "hermes", version, about)]
struct Cli {
  #[arg(long, default_value = "127.0.0.1:8080")]
  bind: String,

  #[arg(long, default_value = ".hermes-sessions")]
  db: String,
}

#[tokio::main]
async fn main() -> std::io::Result<()> {
  let cli = Cli::parse();
  let store = TransactionStore::open(&cli.db)
    .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e.to_string()))?;
  let (tx_sink, rx_tui) = mpsc::unbounded_channel::<Transaction>();
  let tx_for_store = tx_sink.clone();
  let (tx_store_in, mut rx_store) = mpsc::unbounded_channel::<Transaction>();
  let (proxy_tx, mut proxy_relay_rx) = mpsc::unbounded_channel::<Transaction>();

  tokio::spawn(async move {
    while let Some(tx) = proxy_relay_rx.recv().await {
      let _ = tx_for_store.send(tx.clone());
      let _ = tx_store_in.send(tx);
    }
  });

  tokio::spawn(async move {
    while let Some(tx) = rx_store.recv().await {
      if let Err(e) = store.save(&tx) {
        eprintln!("[store] failed to persist transaction {}: {}", tx.id, e);
      }
    }
  });

  let config = ProxyConfig {
    bind_addr: cli.bind.parse().unwrap_or_else(|_| {
      eprintln!("Invalid bind address '{}', using default.", cli.bind);
      "127.0.0.1:8080".parse().unwrap()
    }),
  };

  tokio::spawn(async move {
    if let Err(e) = proxy::run(config, proxy_tx).await {
      eprintln!("[proxy] fatal error: {}", e);
    }
  });

  tui::run_tui(rx_tui).await
}
