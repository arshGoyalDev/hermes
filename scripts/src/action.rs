/// The decision a Rhai script makes for each intercepted request.
#[derive(Debug, Clone)]
pub enum ScriptAction {
  /// Forward the request unmodified (default when no script matches or the
  /// script returns `"passthrough"`).
  Passthrough,

  /// Replace the request headers before forwarding.
  /// The body and all other request fields are unchanged.
  ModifyHeaders(Vec<(String, String)>),

  /// Return a canned response to the client — don't forward to the upstream.
  MockResponse {
    status: u16,
    headers: Vec<(String, String)>,
    body: Vec<u8>,
  },

  /// Silently drop the request — return a 502 Bad Gateway to the client.
  Drop,
}

impl Default for ScriptAction {
  fn default() -> Self {
    Self::Passthrough
  }
}
