use crate::http::{
  BodyKind, body_kind, build_head, has_header, header_value, parse_absolute_target,
  read_request_head, read_response_head, relay_chunked, relay_fixed, remove_header,
  response_body_kind, split_host_port,
};

use std::io;
use std::net::{IpAddr, SocketAddr};
use std::path::Path;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll};
use std::time::Instant;

use scripts::{ScriptAction, ScriptEngine};
use store::{RequestData, ResponseData, Transaction};
use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncWrite, AsyncWriteExt, BufReader, ReadBuf};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::mpsc;
use tokio::time::{Duration, timeout};
use tokio_rustls::TlsAcceptor;
use tokio_rustls::TlsConnector;

use rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer, ServerName};
use rustls::{ClientConfig, RootCertStore, ServerConfig};
use webpki_roots::TLS_SERVER_ROOTS;

const CAPTURE_LIMIT: usize = 16 * 1024;
const CA_CERT_PATH: &str = "hermes-ca.crt";
const CA_KEY_PATH: &str = "hermes-ca.key";
/// How long to wait for a TCP connection to the upstream before giving up.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);

#[derive(Debug, Clone)]
pub struct ProxyConfig {
  pub bind_addr: SocketAddr,
}

impl Default for ProxyConfig {
  fn default() -> Self {
    Self {
      bind_addr: "127.0.0.1:8080".parse().expect("valid bind addr"),
    }
  }
}

/// Shared proxy state, cloned into each spawned task.
struct ProxyState {
  ca_cert: rcgen::Certificate,
  ca_key: rcgen::KeyPair,
  tls_connector: TlsConnector,
  cert_cache: Mutex<std::collections::HashMap<String, Arc<ServerConfig>>>,
  /// Sender half of the channel that feeds captured transactions to the TUI.
  tx_sink: mpsc::UnboundedSender<Transaction>,
  /// Rhai script pipeline — consulted before forwarding each request.
  script_engine: Arc<ScriptEngine>,
}

impl ProxyState {
  fn new(
    ca_cert: rcgen::Certificate,
    ca_key: rcgen::KeyPair,
    tx_sink: mpsc::UnboundedSender<Transaction>,
    script_engine: ScriptEngine,
  ) -> io::Result<Self> {
    let mut roots = RootCertStore::empty();
    roots.extend(TLS_SERVER_ROOTS.iter().cloned());

    let client_config = ClientConfig::builder()
      .with_root_certificates(roots)
      .with_no_client_auth();
    let tls_connector = TlsConnector::from(Arc::new(client_config));

    Ok(Self {
      ca_cert,
      ca_key,
      tls_connector,
      cert_cache: Mutex::new(std::collections::HashMap::new()),
      tx_sink,
      script_engine: Arc::new(script_engine),
    })
  }

  fn tls_acceptor_for_host(&self, host: &str) -> io::Result<TlsAcceptor> {
    let mut cache = self
      .cert_cache
      .lock()
      .map_err(|_| io::Error::new(io::ErrorKind::Other, "cert cache poisoned"))?;
    if let Some(config) = cache.get(host) {
      return Ok(TlsAcceptor::from(Arc::clone(config)));
    }

    let config = Arc::new(make_server_config(&self.ca_cert, &self.ca_key, host)?);
    cache.insert(host.to_string(), Arc::clone(&config));
    Ok(TlsAcceptor::from(config))
  }

  /// Send a finished `Transaction` to the TUI (fire-and-forget).
  fn emit(&self, tx: Transaction) {
    // Ignore send errors — the TUI may have exited before the proxy.
    let _ = self.tx_sink.send(tx);
  }
}

/// Run the proxy, forwarding every captured transaction to `tx_sink`.
/// `script_engine` is consulted before forwarding each request.
pub async fn run(
  config: ProxyConfig,
  tx_sink: mpsc::UnboundedSender<Transaction>,
  script_engine: ScriptEngine,
) -> io::Result<()> {
  let (ca_cert, ca_key) = load_or_create_ca()?;
  print_install_instructions(CA_CERT_PATH);
  let state = Arc::new(ProxyState::new(ca_cert, ca_key, tx_sink, script_engine)?);
  let listener = TcpListener::bind(config.bind_addr).await?;
  eprintln!("Hermes proxy listening on {}", config.bind_addr);

  loop {
    let (stream, peer) = listener.accept().await?;
    let state = Arc::clone(&state);
    tokio::spawn(async move {
      if let Err(err) = handle_client(stream, &state).await {
        eprintln!("connection from {} failed: {}", peer, err);
      }
    });
  }
}

async fn handle_client(stream: TcpStream, state: &Arc<ProxyState>) -> io::Result<()> {
  let mut reader = BufReader::new(stream);
  let request = match read_request_head(&mut reader).await? {
    Some(head) => head,
    None => return Ok(()),
  };

  if request.method.eq_ignore_ascii_case("CONNECT") {
    return handle_connect(reader, request.target, state).await;
  }

  handle_http(reader, request, state).await
}

async fn handle_connect(
  reader: BufReader<TcpStream>,
  target: String,
  state: &Arc<ProxyState>,
) -> io::Result<()> {
  let (host, port) = split_host_port(&target, 443)?;

  let mut reader = reader;
  let buffered = reader.buffer().to_vec();
  reader.consume(buffered.len());
  let mut client = reader.into_inner();
  client
    .write_all(b"HTTP/1.1 200 Connection Established\r\n\r\n")
    .await?;

  handle_connect_mitm(client, buffered, host, port, state).await
}

async fn handle_connect_mitm(
  client: TcpStream,
  buffered: Vec<u8>,
  host: String,
  port: u16,
  state: &Arc<ProxyState>,
) -> io::Result<()> {
  let client = PrefixedStream::new(client, buffered);
  let acceptor = state.tls_acceptor_for_host(&host)?;
  let client_tls = acceptor.accept(client).await?;

  let server_name = match host.parse::<IpAddr>() {
    Ok(ip) => ServerName::IpAddress(ip.into()),
    Err(_) => ServerName::try_from(host.clone())
      .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "invalid server name"))?,
  };
  let upstream = timeout(CONNECT_TIMEOUT, TcpStream::connect((host.as_str(), port)))
    .await
    .map_err(|_| {
      io::Error::new(
        io::ErrorKind::TimedOut,
        format!("connect to {}:{} timed out", host, port),
      )
    })??;
  let mut upstream_tls = state.tls_connector.connect(server_name, upstream).await?;

  let mut client_reader = BufReader::new(client_tls);
  let request = match read_request_head(&mut client_reader).await? {
    Some(head) => head,
    None => return Ok(()),
  };

  let (upstream_host, upstream_port, forward_target, mut headers) = resolve_upstream(&request)?;
  remove_header(&mut headers, "proxy-connection");
  force_close(&mut headers);
  if !has_header(&headers, "host") {
    let host_value = if upstream_port == 443 {
      upstream_host.clone()
    } else {
      format!("{}:{}", upstream_host, upstream_port)
    };
    headers.push(("Host".to_string(), host_value));
  }

  let url = format!("https://{}{}", upstream_host, forward_target);

  // ── Script pipeline ───────────────────────────────────────────────────────
  // Run on a blocking thread so Rhai doesn't starve the async runtime.
  let script_engine = Arc::clone(&state.script_engine);
  let (script_method, script_url, script_headers) =
    (request.method.clone(), url.clone(), headers.clone());
  let action = tokio::task::spawn_blocking(move || {
    script_engine.run(&script_method, &script_url, &script_headers)
  })
  .await
  .unwrap_or(ScriptAction::Passthrough);

  match action {
    ScriptAction::Passthrough => {}
    ScriptAction::ModifyHeaders(new_hdrs) => headers = new_hdrs,
    ScriptAction::Drop => {
      let resp = b"HTTP/1.1 502 Bad Gateway\r\nContent-Length: 0\r\nConnection: close\r\n\r\n";
      client_reader.get_mut().write_all(resp).await?;
      return Ok(());
    }
    ScriptAction::MockResponse { status, headers: resp_hdrs, body } => {
      let reason = http_reason(status);
      let start = format!("HTTP/1.1 {status} {reason}");
      let mut mock_hdrs = resp_hdrs;
      if !mock_hdrs.iter().any(|(n, _)| n.eq_ignore_ascii_case("content-length")) {
        mock_hdrs.push(("Content-Length".into(), body.len().to_string()));
      }
      mock_hdrs.push(("Connection".into(), "close".into()));
      let head = build_head(&start, &mock_hdrs);
      client_reader.get_mut().write_all(&head).await?;
      client_reader.get_mut().write_all(&body).await?;
      return Ok(());
    }
  }

  let start_line = format!("{} {} {}", request.method, forward_target, request.version);
  let head_bytes = build_head(&start_line, &headers);
  upstream_tls.write_all(&head_bytes).await?;

  let mut request_capture = Vec::new();
  let start = Instant::now();

  match body_kind(&headers) {
    BodyKind::None => {}
    BodyKind::ContentLength(len) => {
      relay_fixed(
        &mut client_reader,
        &mut upstream_tls,
        len,
        &mut request_capture,
        CAPTURE_LIMIT,
      )
      .await?;
    }
    BodyKind::Chunked => {
      relay_chunked(
        &mut client_reader,
        &mut upstream_tls,
        &mut request_capture,
        CAPTURE_LIMIT,
      )
      .await?;
    }
  }

  // Build the Transaction (no response yet).
  let mut transaction = Transaction::new(RequestData {
    method: request.method.clone(),
    url,
    headers: headers.clone(),
    body: request_capture,
  });

  let mut upstream_reader = BufReader::new(&mut upstream_tls);
  let response = read_response_head(&mut upstream_reader).await?;
  // Rewrite the response head so the client sees Connection: close and won't
  // try to pipeline another request on this already-dying tunnel.
  let rewritten_head = rewrite_response_head_close(&response.start_line, &response.headers);
  client_reader.get_mut().write_all(&rewritten_head).await?;

  let mut response_capture = Vec::new();
  match response_body_kind(response.status_code, &response.headers) {
    BodyKind::None => {}
    BodyKind::ContentLength(len) => {
      relay_fixed(
        &mut upstream_reader,
        client_reader.get_mut(),
        len,
        &mut response_capture,
        CAPTURE_LIMIT,
      )
      .await?;
    }
    BodyKind::Chunked => {
      relay_chunked(
        &mut upstream_reader,
        client_reader.get_mut(),
        &mut response_capture,
        CAPTURE_LIMIT,
      )
      .await?;
    }
  }

  let duration_ms = start.elapsed().as_millis() as u64;
  transaction.response = Some(ResponseData {
    status: response.status_code,
    headers: response.headers,
    body: response_capture,
  });
  transaction.duration_ms = Some(duration_ms);

  state.emit(transaction);

  // BrokenPipe here means the client already closed its side after reading the
  // response — that's expected and harmless, so we swallow it.
  if let Err(e) = client_reader.get_mut().shutdown().await {
    if e.kind() != io::ErrorKind::BrokenPipe {
      return Err(e);
    }
  }
  Ok(())
}

async fn handle_http(
  mut reader: BufReader<TcpStream>,
  request: crate::http::RequestHead,
  state: &Arc<ProxyState>,
) -> io::Result<()> {
  let (upstream_host, upstream_port, forward_target, mut headers) = resolve_upstream(&request)?;

  remove_header(&mut headers, "proxy-connection");
  force_close(&mut headers);
  if !has_header(&headers, "host") {
    let host_value = if upstream_port == 80 {
      upstream_host.clone()
    } else {
      format!("{}:{}", upstream_host, upstream_port)
    };
    headers.push(("Host".to_string(), host_value));
  }

  let url = format!("http://{}{}", upstream_host, forward_target);

  // ── Script pipeline ───────────────────────────────────────────────────────
  let script_engine = Arc::clone(&state.script_engine);
  let (script_method, script_url, script_headers) =
    (request.method.clone(), url.clone(), headers.clone());
  let action = tokio::task::spawn_blocking(move || {
    script_engine.run(&script_method, &script_url, &script_headers)
  })
  .await
  .unwrap_or(ScriptAction::Passthrough);

  match action {
    ScriptAction::Passthrough => {}
    ScriptAction::ModifyHeaders(new_hdrs) => headers = new_hdrs,
    ScriptAction::Drop => {
      let resp = b"HTTP/1.1 502 Bad Gateway\r\nContent-Length: 0\r\nConnection: close\r\n\r\n";
      reader.get_mut().write_all(resp).await?;
      return Ok(());
    }
    ScriptAction::MockResponse { status, headers: resp_hdrs, body } => {
      let reason = http_reason(status);
      let start = format!("HTTP/1.1 {status} {reason}");
      let mut mock_hdrs = resp_hdrs;
      if !mock_hdrs.iter().any(|(n, _)| n.eq_ignore_ascii_case("content-length")) {
        mock_hdrs.push(("Content-Length".into(), body.len().to_string()));
      }
      mock_hdrs.push(("Connection".into(), "close".into()));
      let head = build_head(&start, &mock_hdrs);
      reader.get_mut().write_all(&head).await?;
      reader.get_mut().write_all(&body).await?;
      return Ok(());
    }
  }

  let start_line = format!("{} {} {}", request.method, forward_target, request.version);
  let head_bytes = build_head(&start_line, &headers);

  let mut upstream = timeout(
    CONNECT_TIMEOUT,
    TcpStream::connect((upstream_host.as_str(), upstream_port)),
  )
  .await
  .map_err(|_| {
    io::Error::new(
      io::ErrorKind::TimedOut,
      format!("connect to {}:{} timed out", upstream_host, upstream_port),
    )
  })??;
  upstream.write_all(&head_bytes).await?;

  let mut request_capture = Vec::new();
  let start = Instant::now();

  match body_kind(&headers) {
    BodyKind::None => {}
    BodyKind::ContentLength(len) => {
      relay_fixed(
        &mut reader,
        &mut upstream,
        len,
        &mut request_capture,
        CAPTURE_LIMIT,
      )
      .await?;
    }
    BodyKind::Chunked => {
      relay_chunked(
        &mut reader,
        &mut upstream,
        &mut request_capture,
        CAPTURE_LIMIT,
      )
      .await?;
    }
  }

  let mut transaction = Transaction::new(RequestData {
    method: request.method.clone(),
    url,
    headers: headers.clone(),
    body: request_capture,
  });

  let mut upstream_reader = BufReader::new(upstream);
  let response = read_response_head(&mut upstream_reader).await?;
  let client = reader.get_mut();
  let rewritten_head = rewrite_response_head_close(&response.start_line, &response.headers);
  client.write_all(&rewritten_head).await?;

  let mut response_capture = Vec::new();
  match response_body_kind(response.status_code, &response.headers) {
    BodyKind::None => {}
    BodyKind::ContentLength(len) => {
      relay_fixed(
        &mut upstream_reader,
        &mut *client,
        len,
        &mut response_capture,
        CAPTURE_LIMIT,
      )
      .await?;
    }
    BodyKind::Chunked => {
      relay_chunked(
        &mut upstream_reader,
        &mut *client,
        &mut response_capture,
        CAPTURE_LIMIT,
      )
      .await?;
    }
  }

  let duration_ms = start.elapsed().as_millis() as u64;
  transaction.response = Some(ResponseData {
    status: response.status_code,
    headers: response.headers,
    body: response_capture,
  });
  transaction.duration_ms = Some(duration_ms);

  state.emit(transaction);

  // BrokenPipe here means the client already closed its side after reading the
  // response — that's expected and harmless, so we swallow it.
  if let Err(e) = client.shutdown().await {
    if e.kind() != io::ErrorKind::BrokenPipe {
      return Err(e);
    }
  }
  Ok(())
}

/// Map a status code to its standard reason phrase.
fn http_reason(status: u16) -> &'static str {
  match status {
    200 => "OK",
    201 => "Created",
    204 => "No Content",
    301 => "Moved Permanently",
    302 => "Found",
    304 => "Not Modified",
    400 => "Bad Request",
    401 => "Unauthorized",
    403 => "Forbidden",
    404 => "Not Found",
    405 => "Method Not Allowed",
    418 => "I'm a Teapot",
    429 => "Too Many Requests",
    500 => "Internal Server Error",
    502 => "Bad Gateway",
    503 => "Service Unavailable",
    504 => "Gateway Timeout",
    _ => "Unknown",
  }
}

fn resolve_upstream(
  request: &crate::http::RequestHead,
) -> io::Result<(String, u16, String, Vec<(String, String)>)> {
  let headers = request.headers.clone();

  if let Some(target) = parse_absolute_target(&request.target) {
    return Ok((target.host, target.port, target.path, headers));
  }

  let host_header = header_value(&headers, "host")
    .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "missing Host header"))?;
  let (host, port) = split_host_port(host_header, 80)?;
  Ok((host, port, request.target.clone(), headers))
}

/// Replace (or insert) `Connection: close` in a set of request headers.
///
/// This tells the upstream server to close the connection after one response,
/// which avoids the broken-pipe / retry latency caused by curl trying to
/// pipeline a second request on an already-dying CONNECT tunnel.
fn force_close(headers: &mut Vec<(String, String)>) {
  remove_header(headers, "connection");
  remove_header(headers, "keep-alive");
  headers.push(("Connection".to_string(), "close".to_string()));
}

/// Build a fresh response head that is identical to the upstream's, except
/// `Connection` is overridden to `close`.  This prevents the client (curl,
/// browser, …) from believing the tunnel is reusable.
fn rewrite_response_head_close(start_line: &str, headers: &[(String, String)]) -> Vec<u8> {
  let mut new_headers: Vec<(String, String)> = headers
    .iter()
    .filter(|(n, _)| !n.eq_ignore_ascii_case("connection") && !n.eq_ignore_ascii_case("keep-alive"))
    .cloned()
    .collect();
  new_headers.push(("Connection".to_string(), "close".to_string()));
  build_head(start_line, &new_headers)
}

fn load_or_create_ca() -> io::Result<(rcgen::Certificate, rcgen::KeyPair)> {
  if Path::new(CA_CERT_PATH).exists() && Path::new(CA_KEY_PATH).exists() {
    let cert_pem = std::fs::read_to_string(CA_CERT_PATH)?;
    let key_pem = std::fs::read_to_string(CA_KEY_PATH)?;
    let params = rcgen::CertificateParams::from_ca_cert_pem(&cert_pem)
      .map_err(|err| io::Error::new(io::ErrorKind::InvalidData, err.to_string()))?;
    let key = rcgen::KeyPair::from_pem(&key_pem)
      .map_err(|err| io::Error::new(io::ErrorKind::InvalidData, err.to_string()))?;
    let cert = params
      .self_signed(&key)
      .map_err(|err| io::Error::new(io::ErrorKind::InvalidData, err.to_string()))?;
    return Ok((cert, key));
  }

  let mut params = rcgen::CertificateParams::new(vec!["Hermes Proxy CA".to_string()])
    .map_err(|err| io::Error::new(io::ErrorKind::InvalidData, err.to_string()))?;
  params.is_ca = rcgen::IsCa::Ca(rcgen::BasicConstraints::Unconstrained);
  let key = rcgen::KeyPair::generate()
    .map_err(|err| io::Error::new(io::ErrorKind::Other, err.to_string()))?;
  let cert = params
    .self_signed(&key)
    .map_err(|err| io::Error::new(io::ErrorKind::Other, err.to_string()))?;
  let cert_pem = cert.pem();
  let key_pem = key.serialize_pem();
  std::fs::write(CA_CERT_PATH, cert_pem)?;
  std::fs::write(CA_KEY_PATH, key_pem)?;
  Ok((cert, key))
}

fn make_server_config(
  ca_cert: &rcgen::Certificate,
  ca_key: &rcgen::KeyPair,
  host: &str,
) -> io::Result<ServerConfig> {
  let mut params = rcgen::CertificateParams::new(vec![host.to_string()])
    .map_err(|err| io::Error::new(io::ErrorKind::InvalidData, err.to_string()))?;
  params.distinguished_name = rcgen::DistinguishedName::new();
  params
    .distinguished_name
    .push(rcgen::DnType::CommonName, host);
  let key = rcgen::KeyPair::generate()
    .map_err(|err| io::Error::new(io::ErrorKind::Other, err.to_string()))?;
  let cert = params
    .signed_by(&key, ca_cert, ca_key)
    .map_err(|err| io::Error::new(io::ErrorKind::Other, err.to_string()))?;
  let cert_der = cert.der().to_vec();
  let ca_der = ca_cert.der().to_vec();
  let key_der = key.serialize_der();

  let cert_chain = vec![CertificateDer::from(cert_der), CertificateDer::from(ca_der)];
  let key = PrivateKeyDer::from(PrivatePkcs8KeyDer::from(key_der));

  let config = ServerConfig::builder()
    .with_no_client_auth()
    .with_single_cert(cert_chain, key)
    .map_err(|err| io::Error::new(io::ErrorKind::Other, err.to_string()))?;
  Ok(config)
}

fn print_install_instructions(cert_path: &str) {
  eprintln!("\nHermes root CA: {}", cert_path);
  eprintln!("Install into your trust store to intercept HTTPS:");
  eprintln!("  Linux (NSS):");
  eprintln!(
    "    certutil -d sql:$HOME/.pki/nssdb -A -t \"CT,C,C\" -n \"Hermes Proxy CA\" -i {}",
    cert_path
  );
  eprintln!("  macOS:");
  eprintln!(
    "    sudo security add-trusted-cert -d -r trustRoot -k /Library/Keychains/System.keychain {}",
    cert_path
  );
  eprintln!("\nTest with:");
  eprintln!(
    "  curl -x http://localhost:8080 --cacert {} https://httpbin.org/get",
    cert_path
  );
}

// ── PrefixedStream — replays buffered bytes before reading the inner stream ──

struct PrefixedStream<S> {
  inner: S,
  buffer: Vec<u8>,
  offset: usize,
}

impl<S> PrefixedStream<S> {
  fn new(inner: S, buffer: Vec<u8>) -> Self {
    Self {
      inner,
      buffer,
      offset: 0,
    }
  }
}

impl<S: AsyncRead + Unpin> AsyncRead for PrefixedStream<S> {
  fn poll_read(
    mut self: Pin<&mut Self>,
    cx: &mut Context<'_>,
    buf: &mut ReadBuf<'_>,
  ) -> Poll<io::Result<()>> {
    if self.offset < self.buffer.len() {
      let remaining = &self.buffer[self.offset..];
      let to_copy = remaining.len().min(buf.remaining());
      buf.put_slice(&remaining[..to_copy]);
      self.offset += to_copy;
      return Poll::Ready(Ok(()));
    }

    Pin::new(&mut self.inner).poll_read(cx, buf)
  }
}

impl<S: AsyncWrite + Unpin> AsyncWrite for PrefixedStream<S> {
  fn poll_write(
    mut self: Pin<&mut Self>,
    cx: &mut Context<'_>,
    data: &[u8],
  ) -> Poll<io::Result<usize>> {
    Pin::new(&mut self.inner).poll_write(cx, data)
  }

  fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
    Pin::new(&mut self.inner).poll_flush(cx)
  }

  fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
    Pin::new(&mut self.inner).poll_shutdown(cx)
  }
}
