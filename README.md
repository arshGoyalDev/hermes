# Hermes

A programmable HTTP/S traffic inspector & replay tool — a self-hosted Charles Proxy / Postman hybrid with a scriptable pipeline and a terminal UI.

Hermes sits as a local MITM proxy, intercepts HTTP and HTTPS traffic, lets you inspect it live, persist sessions, write transform scripts, and replay requests — all without leaving the terminal.

---

## Features

- **MITM Proxy** — Intercepts HTTP and HTTPS via dynamic certificate injection
- **Live TUI** — Real-time traffic viewer (Ratatui): scrollable list + full detail panel
- **Session Storage** — Persists captures to an embedded `sled` database (no external deps)
- **Request Replay** — Re-send any captured request; diff original vs replayed response
- **Rhai Scripting** — Transform requests per-URL: strip headers, inject values, mock responses, or drop requests
- **Binary Safety** — Binary bodies (images, gzip, PDFs…) are detected and never rendered as text

---

## Architecture

Cargo workspace of five crates:

```
hermes/
├── cli/        # Entry point — clap CLI, wires all crates together
├── proxy/      # Core MITM proxy (raw TCP + rustls, no hyper)
├── tui/        # Ratatui interface (two-panel layout, async event loop)
├── scripts/    # Rhai scripting engine
└── store/      # sled-backed session persistence
```

**Data flow:**
```
Browser / curl
      │  (proxy at 127.0.0.1:8080)
      ▼
   proxy  ← decrypts TLS, runs Rhai scripts
      │  mpsc channel
      ▼
  relay task
   ├──→  tui   (Ratatui, live view)
   └──→  store (sled, persisted)
```

**Stack:** `tokio` · `rustls` + `rcgen` · `ratatui` + `crossterm` · `rhai` · `sled` + `bincode` · `clap`

---

## Building & Quick Start

```bash
# Build
cargo build --release

# Run (listens on 127.0.0.1:8080 by default)
cargo run --release -- run
```

On first run a root CA is generated (`hermes-ca.crt`). Install it in your trust store to intercept HTTPS:

```bash
# Linux (NSS)
certutil -d sql:$HOME/.pki/nssdb -A -t "CT,C,C" -n "Hermes Proxy CA" -i hermes-ca.crt

# macOS
sudo security add-trusted-cert -d -r trustRoot \
  -k /Library/Keychains/System.keychain hermes-ca.crt
```

Then send traffic through the proxy:

```bash
curl -x http://localhost:8080 --cacert hermes-ca.crt https://httpbin.org/get
```

---

## CLI Reference

```
hermes run     [--bind 127.0.0.1:8080] [--db .hermes-sessions] [--scripts .hermes-scripts]
hermes list    [--db .hermes-sessions]
hermes replay  <UUID>  [--db .hermes-sessions] [--diff]
```

`replay` re-sends the original request and prints the new response. With `--diff` (default) it shows a line-by-line body diff.

---

## TUI

| Key | Action |
|---|---|
| `↑` / `k` · `↓` / `j` | Navigate list / scroll detail |
| `PgUp` / `PgDn` | Jump 10 rows |
| `Tab` | Toggle focus between panels |
| `q` | Quit |

Method colors: `GET` green · `POST` blue · `PUT/PATCH` yellow · `DELETE` red.  
Status colors: `2xx` green · `3xx` blue · `4xx` yellow · `5xx` red.

---

## Rhai Scripting

Place `.rhai` files in `.hermes-scripts/`. The **filename stem** is used as a URL glob pattern:

| Filename | Matches |
|---|---|
| `*domain*.rhai` | Any URL containing `domain` |
| `*.rhai` | Every request |

Each script exports `fn handle(req)` receiving `req.method`, `req.url`, `req.headers`.

**Return values:**

| Value | Effect |
|---|---|
| `"passthrough"` | Forward unchanged |
| `"drop"` | Drop; proxy returns 502 |
| `#{ modify_headers: [[name, val], …] }` | Replace request headers |
| `#{ mock: true, status: N, body: "…", headers: […] }` | Return synthetic response |

**Example — strip `Authorization` from your API Requests:**

```rhai
// *api.domain*.rhai
fn handle(req) {
  let new_headers = [];
  for pair in req.headers {
    if pair[0].to_lower() != "authorization" { new_headers.push(pair); }
  }
  return #{ modify_headers: new_headers };
}
```

**Example — mock httpbin status endpoints with 418:**

```rhai
// *httpbin.org/status/*.rhai
fn handle(req) {
  if req.method != "GET" { return "passthrough"; }
  return #{ mock: true, status: 418, body: "I'm a teapot (mocked by Hermes)" };
}
```

Scripts run via `spawn_blocking` so they never block the async runtime. The engine enforces a 100k-operation limit.

---

## Session Storage

Transactions are stored in `sled` under `.hermes-sessions/`, serialized with `bincode`. Request and response bodies are captured up to **16 KiB**; larger payloads are truncated at the proxy.

```
Transaction { id, timestamp, request { method, url, headers, body }, response?, duration_ms? }
```

---

## How TLS Interception Works

1. **CA bootstrap** — Generates `hermes-ca.crt` / `hermes-ca.key` on first run (reused on subsequent runs).
2. **CONNECT** — On `CONNECT host:443`, Hermes replies `200 Connection Established` and intercepts the stream.
3. **Server-side TLS** — Dynamically issues a leaf cert for the target hostname, signed by the Hermes CA, and does a TLS handshake with the client. Certs are cached in memory per hostname.
4. **Client-side TLS** — Opens a real TLS connection to the upstream using Mozilla's root store (`webpki-roots`).
5. **Inspection** — Reads decrypted HTTP, runs the script pipeline, captures request/response (capped at 16 KiB), emits the `Transaction`, and returns the response.

---

## Logs

Stderr is redirected to `<db>.log` (e.g. `.hermes-sessions.log`) before the TUI starts, preventing proxy errors from corrupting the display:

```bash
tail -f .hermes-sessions.log
```