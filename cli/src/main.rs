use proxy::{ProxyConfig, run};

#[tokio::main]
async fn main() -> std::io::Result<()> {
  run(ProxyConfig::default()).await
}
