// Mock every GET request to httpbin.org/status/* with a 418 I'm a teapot.
//
// Filename: *httpbin.org/status/*.rhai

fn handle(req) {
  if req.method != "GET" {
    return "passthrough";
  }
  return #{
    mock: true,
    status: 418,
    headers: [
      ["Content-Type", "text/plain"],
      ["X-Hermes-Mocked", "true"],
    ],
    body: "I'm a teapot (mocked by Hermes)",
  };
}
