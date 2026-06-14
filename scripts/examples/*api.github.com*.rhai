// Strip Authorization headers from all requests to api.github.com.
//
// Drop this file in your scripts directory and it will automatically
// apply to every request whose URL contains "api.github.com".
//
// Filename: *api.github.com*.rhai  (the stem is used as a glob pattern)

fn handle(req) {
  let new_headers = [];
  for pair in req.headers {
    // pair[0] = header name, pair[1] = header value
    if pair[0].to_lower() != "authorization" {
      new_headers.push(pair);
    }
  }
  // Return a modify_headers action so the proxy uses new_headers instead.
  return #{ modify_headers: new_headers };
}
