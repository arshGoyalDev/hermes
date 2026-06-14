use thiserror::Error;

#[derive(Debug, Error)]
pub enum ScriptError {
  #[error("Rhai evaluation error: {0}")]
  Eval(#[from] Box<rhai::EvalAltResult>),

  #[error("Rhai parse error: {0}")]
  Parse(#[from] rhai::ParseError),

  #[error("Script returned an invalid action map: {0}")]
  InvalidAction(String),

  #[error("I/O error reading script '{path}': {source}")]
  Io {
    path: String,
    #[source]
    source: std::io::Error,
  },
}
