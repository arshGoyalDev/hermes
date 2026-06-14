use std::io;
use std::time::Instant;

use store::{ResponseData, Transaction};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::time::{Duration, timeout};

const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);

/// Re-send `original`'s request and print the result.
/// If `show_diff` is true, compare response bodies side-by-side.
pub async fn replay(original: &Transaction, show_diff: bool) -> io::Result<()> {
  let req = &original.request;
  println!("━━━ Replaying transaction {} ━━━", original.id);
  println!("  {} {}", req.method, req.url);
  println!();

  // ── Determine scheme, host, port, path ────────────────────────────────────
  let (scheme, rest) = if let Some(s) = req.url.strip_prefix("https://") {
    ("https", s)
  } else if let Some(s) = req.url.strip_prefix("http://") {
    ("http", s)
  } else {
    return Err(io::Error::new(
      io::ErrorKind::InvalidInput,
      format!("Cannot parse URL: {}", req.url),
    ));
  };

  let (authority, path) = rest
    .split_once('/')
    .map(|(a, p)| (a, format!("/{p}")))
    .unwrap_or((rest, "/".to_string()));

  let default_port: u16 = if scheme == "https" { 443 } else { 80 };
  let (host, port) = split_host_port(authority, default_port);

  // ── Build request head ────────────────────────────────────────────────────
  let start_line = format!("{} {} HTTP/1.1", req.method, path);
  let mut headers = req.headers.clone();
  // Ensure Connection: close so we can read the response until EOF.
  headers.retain(|(n, _)| !n.eq_ignore_ascii_case("connection"));
  headers.push(("Connection".into(), "close".into()));

  let head = build_head(&start_line, &headers);

  // ── Connect and send ──────────────────────────────────────────────────────
  let start = Instant::now();

  let response = if scheme == "https" {
    send_https(&host, port, &head, &req.body).await?
  } else {
    send_http(&host, port, &head, &req.body).await?
  };

  let duration_ms = start.elapsed().as_millis();

  // ── Print result ──────────────────────────────────────────────────────────
  println!("━━━ New Response ━━━");
  println!("  Status  : {}", response.status);
  println!("  Duration: {} ms", duration_ms);
  println!("  Headers :");
  for (n, v) in &response.headers {
    println!("    {}: {}", n, v);
  }
  if !response.body.is_empty() {
    println!("  Body ({} bytes):", response.body.len());
    println!("{}", String::from_utf8_lossy(&response.body));
  }

  // ── Diff ──────────────────────────────────────────────────────────────────
  if show_diff {
    if let Some(orig_resp) = &original.response {
      println!();
      println!("━━━ Diff (original vs replayed body) ━━━");
      print_diff(
        &String::from_utf8_lossy(&orig_resp.body),
        &String::from_utf8_lossy(&response.body),
      );
    } else {
      println!("(original response not recorded — no diff available)");
    }
  }

  Ok(())
}

// ── HTTP (plain TCP) ──────────────────────────────────────────────────────────

async fn send_http(
  host: &str,
  port: u16,
  head: &[u8],
  body: &[u8],
) -> io::Result<ResponseData> {
  let mut stream = timeout(CONNECT_TIMEOUT, TcpStream::connect((host, port)))
    .await
    .map_err(|_| io::Error::new(io::ErrorKind::TimedOut, "connect timed out"))??;

  stream.write_all(head).await?;
  if !body.is_empty() {
    stream.write_all(body).await?;
  }

  read_response(stream).await
}

// ── HTTPS (rustls) ────────────────────────────────────────────────────────────

async fn send_https(
  host: &str,
  port: u16,
  head: &[u8],
  body: &[u8],
) -> io::Result<ResponseData> {
  use rustls::{ClientConfig, RootCertStore};
  use rustls::pki_types::ServerName;
  use tokio_rustls::TlsConnector;
  use webpki_roots::TLS_SERVER_ROOTS;

  let mut roots = RootCertStore::empty();
  roots.extend(TLS_SERVER_ROOTS.iter().cloned());
  let client_config = ClientConfig::builder()
    .with_root_certificates(roots)
    .with_no_client_auth();
  let connector = TlsConnector::from(std::sync::Arc::new(client_config));

  let server_name = ServerName::try_from(host.to_string())
    .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "invalid host for TLS"))?;

  let tcp = timeout(CONNECT_TIMEOUT, TcpStream::connect((host, port)))
    .await
    .map_err(|_| io::Error::new(io::ErrorKind::TimedOut, "connect timed out"))??;

  let mut tls = connector.connect(server_name, tcp).await?;

  tls.write_all(head).await?;
  if !body.is_empty() {
    tls.write_all(body).await?;
  }

  read_response(tls).await
}

// ── Response reader ───────────────────────────────────────────────────────────

async fn read_response<S: AsyncReadExt + Unpin>(mut stream: S) -> io::Result<ResponseData> {
  // Read everything until EOF (we always send Connection: close).
  let mut raw = Vec::new();
  stream.read_to_end(&mut raw).await?;

  // Parse status line.
  let header_end = raw
    .windows(4)
    .position(|w| w == b"\r\n\r\n")
    .map(|i| i + 4)
    .unwrap_or(raw.len());

  let header_section = &raw[..header_end.min(raw.len())];
  let header_str = String::from_utf8_lossy(header_section);
  let mut lines = header_str.lines();

  let status_line = lines.next().unwrap_or("HTTP/1.1 0 Unknown");
  let status: u16 = status_line
    .split_whitespace()
    .nth(1)
    .and_then(|s| s.parse().ok())
    .unwrap_or(0);

  let headers: Vec<(String, String)> = lines
    .filter(|l| !l.is_empty())
    .filter_map(|l| l.split_once(':').map(|(n, v)| (n.trim().to_string(), v.trim().to_string())))
    .collect();

  let body = if header_end < raw.len() {
    raw[header_end..].to_vec()
  } else {
    Vec::new()
  };

  Ok(ResponseData {
    status,
    headers,
    body,
  })
}

// ── Utilities ─────────────────────────────────────────────────────────────────

fn split_host_port(authority: &str, default_port: u16) -> (String, u16) {
  if let Some((host, port_str)) = authority.rsplit_once(':') {
    if let Ok(port) = port_str.parse() {
      return (host.to_string(), port);
    }
  }
  (authority.to_string(), default_port)
}

fn build_head(start_line: &str, headers: &[(String, String)]) -> Vec<u8> {
  let mut data = Vec::new();
  data.extend_from_slice(start_line.as_bytes());
  data.extend_from_slice(b"\r\n");
  for (n, v) in headers {
    data.extend_from_slice(n.as_bytes());
    data.extend_from_slice(b": ");
    data.extend_from_slice(v.as_bytes());
    data.extend_from_slice(b"\r\n");
  }
  data.extend_from_slice(b"\r\n");
  data
}

/// Print a simple line-level diff between two strings.
fn print_diff(original: &str, replayed: &str) {
  let orig_lines: Vec<&str> = original.lines().collect();
  let new_lines: Vec<&str> = replayed.lines().collect();
  let max = orig_lines.len().max(new_lines.len());

  if original == replayed {
    println!("  (responses are identical)");
    return;
  }

  for i in 0..max {
    match (orig_lines.get(i), new_lines.get(i)) {
      (Some(o), Some(n)) if o == n => println!("    {}", o),
      (Some(o), Some(n)) => {
        println!("  - {}", o);
        println!("  + {}", n);
      }
      (Some(o), None) => println!("  - {}", o),
      (None, Some(n)) => println!("  + {}", n),
      (None, None) => {}
    }
  }
}
