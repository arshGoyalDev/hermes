use std::io;
use std::str::FromStr;

use tokio::io::{
  AsyncBufRead, AsyncBufReadExt, AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt,
};

#[derive(Debug, Clone)]
pub struct RequestHead {
  pub method: String,
  pub target: String,
  pub version: String,
  pub headers: Vec<(String, String)>,
}

#[derive(Debug, Clone)]
pub struct ResponseHead {
  pub start_line: String,
  pub status_code: u16,
  pub headers: Vec<(String, String)>,
  pub raw: Vec<u8>,
}

#[derive(Debug, Clone, Copy)]
pub enum BodyKind {
  None,
  ContentLength(u64),
  Chunked,
}

#[derive(Debug, Clone)]
pub struct AbsoluteTarget {
  pub host: String,
  pub port: u16,
  pub path: String,
}

pub fn header_value<'a>(headers: &'a [(String, String)], name: &str) -> Option<&'a str> {
  let needle = name.to_ascii_lowercase();
  headers
    .iter()
    .find(|(n, _)| n.to_ascii_lowercase() == needle)
    .map(|(_, v)| v.as_str())
}

pub fn remove_header(headers: &mut Vec<(String, String)>, name: &str) {
  let needle = name.to_ascii_lowercase();
  headers.retain(|(n, _)| n.to_ascii_lowercase() != needle);
}

pub fn has_header(headers: &[(String, String)], name: &str) -> bool {
  header_value(headers, name).is_some()
}

pub fn body_kind(headers: &[(String, String)]) -> BodyKind {
  if let Some(encoding) = header_value(headers, "transfer-encoding") {
    if encoding.to_ascii_lowercase().contains("chunked") {
      return BodyKind::Chunked;
    }
  }
  if let Some(length) = header_value(headers, "content-length") {
    if let Ok(value) = length.trim().parse::<u64>() {
      return BodyKind::ContentLength(value);
    }
  }
  BodyKind::None
}

pub fn response_body_kind(status_code: u16, headers: &[(String, String)]) -> BodyKind {
  if (100..200).contains(&status_code) || status_code == 204 || status_code == 304 {
    return BodyKind::None;
  }
  body_kind(headers)
}

pub fn build_head(start_line: &str, headers: &[(String, String)]) -> Vec<u8> {
  let mut data = Vec::with_capacity(start_line.len() + headers.len() * 32 + 4);
  data.extend_from_slice(start_line.as_bytes());
  data.extend_from_slice(b"\r\n");
  for (name, value) in headers {
    data.extend_from_slice(name.as_bytes());
    data.extend_from_slice(b": ");
    data.extend_from_slice(value.as_bytes());
    data.extend_from_slice(b"\r\n");
  }
  data.extend_from_slice(b"\r\n");
  data
}

pub async fn read_request_head<R: AsyncBufRead + Unpin>(
  reader: &mut R,
) -> io::Result<Option<RequestHead>> {
  let mut start_line = String::new();
  let bytes = reader.read_line(&mut start_line).await?;
  if bytes == 0 {
    return Ok(None);
  }
  let start_line = start_line.trim_end_matches(['\r', '\n']).to_string();
  if start_line.is_empty() {
    return Ok(None);
  }

  let (method, target, version) = parse_request_line(&start_line)?;
  let headers = read_headers(reader, None).await?;

  Ok(Some(RequestHead {
    method,
    target,
    version,
    headers,
  }))
}

pub async fn read_response_head<R: AsyncBufRead + Unpin>(
  reader: &mut R,
) -> io::Result<ResponseHead> {
  let mut raw = Vec::new();
  let mut start_line = String::new();
  let bytes = reader.read_line(&mut start_line).await?;
  if bytes == 0 {
    return Err(io::Error::new(
      io::ErrorKind::UnexpectedEof,
      "upstream closed while reading response line",
    ));
  }
  raw.extend_from_slice(start_line.as_bytes());
  let start_line = start_line.trim_end_matches(['\r', '\n']).to_string();
  let status_code = parse_status_code(&start_line);
  let headers = read_headers(reader, Some(&mut raw)).await?;
  Ok(ResponseHead {
    start_line,
    status_code,
    headers,
    raw,
  })
}

async fn read_headers<R: AsyncBufRead + Unpin>(
  reader: &mut R,
  mut raw: Option<&mut Vec<u8>>,
) -> io::Result<Vec<(String, String)>> {
  let mut headers = Vec::new();
  loop {
    let mut line = String::new();
    let bytes = reader.read_line(&mut line).await?;
    if bytes == 0 {
      break;
    }
    if let Some(raw) = raw.as_deref_mut() {
      raw.extend_from_slice(line.as_bytes());
    }
    let trimmed = line.trim_end_matches(['\r', '\n']);
    if trimmed.is_empty() {
      break;
    }
    if let Some((name, value)) = trimmed.split_once(':') {
      headers.push((name.trim().to_string(), value.trim().to_string()));
    }
  }
  Ok(headers)
}

fn parse_request_line(line: &str) -> io::Result<(String, String, String)> {
  let mut parts = line.split_whitespace();
  let method = parts
    .next()
    .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "missing method in request line"))?;
  let target = parts
    .next()
    .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "missing target in request line"))?;
  let version = parts.next().ok_or_else(|| {
    io::Error::new(
      io::ErrorKind::InvalidData,
      "missing version in request line",
    )
  })?;
  Ok((method.to_string(), target.to_string(), version.to_string()))
}

fn parse_status_code(line: &str) -> u16 {
  let mut parts = line.split_whitespace();
  let _version = parts.next();
  parts
    .next()
    .and_then(|code| u16::from_str(code).ok())
    .unwrap_or(0)
}

pub fn parse_absolute_target(target: &str) -> Option<AbsoluteTarget> {
  let (default_port, rest) = if let Some(stripped) = target.strip_prefix("http://") {
    (80, stripped)
  } else if let Some(stripped) = target.strip_prefix("https://") {
    (443, stripped)
  } else {
    return None;
  };

  let mut parts = rest.splitn(2, '/');
  let authority = parts.next().unwrap_or_default();
  let path = parts
    .next()
    .map(|p| format!("/{}", p))
    .unwrap_or_else(|| "/".to_string());
  let (host, port) = split_host_port(authority, default_port).ok()?;
  Some(AbsoluteTarget { host, port, path })
}

pub fn split_host_port(authority: &str, default_port: u16) -> io::Result<(String, u16)> {
  if authority.starts_with('[') {
    if let Some(end) = authority.find(']') {
      let host = authority[..=end].to_string();
      let port = if let Some(port_str) = authority[end + 1..].strip_prefix(':') {
        port_str.parse().unwrap_or(default_port)
      } else {
        default_port
      };
      return Ok((host, port));
    }
  }

  if let Some((host, port_str)) = authority.rsplit_once(':') {
    if let Ok(port) = port_str.parse() {
      return Ok((host.to_string(), port));
    }
  }

  Ok((authority.to_string(), default_port))
}

pub async fn relay_fixed<R, W>(
  reader: &mut R,
  writer: &mut W,
  mut remaining: u64,
  capture: &mut Vec<u8>,
  capture_limit: usize,
) -> io::Result<()>
where
  R: AsyncRead + Unpin,
  W: AsyncWrite + Unpin,
{
  let mut buffer = [0u8; 8192];
  while remaining > 0 {
    let to_read = remaining.min(buffer.len() as u64) as usize;
    let read = reader.read(&mut buffer[..to_read]).await?;
    if read == 0 {
      return Err(io::Error::new(
        io::ErrorKind::UnexpectedEof,
        "unexpected EOF while reading body",
      ));
    }
    writer.write_all(&buffer[..read]).await?;
    if capture.len() < capture_limit {
      let take = (capture_limit - capture.len()).min(read);
      capture.extend_from_slice(&buffer[..take]);
    }
    remaining -= read as u64;
  }
  Ok(())
}

pub async fn relay_chunked<R, W>(
  reader: &mut R,
  writer: &mut W,
  capture: &mut Vec<u8>,
  capture_limit: usize,
) -> io::Result<()>
where
  R: AsyncBufRead + Unpin,
  W: AsyncWrite + Unpin,
{
  loop {
    let mut size_line = String::new();
    let bytes = reader.read_line(&mut size_line).await?;
    if bytes == 0 {
      return Err(io::Error::new(
        io::ErrorKind::UnexpectedEof,
        "unexpected EOF while reading chunk size",
      ));
    }
    writer.write_all(size_line.as_bytes()).await?;
    let size_str = size_line
      .trim_end_matches(['\r', '\n'])
      .split(';')
      .next()
      .unwrap_or("");
    let size = usize::from_str_radix(size_str.trim(), 16).unwrap_or(0);
    if size == 0 {
      loop {
        let mut trailer = String::new();
        let bytes = reader.read_line(&mut trailer).await?;
        if bytes == 0 {
          return Err(io::Error::new(
            io::ErrorKind::UnexpectedEof,
            "unexpected EOF while reading chunk trailer",
          ));
        }
        writer.write_all(trailer.as_bytes()).await?;
        if trailer.trim_end_matches(['\r', '\n']).is_empty() {
          return Ok(());
        }
      }
    }

    relay_fixed(reader, writer, size as u64, capture, capture_limit).await?;
    let mut crlf = [0u8; 2];
    reader.read_exact(&mut crlf).await?;
    writer.write_all(&crlf).await?;
  }
}
