//! Durable authority for mapping SyncTarget epochs to frozen S2 Lite roots.
//!
//! This module deliberately has no remote operation. It resolves only the
//! registry target, frozen WebDAV root identity, immutable local binding, and
//! root-scoped writer authority needed by a later lifecycle checkpoint.

use std::sync::Mutex;

use rusqlite::Connection;

use super::canonical::{ProtocolError, Result};
use super::durable_persistence::{DesktopRootStateV1, SqliteS2LiteStoreV1, TargetRootBindingV1};
use super::webdav_adapter::webdav_root_v1;

const TARGET_BINDING_FAILURE: ProtocolError = ProtocolError("S2_TARGET_ROOT_BINDING_FAILURE");

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TargetRootAuthorityV1 {
    pub binding: TargetRootBindingV1,
    pub writer_state: DesktopRootStateV1,
}

/// Immutable active target/root authority without ordinary writer allocation.
/// Migration admission uses this form so a future migration writer is not
/// pre-empted by a random desktop writer.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BoundTargetRootV1 {
    pub binding: TargetRootBindingV1,
}

fn active_target_snapshot_v1(
    conn: &Mutex<Connection>,
    target_id: &str,
    target_epoch: u64,
) -> Result<(String, String)> {
    let guard = conn.lock().map_err(|_| TARGET_BINDING_FAILURE)?;
    let registry = crate::sync_targets::registry(&guard).map_err(|_| TARGET_BINDING_FAILURE)?;
    let registry = registry.ok_or(TARGET_BINDING_FAILURE)?;
    if registry.active_target_id.as_deref() != Some(target_id)
        || registry.target_epoch != target_epoch
    {
        return Err(TARGET_BINDING_FAILURE);
    }
    let target = registry
        .targets
        .iter()
        .find(|target| target.id == target_id)
        .ok_or(TARGET_BINDING_FAILURE)?;
    Ok((target.normalized_url.clone(), target.username.clone()))
}

/// Resolves active target authority only through frozen `webdav_root_v1`, then
/// claims or verifies its immutable durable binding.  It intentionally does
/// not initialize desktop writer state.
pub fn resolve_active_target_root_binding_v1(
    conn: &Mutex<Connection>,
    target_id: &str,
    target_epoch: u64,
) -> Result<BoundTargetRootV1> {
    let (url, username) = active_target_snapshot_v1(conn, target_id, target_epoch)?;
    let root = webdav_root_v1(&url, &username).map_err(|_| TARGET_BINDING_FAILURE)?;
    let candidate = TargetRootBindingV1 {
        binding_version: 1,
        target_id: target_id.to_string(),
        target_epoch,
        canonical_url: root.canonical_url,
        normalized_account: root.normalized_account,
        physical_root_id: root.physical_root_id,
    };
    let mut store = SqliteS2LiteStoreV1::open(conn, &candidate.physical_root_id)?;
    let binding = store.bind_target_root_v1(&candidate)?;

    // The active registry may have changed while the durable transaction was
    // running. The immutable row is still useful historical evidence, but this
    // call must not hand a stale target to a new lifecycle execution.
    let _ = active_target_snapshot_v1(conn, target_id, target_epoch)?;
    Ok(BoundTargetRootV1 { binding })
}

/// Established normal S2 lifecycle entry point.  It preserves writer-bearing
/// behavior by resolving the binding first and only then initializing ordinary
/// desktop writer authority.
pub fn resolve_active_target_root_authority_v1(
    conn: &Mutex<Connection>,
    target_id: &str,
    target_epoch: u64,
) -> Result<TargetRootAuthorityV1> {
    let bound = resolve_active_target_root_binding_v1(conn, target_id, target_epoch)?;
    let mut store = SqliteS2LiteStoreV1::open(conn, &bound.binding.physical_root_id)?;
    let writer_state = store.initialize_desktop_writer_v1()?;
    Ok(TargetRootAuthorityV1 {
        binding: bound.binding,
        writer_state,
    })
}

/// Historical recovery lookup. It never checks the active target and never
/// derives a replacement root, so a missing future credential cannot retarget
/// frozen work.
pub fn load_historical_target_root_binding_v1(
    conn: &Mutex<Connection>,
    target_id: &str,
    target_epoch: u64,
) -> Result<Option<TargetRootBindingV1>> {
    SqliteS2LiteStoreV1::load_target_root_binding_v1(conn, target_id, target_epoch)
}
