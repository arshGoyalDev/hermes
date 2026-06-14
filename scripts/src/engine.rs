use std::path::{Path, PathBuf};
use std::sync::Arc;

use rhai::{Dynamic, Engine, Map, Scope};

use crate::action::ScriptAction;
use crate::error::ScriptError;

// ── Per-script record ─────────────────────────────────────────────────────────

/// A compiled script paired with the URL glob pattern it should run on.
struct ScriptEntry {
  /// Glob pattern against the full URL (e.g. `"*://api.github.com/*"`).
  pattern: glob::Pattern,
  /// Pre-compiled Rhai AST — cheap to re-evaluate.
  ast: rhai::AST,
  /// Human-readable path, for error messages.
  path: PathBuf,
}

// ── Public engine ─────────────────────────────────────────────────────────────

/// Loads `.rhai` scripts from a directory and runs them in the proxy pipeline.
///
/// # Script conventions
///
/// Each `.rhai` file must export a top-level function:
///
/// ```rhai
/// fn handle(req) { ... }
/// ```
///
/// `req` is a Rhai `Map` with the following keys:
/// - `method`  : String  — e.g. `"GET"`
/// - `url`     : String  — full URL including scheme
/// - `headers` : Array of `[name, value]` arrays
///
/// The function must return one of:
/// - `"passthrough"` — forward unchanged (default)
/// - `"drop"`        — silently drop, proxy returns 502
/// - A `Map` with key `"modify_headers"` → Array of `[name, value]` arrays
/// - A `Map` with keys `"mock"`, `"status"` (Int), `"body"` (String),
///   and optionally `"headers"` (Array of `[name, value]` arrays)
///
/// # File naming
///
/// The file name (without extension) doubles as the URL pattern:
/// ```
/// scripts/
///   *api.github.com*.rhai   →  matches any URL containing api.github.com
///   *.rhai                  →  matches every request
/// ```
/// Pattern syntax follows standard glob rules (`*`, `?`, `[…]`).
pub struct ScriptEngine {
  rhai: Arc<Engine>,
  scripts: Vec<ScriptEntry>,
}

impl ScriptEngine {
  /// Create an engine with no scripts loaded.
  pub fn new() -> Self {
    let mut engine = Engine::new();
    // Limit iterations to prevent runaway scripts from blocking proxy tasks.
    engine.set_max_operations(100_000);
    engine.set_max_expr_depths(64, 32);

    Self {
      rhai: Arc::new(engine),
      scripts: Vec::new(),
    }
  }

  /// Load all `.rhai` files from `dir`.
  ///
  /// Each file name (without extension) is used as the URL glob pattern.
  /// Files that fail to parse are skipped with a warning printed to stderr.
  pub fn load_dir(mut self, dir: impl AsRef<Path>) -> Self {
    let dir = dir.as_ref();
    if !dir.exists() {
      return self;
    }

    let Ok(entries) = std::fs::read_dir(dir) else {
      eprintln!("[scripts] cannot read dir {}: permission denied", dir.display());
      return self;
    };

    for entry in entries.flatten() {
      let path = entry.path();
      if path.extension().and_then(|e| e.to_str()) != Some("rhai") {
        continue;
      }

      let stem = path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("*")
        .to_string();

      let pattern = match glob::Pattern::new(&stem) {
        Ok(p) => p,
        Err(e) => {
          eprintln!("[scripts] bad pattern in filename '{}': {}", stem, e);
          continue;
        }
      };

      let source = match std::fs::read_to_string(&path) {
        Ok(s) => s,
        Err(e) => {
          eprintln!("[scripts] cannot read {}: {}", path.display(), e);
          continue;
        }
      };

      match self.rhai.compile(&source) {
        Ok(ast) => {
          eprintln!("[scripts] loaded: {} (pattern: {})", path.display(), stem);
          self.scripts.push(ScriptEntry { pattern, ast, path });
        }
        Err(e) => {
          eprintln!("[scripts] parse error in {}: {}", path.display(), e);
        }
      }
    }

    self
  }

  /// Run all matching scripts for `(method, url, headers)`.
  ///
  /// Scripts are run in load order; the first one that returns a non-passthrough
  /// action wins.  If none match (or all return passthrough), returns
  /// `ScriptAction::Passthrough`.
  pub fn run(
    &self,
    method: &str,
    url: &str,
    headers: &[(String, String)],
  ) -> ScriptAction {
    for entry in &self.scripts {
      // Match against the URL (scheme stripped for brevity)
      let url_for_match = url
        .strip_prefix("https://")
        .or_else(|| url.strip_prefix("http://"))
        .unwrap_or(url);

      if !entry.pattern.matches(url_for_match) {
        continue;
      }

      let req = build_req_map(method, url, headers);
      let mut scope = Scope::new();

      let result: Dynamic =
        match self.rhai.call_fn(&mut scope, &entry.ast, "handle", (req,)) {
          Ok(v) => v,
          Err(e) => {
            eprintln!(
              "[scripts] error in {}: {}",
              entry.path.display(),
              e
            );
            continue;
          }
        };

      match parse_action(result) {
        Ok(ScriptAction::Passthrough) => continue, // let next script try
        Ok(action) => return action,
        Err(e) => {
          eprintln!(
            "[scripts] invalid action from {}: {}",
            entry.path.display(),
            e
          );
        }
      }
    }

    ScriptAction::Passthrough
  }

  /// Return `true` if no scripts are loaded (skip pipeline overhead entirely).
  pub fn is_empty(&self) -> bool {
    self.scripts.is_empty()
  }
}

impl Default for ScriptEngine {
  fn default() -> Self {
    Self::new()
  }
}

// ── Helpers ───────────────────────────────────────────────────────────────────

/// Build the Rhai `Map` that is passed to `handle(req)`.
fn build_req_map(method: &str, url: &str, headers: &[(String, String)]) -> Dynamic {
  let mut map = Map::new();
  map.insert("method".into(), Dynamic::from(method.to_string()));
  map.insert("url".into(), Dynamic::from(url.to_string()));

  let hdr_array: rhai::Array = headers
    .iter()
    .map(|(n, v)| {
      let pair: rhai::Array = vec![
        Dynamic::from(n.clone()),
        Dynamic::from(v.clone()),
      ];
      Dynamic::from(pair)
    })
    .collect();
  map.insert("headers".into(), Dynamic::from(hdr_array));

  Dynamic::from(map)
}

/// Convert the `Dynamic` returned by the script into a `ScriptAction`.
fn parse_action(value: Dynamic) -> Result<ScriptAction, ScriptError> {
  // ── "passthrough" | "drop" strings ────────────────────────────────────────
  if let Some(s) = value.clone().try_cast::<String>() {
    return match s.to_lowercase().as_str() {
      "passthrough" => Ok(ScriptAction::Passthrough),
      "drop" => Ok(ScriptAction::Drop),
      other => Err(ScriptError::InvalidAction(format!(
        "unknown string action '{other}'"
      ))),
    };
  }

  // ── Map — either modify_headers or mock ───────────────────────────────────
  if let Some(map) = value.clone().try_cast::<Map>() {
    // modify_headers
    if let Some(new_hdrs) = map.get("modify_headers") {
      let headers = dynamic_to_headers(new_hdrs.clone())?;
      return Ok(ScriptAction::ModifyHeaders(headers));
    }

    // mock response
    if map.contains_key("mock") {
      let status = map
        .get("status")
        .and_then(|v| v.clone().try_cast::<i64>())
        .unwrap_or(200) as u16;

      let body = map
        .get("body")
        .and_then(|v| v.clone().try_cast::<String>())
        .unwrap_or_default()
        .into_bytes();

      let headers = map
        .get("headers")
        .map(|v| dynamic_to_headers(v.clone()))
        .transpose()?
        .unwrap_or_default();

      return Ok(ScriptAction::MockResponse {
        status,
        headers,
        body,
      });
    }

    return Err(ScriptError::InvalidAction(
      "map has neither 'modify_headers' nor 'mock' key".into(),
    ));
  }

  Err(ScriptError::InvalidAction(format!(
    "expected String or Map, got: {:?}",
    value.type_name()
  )))
}

/// Convert a Rhai `Array` of `[name, value]` pairs into `Vec<(String, String)>`.
fn dynamic_to_headers(value: Dynamic) -> Result<Vec<(String, String)>, ScriptError> {
  let arr = value
    .try_cast::<rhai::Array>()
    .ok_or_else(|| ScriptError::InvalidAction("headers must be an array".into()))?;

  arr
    .into_iter()
    .map(|item| {
      let pair = item
        .try_cast::<rhai::Array>()
        .ok_or_else(|| ScriptError::InvalidAction("each header must be a [name, value] array".into()))?;

      let name = pair
        .first()
        .and_then(|v| v.clone().try_cast::<String>())
        .ok_or_else(|| ScriptError::InvalidAction("header name must be a string".into()))?;

      let value = pair
        .get(1)
        .and_then(|v| v.clone().try_cast::<String>())
        .ok_or_else(|| ScriptError::InvalidAction("header value must be a string".into()))?;

      Ok((name, value))
    })
    .collect()
}
