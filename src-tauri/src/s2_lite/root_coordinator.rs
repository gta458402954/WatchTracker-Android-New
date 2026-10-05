//! Process-local root keyed execution coordination.
//!
//! This is deliberately not protocol authority: SQLite root safety and its
//! transactions remain the durable inter-process boundary. The coordinator
//! only prevents same-process legacy and S2 mutations from interleaving while
//! one of them is in flight.

use std::collections::BTreeMap;
use std::sync::Arc;

use tokio::sync::{Mutex, OwnedMutexGuard};

use super::canonical::{ProtocolError, Result};

#[derive(Default)]
pub struct RootExecutionCoordinatorV1 {
    locks: Mutex<BTreeMap<String, Arc<Mutex<()>>>>,
}

impl RootExecutionCoordinatorV1 {
    async fn lock_for(&self, root_id: &str) -> Result<Arc<Mutex<()>>> {
        if root_id.is_empty() {
            return Err(ProtocolError("root_execution_lock"));
        }
        let mut locks = self.locks.lock().await;
        Ok(locks
            .entry(root_id.to_string())
            .or_insert_with(|| Arc::new(Mutex::new(())))
            .clone())
    }

    /// Acquires the root lock for an async command and keeps it through the
    /// admitted network operation.
    pub async fn acquire(&self, root_id: &str) -> Result<OwnedMutexGuard<()>> {
        Ok(self.lock_for(root_id).await?.lock_owned().await)
    }

    /// Synchronous lifecycle callers run outside the Tauri command runtime.
    /// They use the very same root lock as async commands.
    pub fn acquire_blocking(&self, root_id: &str) -> Result<OwnedMutexGuard<()>> {
        if root_id.is_empty() {
            return Err(ProtocolError("root_execution_lock"));
        }
        let lock = {
            let mut locks = self.locks.blocking_lock();
            locks
                .entry(root_id.to_string())
                .or_insert_with(|| Arc::new(Mutex::new(())))
                .clone()
        };
        Ok(lock.blocking_lock_owned())
    }
}
