use std::path::Path;

use sled::Db;
use uuid::Uuid;

use crate::error::StoreError;
use crate::transaction::Transaction;

use bincode::{deserialize, serialize};

// A thin wrapper around `sled` that stores `Transaction` records.
// Each transaction is serialized with `bincode` and keyed by its UUID bytes.
pub struct TransactionStore {
  db: Db,
}

impl TransactionStore {
  // Open (or create) the sled database at the given path.
  pub fn open(path: impl AsRef<Path>) -> Result<Self, StoreError> {
    let db = sled::open(path)?;
    Ok(Self { db })
  }

  // Persist a transaction, overwriting any previous record with the same id.
  pub fn save(&self, tx: &Transaction) -> Result<(), StoreError> {
    let key = tx.id.as_bytes().to_vec();
    let value = serialize(tx)?;
    self.db.insert(key, value)?;
    Ok(())
  }

  // Retrieve a single transaction by id.
  pub fn get(&self, id: Uuid) -> Result<Option<Transaction>, StoreError> {
    let key = id.as_bytes().to_vec();
    match self.db.get(&key)? {
      Some(bytes) => {
        let tx: Transaction = deserialize(&bytes)?;
        Ok(Some(tx))
      }
      None => Ok(None),
    }
  }

  // Return all transactions, ordered by their insertion order in sled
  // (which is lexicographic on UUID bytes — effectively insertion order
  // for v4 UUIDs within a session).
  pub fn all(&self) -> Result<Vec<Transaction>, StoreError> {
    let mut result = Vec::new();
    for item in self.db.iter() {
      let (_, value) = item?;
      let tx: Transaction = deserialize(&value)?;
      result.push(tx);
    }
    // Sort by timestamp so the list is chronological.
    result.sort_by_key(|t| t.timestamp);
    Ok(result)
  }

  // Delete a transaction by id.
  pub fn remove(&self, id: Uuid) -> Result<bool, StoreError> {
    let key = id.as_bytes().to_vec();
    let existed = self.db.remove(key)?.is_some();
    Ok(existed)
  }

  // Flush pending writes to disk.
  pub fn flush(&self) -> Result<(), StoreError> {
    self.db.flush()?;
    Ok(())
  }
}
