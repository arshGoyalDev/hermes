use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Captured HTTP request data.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RequestData {
  pub method: String,
  pub url: String,
  pub headers: Vec<(String, String)>,
  /// Body bytes, capped at the proxy's capture limit.
  pub body: Vec<u8>,
}

/// Captured HTTP response data.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResponseData {
  pub status: u16,
  pub headers: Vec<(String, String)>,
  /// Body bytes, capped at the proxy's capture limit.
  pub body: Vec<u8>,
}

/// A complete request ↔ response exchange captured by the proxy.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Transaction {
  /// Globally unique identifier — used as the sled key.
  pub id: Uuid,
  /// Wall-clock time the request was received.
  pub timestamp: DateTime<Utc>,
  pub request: RequestData,
  /// `None` while the response is still in flight.
  pub response: Option<ResponseData>,
  /// Round-trip duration in milliseconds.
  pub duration_ms: Option<u64>,
}

impl Transaction {
  /// Create a new `Transaction` with no response yet.
  pub fn new(request: RequestData) -> Self {
    Self {
      id: Uuid::new_v4(),
      timestamp: Utc::now(),
      request,
      response: None,
      duration_ms: None,
    }
  }

  /// Convenience: extract the host from the URL for display purposes.
  pub fn host(&self) -> &str {
    let url = &self.request.url;
    // Try to parse out the authority portion from http(s)://host/...
    let rest = url
      .strip_prefix("https://")
      .or_else(|| url.strip_prefix("http://"))
      .unwrap_or(url);
    rest.split('/').next().unwrap_or(rest)
  }

  /// Convenience: extract the path from the URL for display purposes.
  pub fn path(&self) -> &str {
    let url = &self.request.url;
    let rest = url
      .strip_prefix("https://")
      .or_else(|| url.strip_prefix("http://"))
      .unwrap_or(url);
    rest.find('/').map(|i| &rest[i..]).unwrap_or("/")
  }
}
