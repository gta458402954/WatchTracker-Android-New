//! Explicit read-only execution boundary for I6.3. Not registered with S1,
//! lifecycle, or UI; the caller chooses the root and invokes a bounded round.
use std::collections::BTreeMap;

use async_trait::async_trait;
use rusqlite::Connection;

use super::canonical::Result;
use super::discovery_persistence::{load_read_state_v1, save_round_v1, DurableReadStateV1};
use super::remote_discovery::{
    create_discovery_state_v1, run_discovery_round_v1, validate_production_activation_body_v1,
    DirectoryListResultV1, DiscoveryBudgetsV1, DiscoveryExactGetResultV1, DiscoveryRemoteV1,
};
use super::webdav_adapter::{
    DirectoryListResultV1 as WebDavList, WebDavGetResultV1, WebDavS2AdapterV1, WebDavTransportV1,
};

#[async_trait]
pub trait ReadOnlyDiscoveryRemoteV1: Send {
    fn physical_root_id(&self) -> &str;
    async fn list_directory(&mut self, path: &str) -> DirectoryListResultV1;
    async fn get_exact(&mut self, path: &str) -> DiscoveryExactGetResultV1;
}

#[async_trait]
impl<T: WebDavTransportV1> ReadOnlyDiscoveryRemoteV1 for WebDavS2AdapterV1<T> {
    fn physical_root_id(&self) -> &str {
        self.physical_root_id()
    }
    async fn list_directory(&mut self, path: &str) -> DirectoryListResultV1 {
        match self.list_directory(path).await {
            WebDavList::Entries(entries) => DirectoryListResultV1::Entries(entries),
            WebDavList::Indeterminate => DirectoryListResultV1::Indeterminate,
            WebDavList::AuthOrCapabilityFailure => DirectoryListResultV1::AuthOrCapabilityFailure,
        }
    }
    async fn get_exact(&mut self, path: &str) -> DiscoveryExactGetResultV1 {
        match self.get_exact(path).await {
            WebDavGetResultV1::DefinitelyPresent(bytes) => {
                DiscoveryExactGetResultV1::DefinitelyPresent(bytes)
            }
            WebDavGetResultV1::DefinitelyAbsent => DiscoveryExactGetResultV1::DefinitelyAbsent,
            WebDavGetResultV1::Indeterminate => DiscoveryExactGetResultV1::Indeterminate,
            WebDavGetResultV1::AuthOrCapabilityFailure => {
                DiscoveryExactGetResultV1::AuthOrCapabilityFailure
            }
        }
    }
}

#[derive(Default)]
struct RoundTranscript {
    lists: BTreeMap<String, DirectoryListResultV1>,
    gets: BTreeMap<String, DiscoveryExactGetResultV1>,
    requested: Vec<(bool, String)>,
}
impl DiscoveryRemoteV1 for RoundTranscript {
    fn list_directory(&mut self, path: &str) -> DirectoryListResultV1 {
        self.lists.get(path).cloned().unwrap_or_else(|| {
            self.requested.push((true, path.into()));
            DirectoryListResultV1::Indeterminate
        })
    }
    fn get_exact(&mut self, path: &str) -> DiscoveryExactGetResultV1 {
        self.gets.get(path).cloned().unwrap_or_else(|| {
            self.requested.push((false, path.into()));
            DiscoveryExactGetResultV1::Indeterminate
        })
    }
}

/// Bridges the frozen synchronous core to async transport through one cached
/// round transcript. Each requested resource is read at most once per round.
/// Every planning pass starts from the SAME durable prior state; provisional
/// indeterminate placeholders never become persisted discovery facts.
/// No SQLite transaction is held over network I/O. The final CAS prevents
/// concurrent rounds from dropping observations or resetting fatal authority.
pub async fn discover_read_only_v1<R: ReadOnlyDiscoveryRemoteV1>(
    conn: &mut Connection,
    remote: &mut R,
    budgets: &DiscoveryBudgetsV1,
) -> Result<DurableReadStateV1> {
    let root = remote.physical_root_id().to_owned();
    let prior = load_read_state_v1(conn, &root)?;
    let expected = prior
        .as_ref()
        .map(|value| value.discovery.storage_generation);
    let state = prior
        .map(|value| value.discovery.state)
        .unwrap_or_else(create_discovery_state_v1);
    let mut transcript = RoundTranscript::default();
    loop {
        transcript.requested.clear();
        let next = run_discovery_round_v1(
            &state,
            &mut transcript,
            &mut validate_production_activation_body_v1,
            budgets,
        )?;
        if transcript.requested.is_empty() {
            return save_round_v1(conn, &root, expected, next);
        }
        // Resolve the first unknown call before considering later provisional
        // requests. This keeps the async transcript on the frozen call order,
        // including its fetch budget; no speculative GETs consume extra reads.
        if let Some((listing, path)) = transcript.requested.first().cloned() {
            if listing {
                if !transcript.lists.contains_key(&path) {
                    transcript
                        .lists
                        .insert(path.clone(), remote.list_directory(&path).await);
                }
            } else if !transcript.gets.contains_key(&path) {
                transcript
                    .gets
                    .insert(path.clone(), remote.get_exact(&path).await);
            }
        }
    }
}
