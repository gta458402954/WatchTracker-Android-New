use async_trait::async_trait;
use base64::Engine;
use reqwest::{Method, Url};
use rusqlite::{params, Connection};
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::time::Duration;

use super::causal::decode_frozen_wire_commit_v1;
use super::discovery_persistence::*;
use super::discovery_runtime::discover_read_only_v1;
use super::immutable_publish::build_commit_remote_path_v1;
use super::local_authority::{
    capture_staged_descriptor, load_staged_descriptors, StagingAnchorStateV1,
};
use super::materialized_projection::{
    MaterializedProjectionStatusV1, OrdinaryCausalBaseResolutionV1,
};
use super::ordinary_mutation::OrdinaryCausalBaseV1;
use super::remote_discovery::*;
use super::types::CommitRef;
use super::webdav_adapter::{
    webdav_root_v1, WebDavResponseV1, WebDavRootV1, WebDavS2AdapterV1, WebDavS2ConfigV1,
    WebDavTransportV1,
};

const W1: &str = "10000000-0000-4000-8000-000000000001";
const W2: &str = "10000000-0000-4000-8000-000000000002";
fn root() -> WebDavRootV1 {
    webdav_root_v1("https://example.test/dav", "alice").unwrap()
}
fn database() -> Connection {
    let c = Connection::open_in_memory().unwrap();
    crate::db::setup_db(&c).unwrap();
    c
}
fn raw_template() -> Value {
    let fixture: Value = serde_json::from_str(include_str!(
        "../../../contracts/s2-lite/v1/raw-wire-json-v1.json"
    ))
    .unwrap();
    let case = fixture["cases"]
        .as_array()
        .unwrap()
        .iter()
        .find(|case| case["name"] == "canonical-mutation-resolves-absent")
        .unwrap();
    serde_json::from_slice(
        &base64::engine::general_purpose::STANDARD
            .decode(case["utf8Base64"].as_str().unwrap())
            .unwrap(),
    )
    .unwrap()
}
#[derive(Clone)]
struct Object {
    bytes: Vec<u8>,
    path: String,
    reference: CommitRef,
}
fn object(mut wire: Value) -> Object {
    wire.as_object_mut().unwrap().remove("contentHash");
    let bytes = serde_json::to_vec(&wire).unwrap();
    let reference = decode_frozen_wire_commit_v1(&bytes).unwrap().commit_ref();
    let path = build_commit_remote_path_v1(&reference).unwrap();
    Object {
        bytes,
        path,
        reference,
    }
}
fn commit(
    writer: &str,
    seq: u64,
    salt: u64,
    entity: &str,
    name: &str,
    previous: Option<CommitRef>,
) -> Object {
    let mut wire = raw_template();
    wire["writerId"] = json!(writer);
    wire["writerSeq"] = json!(seq.to_string());
    wire["commitId"] = json!(format!("20000000-0000-4000-8000-{salt:012}"));
    wire["previousWriterCommit"] = serde_json::to_value(&previous).unwrap();
    wire["basisClock"] = serde_json::to_value(previous.iter().collect::<Vec<_>>()).unwrap();
    wire["mutations"][0]["localMutationId"] = json!(format!("40000000-0000-4000-8000-{salt:012}"));
    wire["mutations"][0]["entityKey"] = json!(["collection", entity]);
    wire["mutations"][0]["value"]["id"] = json!(entity);
    wire["mutations"][0]["value"]["name"] = json!(name);
    wire["mutations"][0]["value"]["normalizedName"] = json!(name.to_lowercase());
    object(wire)
}
#[derive(Default)]
struct Fake {
    lists: BTreeMap<String, Vec<String>>,
    objects: BTreeMap<String, Vec<u8>>,
    calls: Vec<(Method, String)>,
    fail_lists: bool,
}
#[async_trait]
impl WebDavTransportV1 for Fake {
    async fn request(
        &mut self,
        method: Method,
        url: Url,
        _headers: Vec<(String, String)>,
        _body: Option<Vec<u8>>,
    ) -> std::result::Result<WebDavResponseV1, ()> {
        let path = url
            .path()
            .strip_prefix("/dav/")
            .unwrap()
            .trim_end_matches('/')
            .to_owned();
        self.calls.push((method.clone(), path.clone()));
        if method == Method::GET {
            return Ok(WebDavResponseV1 {
                status: if self.objects.contains_key(&path) {
                    200
                } else {
                    404
                },
                body: self.objects.get(&path).cloned().unwrap_or_default(),
            });
        }
        assert_eq!(
            method.as_str(),
            "PROPFIND",
            "read-only boundary must never provision or PUT"
        );
        if self.fail_lists {
            return Err(());
        }
        let entries = self.lists.get(&path).cloned().unwrap_or_default();
        let body = format!(
            "<d:multistatus xmlns:d='DAV:'>{}</d:multistatus>",
            entries
                .iter()
                .map(|entry| format!("<d:response><d:href>/dav/{entry}</d:href></d:response>"))
                .collect::<String>()
        )
        .into_bytes();
        Ok(WebDavResponseV1 { status: 207, body })
    }
}
fn fake_remote(objects: &[Object]) -> Fake {
    let mut fake = Fake::default();
    for item in objects {
        fake.objects.insert(item.path.clone(), item.bytes.clone());
        let parts = item.path.split('/').collect::<Vec<_>>();
        for (dir, child) in [
            ("writers".to_string(), format!("writers/{}", parts[1])),
            (
                format!("writers/{}/segments", parts[1]),
                format!("writers/{}/segments/{}", parts[1], parts[3]),
            ),
            (
                format!("writers/{}/segments/{}", parts[1], parts[3]),
                item.path.clone(),
            ),
        ] {
            let entries = fake.lists.entry(dir).or_default();
            if !entries.contains(&child) {
                entries.push(child);
            }
        }
    }
    fake
}
fn adapter(fake: Fake) -> WebDavS2AdapterV1<Fake> {
    WebDavS2AdapterV1::new(
        WebDavS2ConfigV1 {
            root: root(),
            username: "alice".into(),
            password: "secret".into(),
            proxy: None,
            timeout: Duration::from_secs(1),
        },
        fake,
    )
    .unwrap()
}
fn remote(objects: &[Object]) -> WebDavS2AdapterV1<Fake> {
    adapter(fake_remote(objects))
}
fn run(c: &mut Connection, remote: &mut WebDavS2AdapterV1<Fake>) -> DurableReadStateV1 {
    tauri::async_runtime::block_on(discover_read_only_v1(
        c,
        remote,
        &DiscoveryBudgetsV1::default(),
    ))
    .unwrap()
}
fn disk() -> (std::path::PathBuf, Connection) {
    let path = std::env::temp_dir().join(format!("i63-{}.sqlite", uuid::Uuid::new_v4()));
    let c = Connection::open(&path).unwrap();
    crate::db::setup_db(&c).unwrap();
    (path, c)
}
fn reopen(path: &std::path::Path) -> Connection {
    let c = Connection::open(path).unwrap();
    crate::db::setup_db(&c).unwrap();
    c
}

#[test]
fn observations_exact_bytes_and_live_projection_survive_restart() {
    let a = commit(W1, 1, 1, "c1", "One", None);
    let (path, mut c) = disk();
    let first = run(&mut c, &mut remote(&[a]));
    assert_eq!(first.discovery.state.verified_objects.len(), 1);
    assert_eq!(
        first.projection.state.status,
        MaterializedProjectionStatusV1::Complete
    );
    let anchor =
        resolve_verified_anchor_v1(&c, &root().physical_root_id, &json!(["collection", "c1"]))
            .unwrap();
    assert!(
        matches!(&anchor,VerifiedAnchorResolutionV1::Verified(OrdinaryCausalBaseResolutionV1::Ready{causal_base:OrdinaryCausalBaseV1::Live(value),base_frontier,..}) if value["name"]=="One" && base_frontier.len()==1)
    );
    drop(c);
    let c = reopen(&path);
    let loaded = load_read_state_v1(&c, &root().physical_root_id)
        .unwrap()
        .unwrap();
    assert_eq!(loaded.projection, first.projection);
    assert_eq!(loaded.discovery.state, first.discovery.state);
    assert_eq!(
        resolve_verified_anchor_v1(&c, &root().physical_root_id, &json!(["collection", "c1"]))
            .unwrap(),
        anchor
    );
    drop(c);
    std::fs::remove_file(path).unwrap();
}
#[test]
fn remote_omission_never_removes_retained_evidence() {
    let mut c = database();
    let a = commit(W1, 1, 1, "c1", "One", None);
    let first = run(&mut c, &mut remote(&[a]));
    let second = run(&mut c, &mut remote(&[]));
    assert_eq!(
        second.discovery.state.verified_objects,
        first.discovery.state.verified_objects
    );
    assert_eq!(
        second.projection.state.entities,
        first.projection.state.entities
    );
    assert_eq!(
        second.discovery.state.observed_candidates,
        first.discovery.state.observed_candidates
    );
}
#[test]
fn empty_or_failed_discovery_never_proves_unobserved_absence() {
    let mut c = database();
    let key = json!(["collection", "unknown"]);
    assert_eq!(
        resolve_verified_anchor_v1(&c, &root().physical_root_id, &key).unwrap(),
        VerifiedAnchorResolutionV1::Unavailable
    );
    run(&mut c, &mut remote(&[]));
    let mut failed = Fake {
        fail_lists: true,
        ..Fake::default()
    };
    let round = run(&mut c, &mut adapter(std::mem::take(&mut failed)));
    assert!(round.discovery.state.last_round_indeterminate);
    assert_eq!(
        resolve_verified_anchor_v1(&c, &root().physical_root_id, &key).unwrap(),
        VerifiedAnchorResolutionV1::Unavailable
    );
}
#[test]
fn pending_dependency_survives_restart_and_late_exact_get_unblocks_replay() {
    let a = commit(W1, 1, 1, "c1", "One", None);
    let b = commit(W1, 2, 2, "c2", "Two", Some(a.reference.clone()));
    let (path, mut c) = disk();
    let pending = run(&mut c, &mut remote(&[b]));
    assert!(matches!(
        pending.projection.state.status,
        MaterializedProjectionStatusV1::PendingDependencies { .. }
    ));
    assert!(!pending.discovery.state.targeted_queue.is_empty());
    drop(c);
    let mut c = reopen(&path);
    let loaded = load_read_state_v1(&c, &root().physical_root_id)
        .unwrap()
        .unwrap();
    assert_eq!(loaded.projection, pending.projection);
    let complete = run(&mut c, &mut remote(&[a]));
    assert_eq!(
        complete.projection.state.status,
        MaterializedProjectionStatusV1::Complete
    );
    assert_eq!(complete.projection.state.entities.len(), 2);
    drop(c);
    std::fs::remove_file(path).unwrap();
}
#[test]
fn writer_sequence_fork_is_durable_and_retains_forensic_alternatives() {
    let a = commit(W1, 1, 1, "c1", "One", None);
    let b = commit(W1, 1, 2, "c1", "Fork", None);
    let (path, mut c) = disk();
    let frozen = run(&mut c, &mut remote(&[a, b]));
    assert!(frozen.fatal_codes.iter().any(|code| code == "WRITER_FORK"));
    assert_eq!(frozen.discovery.state.verified_objects.len(), 2);
    drop(c);
    let mut c = reopen(&path);
    let still = run(&mut c, &mut remote(&[]));
    assert_eq!(still.fatal_codes, frozen.fatal_codes);
    assert_eq!(still.discovery.state.verified_objects.len(), 2);
    assert!(matches!(
        resolve_verified_anchor_v1(&c, &root().physical_root_id, &json!(["collection", "c1"]))
            .unwrap(),
        VerifiedAnchorResolutionV1::Verified(OrdinaryCausalBaseResolutionV1::Fatal)
    ));
    drop(c);
    std::fs::remove_file(path).unwrap();
}
#[test]
fn cursor_survives_restart_and_late_historical_fork_freezes() {
    let a = commit(W1, 1, 1, "c1", "One", None);
    let b = commit(W1, 1, 2, "c1", "Fork", None);
    let (path, mut c) = disk();
    let mut fake = fake_remote(&[a]);
    fake.lists
        .entry(format!("writers/{W1}/segments"))
        .or_default()
        .push(format!("writers/{W1}/segments/00000000000004"));
    let mut initial = adapter(fake);
    let first = run(&mut c, &mut initial);
    assert!(!first.discovery.state.historical_closed_segments.is_empty());
    drop(c);
    let mut c = reopen(&path);
    assert_eq!(
        load_read_state_v1(&c, &root().physical_root_id)
            .unwrap()
            .unwrap()
            .discovery
            .state
            .historical_audit_cursor,
        first.discovery.state.historical_audit_cursor
    );
    let mut late = remote(&[b]);
    let mut found = false;
    for _ in 0..8 {
        let next = run(&mut c, &mut late);
        if next.fatal_codes.iter().any(|code| code == "WRITER_FORK") {
            found = true;
            break;
        }
    }
    assert!(found);
    drop(c);
    let c = reopen(&path);
    assert!(load_read_state_v1(&c, &root().physical_root_id)
        .unwrap()
        .unwrap()
        .fatal_codes
        .iter()
        .any(|code| code == "WRITER_FORK"));
    drop(c);
    std::fs::remove_file(path).unwrap();
}
#[test]
fn concurrent_conflict_persists_and_is_not_overwritten_by_more_discovery() {
    let a = commit(W1, 1, 1, "c1", "One", None);
    let b = commit(W2, 1, 2, "c1", "Two", None);
    let (path, mut c) = disk();
    let first = run(&mut c, &mut remote(&[a, b]));
    assert!(first.projection.state.entities[0].conflict);
    let conflict = first.projection.replay["materialized"].clone();
    drop(c);
    let mut c = reopen(&path);
    let second = run(&mut c, &mut remote(&[]));
    assert_eq!(second.projection.replay["materialized"], conflict);
    assert!(matches!(
        resolve_verified_anchor_v1(&c, &root().physical_root_id, &json!(["collection", "c1"]))
            .unwrap(),
        VerifiedAnchorResolutionV1::Verified(OrdinaryCausalBaseResolutionV1::ConflictBlocked)
    ));
    drop(c);
    std::fs::remove_file(path).unwrap();
}
#[test]
fn deterministic_replay_is_independent_of_listing_order() {
    let a = commit(W1, 1, 1, "c1", "One", None);
    let b = commit(W2, 1, 2, "c1", "Two", None);
    let mut x = database();
    let mut y = database();
    let first = run(&mut x, &mut remote(&[a.clone(), b.clone()]));
    let second = run(&mut y, &mut remote(&[b, a]));
    assert_eq!(first.projection, second.projection);
}
#[test]
fn later_projection_does_not_rewrite_first_local_basis_or_mutation_id() {
    let mut c = database();
    let first = capture_staged_descriptor(
        &c,
        "collection",
        "c1",
        None,
        Some(json!({"id":"c1","name":"local"})),
        1,
    )
    .unwrap();
    let a = commit(W1, 1, 1, "c1", "Remote", None);
    run(&mut c, &mut remote(&[a]));
    let second = load_staged_descriptors(&c).unwrap().pop().unwrap();
    assert_eq!(first, second);
    assert_eq!(second.causal_anchor, StagingAnchorStateV1::Unavailable);
}
#[test]
fn complete_verified_deletion_is_not_confused_with_unobserved_entity() {
    let a = commit(W1, 1, 1, "c1", "One", None);
    let mut wire = raw_template();
    wire["writerId"] = json!(W1);
    wire["writerSeq"] = json!("2");
    wire["commitId"] = json!("20000000-0000-4000-8000-000000000002");
    wire["previousWriterCommit"] = json!(a.reference);
    wire["basisClock"] = json!([a.reference]);
    wire["mutations"][0] = json!({"localMutationId":"40000000-0000-4000-8000-000000000002","entityType":"collection","entityKey":["collection","c1"],"operation":"tombstone","value":{"id":"c1","deletedAt":"2026-01-02T00:00:00.000Z","rev":"2","revActor":"test"},"baseFrontier":[a.reference],"changedFields":["$tombstone"]});
    let delete = object(wire);
    let mut c = database();
    run(&mut c, &mut remote(&[a, delete]));
    assert!(matches!(
        resolve_verified_anchor_v1(&c, &root().physical_root_id, &json!(["collection", "c1"]))
            .unwrap(),
        VerifiedAnchorResolutionV1::Verified(OrdinaryCausalBaseResolutionV1::Ready {
            causal_base: OrdinaryCausalBaseV1::Tombstone,
            ..
        })
    ));
    assert_eq!(
        resolve_verified_anchor_v1(
            &c,
            &root().physical_root_id,
            &json!(["collection", "missing"])
        )
        .unwrap(),
        VerifiedAnchorResolutionV1::Unavailable
    );
}
#[test]
fn malformed_persisted_state_and_unknown_nested_fields_fail_closed() {
    for mode in 0..6 {
        let mut c = database();
        let a = commit(W1, 1, 1, "c1", "One", None);
        let first = run(&mut c, &mut remote(&[a]));
        let mut bad = serde_json::to_value(first.discovery).unwrap();
        match mode {
            0 => bad["state"]["stateVersion"] = json!(2),
            1 => {
                bad["state"]
                    .as_object_mut()
                    .unwrap()
                    .remove("exactWorkScheduler");
            }
            2 => bad["state"]["verifiedObjects"][0]["exactBytesHash"] = json!("0".repeat(64)),
            3 => bad["state"]["historicalAuditCursor"]["unknown"] = json!(true),
            4 => bad["state"]["historicalAuditCursor"]["lastWriterId"] = json!("bad"),
            _ => {
                bad["state"]["exactWorkScheduler"]["afterByClass"]
                    .as_object_mut()
                    .unwrap()
                    .remove("dependency");
            }
        };
        c.execute(
            "UPDATE s2_lite_discovery_v1 SET state_json=?1",
            params![serde_json::to_vec(&bad).unwrap()],
        )
        .unwrap();
        assert!(load_read_state_v1(&c, &root().physical_root_id).is_err());
        assert!(crate::db::setup_db(&c).is_err());
    }
}
#[test]
fn projection_corruption_and_one_sided_rows_fail_closed() {
    let mut c = database();
    run(&mut c, &mut remote(&[]));
    c.execute("DELETE FROM s2_lite_read_projection_v1", [])
        .unwrap();
    assert!(load_read_state_v1(&c, &root().physical_root_id).is_err());
}
#[test]
fn stale_generation_and_injected_failure_cannot_drop_discovery_or_fatal_state() {
    let mut c = database();
    let a = commit(W1, 1, 1, "c1", "One", None);
    let first = run(&mut c, &mut remote(&[a]));
    assert!(save_round_v1(
        &mut c,
        &root().physical_root_id,
        None,
        create_discovery_state_v1()
    )
    .is_err());
    c.execute_batch("CREATE TRIGGER reject_projection BEFORE INSERT ON s2_lite_read_projection_v1 BEGIN SELECT RAISE(ABORT,'injected'); END;").unwrap();
    let b = commit(W1, 1, 2, "c1", "Fork", None);
    assert!(tauri::async_runtime::block_on(discover_read_only_v1(
        &mut c,
        &mut remote(&[b]),
        &DiscoveryBudgetsV1::default()
    ))
    .is_err());
    let loaded = load_read_state_v1(&c, &root().physical_root_id)
        .unwrap()
        .unwrap();
    assert_eq!(loaded.discovery.state, first.discovery.state);
    assert_eq!(loaded.projection, first.projection);
    assert!(loaded.fatal_codes.is_empty());
}

#[test]
fn corrupt_remote_commit_and_unsupported_activation_remain_fatal_after_restart() {
    for activation in [false, true] {
        let (path, mut c) = disk();
        let mut fake = Fake::default();
        if activation {
            let bytes=serde_json::to_vec(&json!({"activationId":"30000000-0000-4000-8000-000000000001","legacyFingerprint":null,"protocol":"watchtracker-s2-lite","protocolVersion":1,"s2SemanticProfileVersion":2,"requiredFeatures":[]})).unwrap();
            let object_path = format!(
                "activations/30000000-0000-4000-8000-000000000001--{}.json",
                super::canonical::sha256_hex(&bytes)
            );
            fake.lists
                .insert("activations".into(), vec![object_path.clone()]);
            fake.objects.insert(object_path, bytes);
        } else {
            let a = commit(W1, 1, 1, "c1", "One", None);
            fake = fake_remote(std::slice::from_ref(&a));
            fake.objects.insert(a.path, b"{}".to_vec());
        }
        let first = run(&mut c, &mut adapter(fake));
        assert!(!first.fatal_codes.is_empty());
        assert!(first.discovery.state.verified_objects.is_empty());
        drop(c);
        let mut c = reopen(&path);
        let later = run(&mut c, &mut remote(&[]));
        assert_eq!(later.fatal_codes, first.fatal_codes);
        assert_eq!(
            later.discovery.state.root_fatal_signals,
            first.discovery.state.root_fatal_signals
        );
        drop(c);
        std::fs::remove_file(path).unwrap();
    }
}

#[test]
fn async_adapter_matches_frozen_round_trace_and_fetch_budget() {
    struct SynchronousFake(Fake);
    impl DiscoveryRemoteV1 for SynchronousFake {
        fn list_directory(&mut self, path: &str) -> DirectoryListResultV1 {
            DirectoryListResultV1::Entries(
                self.0
                    .lists
                    .get(path.trim_end_matches('/'))
                    .cloned()
                    .unwrap_or_default(),
            )
        }
        fn get_exact(&mut self, path: &str) -> DiscoveryExactGetResultV1 {
            self.0
                .objects
                .get(path)
                .cloned()
                .map(DiscoveryExactGetResultV1::DefinitelyPresent)
                .unwrap_or(DiscoveryExactGetResultV1::DefinitelyAbsent)
        }
    }
    struct Tracked(WebDavS2AdapterV1<Fake>, Vec<(bool, String)>);
    #[async_trait]
    impl super::discovery_runtime::ReadOnlyDiscoveryRemoteV1 for Tracked {
        fn physical_root_id(&self) -> &str {
            self.0.physical_root_id()
        }
        async fn list_directory(&mut self, path: &str) -> DirectoryListResultV1 {
            self.1.push((true, path.into()));
            super::discovery_runtime::ReadOnlyDiscoveryRemoteV1::list_directory(&mut self.0, path)
                .await
        }
        async fn get_exact(&mut self, path: &str) -> DiscoveryExactGetResultV1 {
            self.1.push((false, path.into()));
            super::discovery_runtime::ReadOnlyDiscoveryRemoteV1::get_exact(&mut self.0, path).await
        }
    }
    let a = commit(W1, 1, 1, "c1", "One", None);
    let b = commit(W1, 2, 2, "c2", "Two", Some(a.reference.clone()));
    let d = commit(W2, 1, 3, "c3", "Three", None);
    let objects = [a, b, d];
    let budgets = DiscoveryBudgetsV1 {
        max_exact_fetches_per_sync: 1,
        ..DiscoveryBudgetsV1::default()
    };
    let mut sync = SynchronousFake(fake_remote(&objects));
    let mut async_remote = Tracked(remote(&objects), vec![]);
    let mut c = database();
    let mut prior = create_discovery_state_v1();
    for _ in 0..5 {
        let expected = run_discovery_round_v1(
            &prior,
            &mut sync,
            &mut validate_production_activation_body_v1,
            &budgets,
        )
        .unwrap();
        async_remote.1.clear();
        let actual = tauri::async_runtime::block_on(discover_read_only_v1(
            &mut c,
            &mut async_remote,
            &budgets,
        ))
        .unwrap();
        assert_eq!(actual.discovery.state, expected);
        let calls = async_remote
            .1
            .iter()
            .filter(|(list, _)| !list)
            .map(|(_, path)| path.clone())
            .collect::<Vec<_>>();
        assert_eq!(calls, expected.last_round_scheduled_gets);
        assert!(calls.len() <= 1);
        prior = expected;
    }
}

#[test]
fn missing_schema_metadata_or_table_cannot_reinitialize_read_authority() {
    for table_missing in [false, true] {
        let mut c = database();
        run(&mut c, &mut remote(&[]));
        if table_missing {
            c.execute_batch("DROP TABLE s2_lite_read_projection_v1")
                .unwrap();
        } else {
            c.execute(
                "DELETE FROM settings WHERE key='s2_lite_read_authority_schema_version'",
                [],
            )
            .unwrap();
        }
        assert!(crate::db::setup_db(&c).is_err());
    }
}

#[test]
fn persisted_pending_queue_cannot_discard_required_dependency() {
    let a = commit(W1, 1, 1, "c1", "One", None);
    let b = commit(W1, 2, 2, "c2", "Two", Some(a.reference));
    let mut c = database();
    let pending = run(&mut c, &mut remote(&[b]));
    let mut bad = serde_json::to_value(pending.discovery).unwrap();
    bad["state"]["targetedQueue"] = json!([]);
    c.execute(
        "UPDATE s2_lite_discovery_v1 SET state_json=?1",
        params![serde_json::to_vec(&bad).unwrap()],
    )
    .unwrap();
    assert!(load_read_state_v1(&c, &root().physical_root_id).is_err());
    assert!(crate::db::setup_db(&c).is_err());
}
