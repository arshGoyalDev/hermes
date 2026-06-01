use crate::http::{
  BodyKind, body_kind, build_head, has_header, header_value, parse_absolute_target,
  read_request_head, read_response_head, relay_chunked, relay_fixed, remove_header,
  response_body_kind, split_host_port,
};

use std::io;
use std::net::SocketAddr;

use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{TcpListener, TcpStream};

const CAPTURE_LIMIT: usize = 16 * 1024;

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

pub async fn run(config: ProxyConfig) -> io::Result<()> {
  let listener = TcpListener::bind(config.bind_addr).await?;
  eprintln!("Hermes proxy listening on {}", config.bind_addr);

  loop {
    let (stream, peer) = listener.accept().await?;
    tokio::spawn(async move {
      if let Err(err) = handle_client(stream).await {
        eprintln!("connection from {} failed: {}", peer, err);
      }
    });
  }
}

async fn handle_client(stream: TcpStream) -> io::Result<()> {
  let mut reader = BufReader::new(stream);
  let request = match read_request_head(&mut reader).await? {
    Some(head) => head,
    None => return Ok(()),
  };

  if request.method.eq_ignore_ascii_case("CONNECT") {
    return handle_connect(reader, request.target).await;
  }

  handle_http(reader, request).await
}

async fn handle_connect(reader: BufReader<TcpStream>, target: String) -> io::Result<()> {
    let (host, port) = split_host_port(&target, 443)?;
    eprintln!(">> CONNECT {}:{}", host, port);
    let mut upstream = TcpStream::connect((host.as_str(), port)).await?;

    let mut reader = reader;
    let buffered = reader.buffer().to_vec();
    reader.consume(buffered.len());
    let mut client = reader.into_inner();
    eprintln!("<< 200 Connection Established");
    client
        .write_all(b"HTTP/1.1 200 Connection Established\r\n\r\n")
        .await?;
    eprintln!("-- Tunneling encrypted TLS traffic");
    if !buffered.is_empty() {
        upstream.write_all(&buffered).await?;
    }
    let _ = tokio::io::copy_bidirectional(&mut client, &mut upstream).await?;
    Ok(())
}

async fn handle_http(
  mut reader: BufReader<TcpStream>,
  request: crate::http::RequestHead,
) -> io::Result<()> {
  let (upstream_host, upstream_port, forward_target, mut headers) = resolve_upstream(&request)?;

  remove_header(&mut headers, "proxy-connection");
  if !has_header(&headers, "host") {
    let host_value = if upstream_port == 80 {
      upstream_host.clone()
    } else {
      format!("{}:{}", upstream_host, upstream_port)
    };
    headers.push(("Host".to_string(), host_value));
  }

  let start_line = format!("{} {} {}", request.method, forward_target, request.version);
  let head_bytes = build_head(&start_line, &headers);

  let mut upstream = TcpStream::connect((upstream_host.as_str(), upstream_port)).await?;
  upstream.write_all(&head_bytes).await?;
  log_head("request", &start_line, &headers);

  let mut request_capture = Vec::new();
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

  log_body("request body", &request_capture);

  let mut upstream_reader = BufReader::new(upstream);
  let response = read_response_head(&mut upstream_reader).await?;
  log_response_head(&response.start_line, &response.headers);
  let client = reader.get_mut();
  client.write_all(&response.raw).await?;

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

  log_body("response body", &response_capture);
  client.shutdown().await?;
  Ok(())
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

fn log_head(kind: &str, start_line: &str, headers: &[(String, String)]) {
  println!("{}: {}", kind, start_line);
  for (name, value) in headers {
    println!("{}: {}: {}", kind, name, value);
  }
  println!("{}:", kind);
}

fn log_response_head(start_line: &str, headers: &[(String, String)]) {
  println!("response: {}", start_line);
  for (name, value) in headers {
    println!("response: {}: {}", name, value);
  }
  println!("response:");
}

fn log_body(label: &str, body: &[u8]) {
  if body.is_empty() {
    return;
  }
  let display = String::from_utf8_lossy(body);
  println!("{} ({} bytes):", label, body.len());
  println!("{}", display);
}
