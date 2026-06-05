use thiserror::Error;

use bincode::Error as bincodeError;
use sled::Error as sledError;

#[derive(Debug, Error)]
pub enum StoreError {
  #[error("database error: {0}")]
  Db(#[from] sledError),

  #[error("serialization error: {0}")]
  Bincode(#[from] bincodeError),

  #[error("transaction not found: {0}")]
  NotFound(String),
}
