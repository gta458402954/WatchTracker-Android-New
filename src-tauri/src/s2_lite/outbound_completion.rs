//! Generation-safe local completion for an already receipted outbound batch.
//!
//! This boundary has no remote collaborator. It can only consume the durable
//! receipt written by publication recovery, then atomically acknowledge the
//! exact captured staging tokens and advance the local writer head.

use std::sync::Mutex;

use rusqlite::Connection;

use super::canonical::Result;
use super::durable_persistence::{OutboundCompletionResultV1, SqliteS2LiteStoreV1};

pub fn complete_verified_outbound_batch_v1(
    conn: &Mutex<Connection>,
    physical_root_id: &str,
) -> Result<OutboundCompletionResultV1> {
    SqliteS2LiteStoreV1::open(conn, physical_root_id)?.complete_verified_outbound_batch()
}
