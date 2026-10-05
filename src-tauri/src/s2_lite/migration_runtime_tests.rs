use super::durable_persistence::SqliteS2LiteStoreV1;
use super::migration_orchestration::{MigrationStateStoreV1, MigrationStatusV1};
use super::migration_runtime::*;
use super::webdav_adapter::*;
use async_trait::async_trait;
use reqwest::{Method, Url};
use rusqlite::Connection;
use serde_json::json;
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};
const NOW: &str = "2026-01-01T00:00:00.000Z";
#[derive(Default)]
struct Cloud {
    objects: BTreeMap<String, Vec<u8>>,
    puts: Vec<(String, Vec<u8>)>,
    lost: bool,
    deny_object_get: bool,
    mismatch_next_verification: bool,
    mismatch_on_put_prefix: Option<&'static str>,
    unavailable_after_next_put: bool,
    unavailable_next_get: bool,
}
#[derive(Clone)]
struct Fake(Arc<Mutex<Cloud>>);
#[async_trait]
impl WebDavTransportV1 for Fake {
    async fn request(
        &mut self,
        method: Method,
        url: Url,
        headers: Vec<(String, String)>,
        body: Option<Vec<u8>>,
    ) -> std::result::Result<WebDavResponseV1, ()> {
        let path = url
            .path()
            .strip_prefix("/dav/")
            .unwrap()
            .trim_end_matches('/')
            .to_string();
        let mut cloud = self.0.lock().unwrap();
        if method == Method::GET {
            if cloud.unavailable_next_get {
                cloud.unavailable_next_get = false;
                return Ok(WebDavResponseV1 {
                    status: 503,
                    body: vec![],
                });
            }
            if cloud.mismatch_next_verification
                && cloud.objects.contains_key(&path)
                && cloud.puts.iter().any(|(put_path, _)| put_path == &path)
            {
                cloud.mismatch_next_verification = false;
                return Ok(WebDavResponseV1 {
                    status: 200,
                    body: b"observed immutable corruption".to_vec(),
                });
            }
            if cloud.deny_object_get && path.ends_with(".json") {
                return Ok(WebDavResponseV1 {
                    status: 503,
                    body: vec![],
                });
            }
            return Ok(WebDavResponseV1 {
                status: if cloud.objects.contains_key(&path) {
                    200
                } else {
                    404
                },
                body: cloud.objects.get(&path).cloned().unwrap_or_default(),
            });
        }
        if method == Method::PUT {
            assert!(headers
                .iter()
                .any(|(key, value)| key.eq_ignore_ascii_case("if-none-match") && value == "*"));
            if cloud
                .mismatch_on_put_prefix
                .is_some_and(|prefix| path.starts_with(prefix))
            {
                cloud.mismatch_on_put_prefix = None;
                cloud.mismatch_next_verification = true;
            }
            if cloud.unavailable_after_next_put {
                cloud.unavailable_after_next_put = false;
                cloud.unavailable_next_get = true;
            }
            let bytes = body.unwrap();
            cloud.puts.push((path.clone(), bytes.clone()));
            cloud.objects.entry(path).or_insert(bytes);
            if cloud.lost {
                return Err(());
            }
            return Ok(WebDavResponseV1 {
                status: 201,
                body: vec![],
            });
        }
        if method.as_str() == "MKCOL" {
            return Ok(WebDavResponseV1 {
                status: 201,
                body: vec![],
            });
        }
        assert_eq!(method.as_str(), "PROPFIND");
        let prefix = format!("{path}/");
        let mut children = std::collections::BTreeSet::new();
        for key in cloud.objects.keys() {
            if let Some(suffix) = key.strip_prefix(&prefix) {
                children.insert(format!("{prefix}{}", suffix.split('/').next().unwrap()));
            }
        }
        let body = format!(
            "<d:multistatus xmlns:d='DAV:'>{}</d:multistatus>",
            children
                .iter()
                .map(|child| format!("<d:response><d:href>/dav/{child}</d:href></d:response>"))
                .collect::<String>()
        )
        .into_bytes();
        Ok(WebDavResponseV1 { status: 207, body })
    }
}
fn remote(cloud: &Arc<Mutex<Cloud>>) -> BlockingWebDavRemoteV1<Fake> {
    let config = WebDavS2ConfigV1 {
        root: webdav_root_v1("https://example.test/dav/", "alice").unwrap(),
        username: "alice".into(),
        password: "secret".into(),
        proxy: None,
        timeout: std::time::Duration::from_secs(1),
    };
    BlockingWebDavRemoteV1::new(WebDavS2AdapterV1::new(config, Fake(cloud.clone())).unwrap())
        .unwrap()
}
fn database(path: Option<&std::path::Path>) -> (Mutex<Connection>, String) {
    let c = path
        .map(Connection::open)
        .unwrap_or_else(Connection::open_in_memory)
        .unwrap();
    crate::db::setup_db(&c).unwrap();
    let url = "https://example.test/dav/";
    let id = crate::sync_targets::target_id(url, "alice");
    let registry = json!({"version":1,"activeTargetId":id,"targetEpoch":1,"targets":[{"id":id,"normalizedUrl":url,"username":"alice","createdAt":NOW,"lastActivatedAt":NOW}]});
    c.execute("INSERT INTO settings(key,value) VALUES('sync_targets_v1',?1) ON CONFLICT(key) DO UPDATE SET value=excluded.value",[registry.to_string()]).unwrap();
    (Mutex::new(c), id)
}
fn seed(c: &Mutex<Connection>) {
    let c = c.lock().unwrap();
    c.execute("INSERT INTO collections(id,name,normalizedName,description,sourceKind,sourceKey,collectionKind,orderMode,createdAt,updatedAt,rev,revActor) VALUES('c1','One','one',NULL,'manual',NULL,'manual','manual',?1,?1,1,'seed')",[NOW]).unwrap();
}
#[test]
fn clean_migration_uses_exact_adapter_and_permanently_blocks_legacy() {
    let (c, id) = database(None);
    seed(&c);
    let cloud = Arc::new(Mutex::new(Cloud::default()));
    let mut remote = remote(&cloud);
    let root = remote.adapter.physical_root_id().to_string();
    let mut last = None;
    for _ in 0..20 {
        let outcome = execute_migration_step_with_adapter_v1(&c, &mut remote, &id, 1, NOW);
        let state = outcome.unwrap().unwrap();
        let done = state.status == MigrationStatusV1::MigrationComplete;
        last = Some(state);
        if done {
            break;
        }
    }
    assert_eq!(last.unwrap().status, MigrationStatusV1::MigrationComplete);
    let mut store = SqliteS2LiteStoreV1::open(&c, &root).unwrap();
    assert!(
        store
            .load_root_safety(&root)
            .unwrap()
            .cutover_state
            .remote_s2_activated
    );
    let called = std::cell::Cell::new(false);
    assert!(run_legacy_put_with_adapter_v1(&c, &mut remote, &id, 1, || {
        called.set(true);
        Ok(())
    })
    .is_err());
    assert!(!called.get());
    assert!(cloud
        .lock()
        .unwrap()
        .puts
        .iter()
        .all(|(path, _)| path.starts_with("writers/") || path.starts_with("activations/")));
}

fn finish(
    c: &Mutex<Connection>,
    id: &str,
    cloud: &Arc<Mutex<Cloud>>,
) -> super::migration_orchestration::MigrationStateV1 {
    for _ in 0..20 {
        let mut remote = remote(cloud);
        let state = execute_migration_step_with_adapter_v1(c, &mut remote, id, 1, NOW)
            .unwrap()
            .unwrap();
        if state.status == MigrationStatusV1::MigrationComplete {
            return state;
        }
    }
    panic!("migration did not finish")
}
fn admit(c: &Mutex<Connection>, id: &str) -> super::migration_orchestration::MigrationStateV1 {
    let writer = {
        let guard = c.lock().unwrap();
        super::local_authority::initialize_writer(&guard)
            .unwrap()
            .writer_id
    };
    super::migration_admission::admit_and_capture_migration_v1(
        c,
        id,
        1,
        "30000000-0000-4000-8000-000000000001",
        &writer,
        NOW,
    )
    .unwrap()
    .state
}
fn root_id() -> String {
    webdav_root_v1("https://example.test/dav/", "alice")
        .unwrap()
        .physical_root_id
}
fn reopen(path: &std::path::Path) -> Mutex<Connection> {
    let c = Connection::open(path).unwrap();
    crate::db::setup_db(&c).unwrap();
    Mutex::new(c)
}
fn temp_path() -> std::path::PathBuf {
    std::env::temp_dir().join(format!("wt-i64-{}.sqlite", uuid::Uuid::new_v4()))
}
#[test]
fn restart_after_planning_and_each_executor_boundary_reuses_all_identities() {
    let path = temp_path();
    let (mut c, id) = database(Some(&path));
    seed(&c);
    drop(c);
    c = reopen(&path);
    let planned = admit(&c, &id);
    assert_eq!(planned.status, MigrationStatusV1::BootstrapPlanned);
    assert!(c
        .lock()
        .unwrap()
        .execute("UPDATE collections SET name='Forbidden' WHERE id='c1'", [])
        .is_err());
    let cloud = Arc::new(Mutex::new(Cloud::default()));
    let mut statuses = Vec::new();
    for _ in 0..20 {
        drop(c);
        c = reopen(&path);
        let mut remote = remote(&cloud);
        let state = execute_migration_step_with_adapter_v1(&c, &mut remote, &id, 1, NOW)
            .unwrap()
            .unwrap();
        assert_eq!(state.migration_id, planned.migration_id);
        assert_eq!(state.writer_id, planned.writer_id);
        assert_eq!(state.snapshot, planned.snapshot);
        assert_eq!(state.stage_a[0].intent, planned.stage_a[0].intent);
        assert_eq!(state.activation_intent, planned.activation_intent);
        statuses.push(state.status);
        if state.status == MigrationStatusV1::MigrationComplete {
            break;
        }
    }
    assert!(statuses.contains(&MigrationStatusV1::ActivationVerified));
    assert_eq!(statuses.last(), Some(&MigrationStatusV1::MigrationComplete));
    drop(c);
    let c = reopen(&path);
    let mut remote = remote(&cloud);
    let called = std::cell::Cell::new(false);
    assert!(run_legacy_put_with_adapter_v1(&c, &mut remote, &id, 1, || {
        called.set(true);
        Ok(())
    })
    .is_err());
    assert!(!called.get());
    let root = root_id();
    let mut store = SqliteS2LiteStoreV1::open(&c, &root).unwrap();
    assert!(
        store
            .load_root_safety(&root_id())
            .unwrap()
            .cutover_state
            .remote_s2_activated
    );
    drop(c);
    std::fs::remove_file(path).unwrap();
}
#[test]
fn lost_activation_and_bootstrap_put_responses_are_exactly_verified() {
    let (c, id) = database(None);
    seed(&c);
    let cloud = Arc::new(Mutex::new(Cloud {
        lost: true,
        ..Cloud::default()
    }));
    let state = finish(&c, &id, &cloud);
    assert!(state.activation_receipt.is_some());
    assert_eq!(
        cloud.lock().unwrap().puts.len(),
        state.stage_a.len() + state.stage_b.len() + 1
    );
}
#[test]
fn crash_after_remote_put_before_receipt_retains_prepared_bytes() {
    for kind in ["commit", "activation"] {
        let path = temp_path();
        let (mut c, id) = database(Some(&path));
        seed(&c);
        let planned = admit(&c, &id);
        let cloud = Arc::new(Mutex::new(Cloud::default()));
        c.lock().unwrap().execute_batch(&format!("CREATE TRIGGER crash_receipt BEFORE INSERT ON s2_lite_published_receipt_v1 WHEN NEW.receipt_kind='{kind}' BEGIN SELECT RAISE(ABORT,'crash'); END;")).unwrap();
        let mut saw = false;
        for _ in 0..8 {
            let mut remote = remote(&cloud);
            if execute_migration_step_with_adapter_v1(&c, &mut remote, &id, 1, NOW).is_err() {
                saw = true;
                break;
            }
        }
        assert!(saw);
        assert!(cloud
            .lock()
            .unwrap()
            .objects
            .keys()
            .any(|key| if kind == "commit" {
                key.starts_with("writers/")
            } else {
                key.starts_with("activations/")
            }));
        drop(c);
        c = reopen(&path);
        c.lock()
            .unwrap()
            .execute_batch("DROP TRIGGER crash_receipt")
            .unwrap();
        let state = finish(&c, &id, &cloud);
        assert_eq!(state.stage_a[0].intent, planned.stage_a[0].intent);
        assert_eq!(state.activation_intent, planned.activation_intent);
        assert_eq!(cloud.lock().unwrap().puts.len(), 2);
        drop(c);
        std::fs::remove_file(path).unwrap();
    }
}
#[test]
fn receipt_before_state_transition_is_reused_after_restart() {
    for kind in ["commit", "activation"] {
        let path = temp_path();
        let (mut c, id) = database(Some(&path));
        seed(&c);
        admit(&c, &id);
        let cloud = Arc::new(Mutex::new(Cloud::default()));
        c.lock().unwrap().execute_batch(&format!("CREATE TRIGGER crash_cas BEFORE UPDATE ON s2_lite_migration_v1 WHEN EXISTS(SELECT 1 FROM s2_lite_published_receipt_v1 WHERE receipt_kind='{kind}') BEGIN SELECT RAISE(ABORT,'crash'); END;")).unwrap();
        let mut saw = false;
        for _ in 0..8 {
            let mut remote = remote(&cloud);
            if execute_migration_step_with_adapter_v1(&c, &mut remote, &id, 1, NOW).is_err() {
                saw = true;
                break;
            }
        }
        assert!(saw);
        let count: i64 = c
            .lock()
            .unwrap()
            .query_row(
                "SELECT COUNT(*) FROM s2_lite_published_receipt_v1 WHERE receipt_kind=?1",
                [kind],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(count, 1);
        drop(c);
        c = reopen(&path);
        c.lock()
            .unwrap()
            .execute_batch("DROP TRIGGER crash_cas")
            .unwrap();
        finish(&c, &id, &cloud);
        assert_eq!(cloud.lock().unwrap().puts.len(), 2);
        drop(c);
        std::fs::remove_file(path).unwrap();
    }
}
#[test]
fn activation_same_path_other_bytes_freezes_and_survives_restart() {
    let path = temp_path();
    let (c, id) = database(Some(&path));
    seed(&c);
    let planned = admit(&c, &id);
    let intent = planned.activation_intent.unwrap();
    let cloud = Arc::new(Mutex::new(Cloud::default()));
    cloud
        .lock()
        .unwrap()
        .objects
        .insert(intent.remote_path, b"corrupt".to_vec());
    let mut remote = remote(&cloud);
    assert!(
        execute_migration_step_with_adapter_v1(&c, &mut remote, &id, 1, NOW).is_err()
            || !SqliteS2LiteStoreV1::open(&c, &root_id())
                .unwrap()
                .load_root_safety(&root_id())
                .unwrap()
                .root_fatal_signals
                .is_empty()
    );
    drop(c);
    let c = reopen(&path);
    let root = root_id();
    let mut store = SqliteS2LiteStoreV1::open(&c, &root).unwrap();
    assert!(!store
        .load_root_safety(&root_id())
        .unwrap()
        .root_fatal_signals
        .is_empty());
    let called = std::cell::Cell::new(false);
    assert!(run_legacy_put_with_adapter_v1(&c, &mut remote, &id, 1, || {
        called.set(true);
        Ok(())
    })
    .is_err());
    assert!(!called.get());
    drop(c);
    std::fs::remove_file(path).unwrap();
}
#[test]
fn preexisting_compatible_activation_blocks_s1_and_does_not_publish() {
    let (c, id) = database(None);
    seed(&c);
    let cloud = Arc::new(Mutex::new(Cloud::default()));
    finish(&c, &id, &cloud);
    let (other, other_id) = database(None);
    seed(&other);
    let before = cloud.lock().unwrap().puts.len();
    let mut remote = remote(&cloud);
    assert!(
        execute_migration_step_with_adapter_v1(&other, &mut remote, &other_id, 1, NOW)
            .unwrap()
            .is_none()
    );
    assert_eq!(cloud.lock().unwrap().puts.len(), before);
    assert!(run_legacy_put_with_adapter_v1(&other, &mut remote, &other_id, 1, || Ok(())).is_err());
}
#[test]
fn nullable_fingerprint_mismatch_is_durable_and_blocks_all_writes() {
    let (c, id) = database(None);
    seed(&c);
    let cloud = Arc::new(Mutex::new(Cloud::default()));
    finish(&c, &id, &cloud);
    let (other, other_id) = database(None);
    let mut remote = remote(&cloud);
    assert!(
        execute_migration_step_with_adapter_v1(&other, &mut remote, &other_id, 1, NOW).is_err()
    );
    let root = root_id();
    let mut store = SqliteS2LiteStoreV1::open(&other, &root).unwrap();
    let safety = store.load_root_safety(&root_id()).unwrap();
    assert!(safety.cutover_state.remote_s2_activated);
    assert!(safety
        .root_fatal_signals
        .iter()
        .any(|fatal| fatal.code == "SYNC_ROOT_FROZEN_LEGACY_CHANGE"));
}
#[test]
fn earlier_unavailable_basis_is_preserved_and_new_basis_uses_applied_projection() {
    use super::local_authority::*;
    let (c, id) = database(None);
    seed(&c);
    let old = {
        let guard = c.lock().unwrap();
        capture_staged_descriptor(
            &guard,
            "collection",
            "old",
            None,
            Some(json!({"id":"old"})),
            0,
        )
        .unwrap()
    };
    let cloud = Arc::new(Mutex::new(Cloud::default()));
    finish(&c, &id, &cloud);
    let mut guard = c.lock().unwrap();
    let tx = guard.transaction().unwrap();
    let live =
        capture_staged_descriptor(&tx, "collection", "c1", None, Some(json!({"id":"c1"})), 1)
            .unwrap();
    assert!(matches!(
        live.causal_anchor,
        StagingAnchorStateV1::Live { .. }
    ));
    assert!(!live.verified_basis.unwrap().base_frontier.is_empty());
    let absent =
        capture_staged_descriptor(&tx, "collection", "new", None, Some(json!({"id":"new"})), 1)
            .unwrap();
    assert_eq!(absent.causal_anchor, StagingAnchorStateV1::Absent);
    let earlier = capture_staged_descriptor(
        &tx,
        "collection",
        "old",
        None,
        Some(json!({"id":"old","name":"changed"})),
        1,
    )
    .unwrap();
    assert_eq!(earlier.causal_anchor, StagingAnchorStateV1::Unavailable);
    assert_eq!(earlier.local_mutation_id, old.local_mutation_id);
    assert_eq!(earlier.first_generation, old.first_generation);
    tx.rollback().unwrap();
    assert_eq!(load_staged_descriptors(&guard).unwrap().len(), 1);
}
#[test]
fn stale_target_and_different_root_do_not_inherit_cutover() {
    let (c, id) = database(None);
    seed(&c);
    let cloud = Arc::new(Mutex::new(Cloud::default()));
    finish(&c, &id, &cloud);
    let other_root = webdav_root_v1("https://example.test/other/", "alice")
        .unwrap()
        .physical_root_id;
    let mut store = SqliteS2LiteStoreV1::open(&c, &other_root).unwrap();
    assert!(
        !store
            .load_root_safety(&other_root)
            .unwrap()
            .cutover_state
            .remote_s2_activated
    );
    let mut remote = remote(&cloud);
    assert!(
        execute_migration_step_with_adapter_v1(&c, &mut remote, "stale-target", 0, NOW).is_err()
    );
    let config = WebDavS2ConfigV1 {
        root: webdav_root_v1("https://example.test/other/", "alice").unwrap(),
        username: "alice".into(),
        password: "secret".into(),
        proxy: None,
        timeout: std::time::Duration::from_secs(1),
    };
    let mut mismatch =
        BlockingWebDavRemoteV1::new(WebDavS2AdapterV1::new(config, Fake(cloud.clone())).unwrap())
            .unwrap();
    assert!(execute_migration_step_with_adapter_v1(&c, &mut mismatch, &id, 1, NOW).is_err());
}
#[test]
fn final_legacy_acknowledgement_gate_rejects_stale_business_commit() {
    let (c, id) = database(None);
    seed(&c);
    {
        let guard = c.lock().unwrap();
        super::durable_persistence::admit_legacy_business_ack_v1(&guard).unwrap();
    }
    let cloud = Arc::new(Mutex::new(Cloud::default()));
    finish(&c, &id, &cloud);
    let mut guard = c.lock().unwrap();
    let tx = guard.transaction().unwrap();
    assert!(super::durable_persistence::admit_legacy_business_ack_v1(&tx).is_err());
    tx.rollback().unwrap();
}

fn put_activation(
    cloud: &Arc<Mutex<Cloud>>,
    fingerprint: serde_json::Value,
    salt: u64,
    features: serde_json::Value,
) {
    let body=serde_json::to_vec(&json!({"activationId":format!("50000000-0000-4000-8000-{salt:012}"),"legacyFingerprint":fingerprint,"protocol":"watchtracker-s2-lite","protocolVersion":1,"requiredFeatures":features,"s2SemanticProfileVersion":1})).unwrap();
    let hash = super::canonical::sha256_hex(&body);
    let path = format!("activations/50000000-0000-4000-8000-{salt:012}--{hash}.json");
    cloud.lock().unwrap().objects.insert(path, body);
}
fn put_collection(cloud: &Arc<Mutex<Cloud>>, writer: u64, salt: u64, name: &str) {
    use base64::Engine;
    let fixture: serde_json::Value = serde_json::from_str(include_str!(
        "../../../contracts/s2-lite/v1/raw-wire-json-v1.json"
    ))
    .unwrap();
    let case = fixture["cases"]
        .as_array()
        .unwrap()
        .iter()
        .find(|case| case["name"] == "canonical-mutation-resolves-absent")
        .unwrap();
    let mut wire: serde_json::Value = serde_json::from_slice(
        &base64::engine::general_purpose::STANDARD
            .decode(case["utf8Base64"].as_str().unwrap())
            .unwrap(),
    )
    .unwrap();
    wire["writerId"] = json!(format!("10000000-0000-4000-8000-{writer:012}"));
    wire["writerSeq"] = json!("1");
    wire["commitId"] = json!(format!("20000000-0000-4000-8000-{salt:012}"));
    wire["previousWriterCommit"] = serde_json::Value::Null;
    wire["basisClock"] = json!([]);
    wire["mutations"][0]["localMutationId"] = json!(format!("40000000-0000-4000-8000-{salt:012}"));
    wire["mutations"][0]["entityKey"] = json!(["collection", "c1"]);
    wire["mutations"][0]["value"]["id"] = json!("c1");
    wire["mutations"][0]["value"]["name"] = json!(name);
    wire["mutations"][0]["value"]["normalizedName"] = json!(name.to_lowercase());
    wire.as_object_mut().unwrap().remove("contentHash");
    let bytes = serde_json::to_vec(&wire).unwrap();
    let reference = super::causal::decode_frozen_wire_commit_v1(&bytes)
        .unwrap()
        .commit_ref();
    let path = super::immutable_publish::build_commit_remote_path_v1(&reference).unwrap();
    cloud.lock().unwrap().objects.insert(path, bytes);
}
#[test]
fn compatible_null_activation_and_multiple_candidates_latch_without_local_migration() {
    let (c, id) = database(None);
    let cloud = Arc::new(Mutex::new(Cloud::default()));
    put_activation(&cloud, serde_json::Value::Null, 1, json!([]));
    put_activation(&cloud, serde_json::Value::Null, 2, json!([]));
    let mut remote = remote(&cloud);
    assert!(
        execute_migration_step_with_adapter_v1(&c, &mut remote, &id, 1, NOW)
            .unwrap()
            .is_none()
    );
    let root = root_id();
    let mut store = SqliteS2LiteStoreV1::open(&c, &root).unwrap();
    let safety = store.load_root_safety(&root).unwrap();
    assert!(safety.cutover_state.remote_s2_activated);
    assert!(safety.root_fatal_signals.is_empty());
    assert!(cloud.lock().unwrap().puts.is_empty());
}
#[test]
fn null_activation_cannot_attach_to_nonnull_legacy_snapshot() {
    let (c, id) = database(None);
    seed(&c);
    let cloud = Arc::new(Mutex::new(Cloud::default()));
    put_activation(&cloud, serde_json::Value::Null, 1, json!([]));
    let mut remote = remote(&cloud);
    assert!(execute_migration_step_with_adapter_v1(&c, &mut remote, &id, 1, NOW).is_err());
    let root = root_id();
    let mut store = SqliteS2LiteStoreV1::open(&c, &root).unwrap();
    assert!(store
        .load_root_safety(&root)
        .unwrap()
        .root_fatal_signals
        .iter()
        .any(|fatal| fatal.code == "SYNC_ROOT_FROZEN_LEGACY_CHANGE"));
    assert!(cloud.lock().unwrap().puts.is_empty());
}
#[test]
fn conflicts_are_retained_and_cannot_replace_business_rows_or_supply_new_live_basis() {
    let (c, id) = database(None);
    seed(&c);
    let cloud = Arc::new(Mutex::new(Cloud::default()));
    put_collection(&cloud, 1, 1, "Remote A");
    put_collection(&cloud, 2, 2, "Remote B");
    finish(&c, &id, &cloud);
    let root = root_id();
    let mut store = SqliteS2LiteStoreV1::open(&c, &root).unwrap();
    let projection = store.load_materialized_projection().unwrap().unwrap();
    assert!(projection
        .state
        .entities
        .iter()
        .any(|entity| entity.entity_key == json!(["collection", "c1"]) && entity.conflict));
    let guard = c.lock().unwrap();
    assert_eq!(crate::collections::all(&guard).unwrap()[0].name, "One");
    let descriptor = super::local_authority::capture_staged_descriptor(
        &guard,
        "collection",
        "c1",
        None,
        Some(json!({"id":"c1"})),
        1,
    )
    .unwrap();
    assert_eq!(
        descriptor.causal_anchor,
        super::local_authority::StagingAnchorStateV1::Unavailable
    );
}
#[test]
fn writer_fork_blocks_migration_before_any_publication() {
    let (c, id) = database(None);
    seed(&c);
    let cloud = Arc::new(Mutex::new(Cloud::default()));
    put_collection(&cloud, 1, 1, "Remote A");
    put_collection(&cloud, 1, 2, "Remote B");
    let mut remote = remote(&cloud);
    let result = execute_migration_step_with_adapter_v1(&c, &mut remote, &id, 1, NOW);
    assert!(result.is_err() || result.unwrap().unwrap().status == MigrationStatusV1::RootFrozen);
    assert!(cloud.lock().unwrap().puts.is_empty());
    let root = root_id();
    let mut store = SqliteS2LiteStoreV1::open(&c, &root).unwrap();
    assert!(!store
        .load_root_safety(&root)
        .unwrap()
        .root_fatal_signals
        .is_empty());
}
#[test]
fn unsupported_activation_fails_closed_before_legacy_put() {
    let (c, id) = database(None);
    let cloud = Arc::new(Mutex::new(Cloud::default()));
    put_activation(
        &cloud,
        serde_json::Value::Null,
        1,
        json!(["future-required-feature"]),
    );
    let mut remote = remote(&cloud);
    let called = std::cell::Cell::new(false);
    let result = run_legacy_put_with_adapter_v1(&c, &mut remote, &id, 1, || {
        called.set(true);
        Ok(())
    });
    assert!(
        result.is_err()
            || !matches!(
                result.unwrap(),
                super::durable_persistence::LegacyS1PublishAdmissionV1::Executed(_)
            )
    );
    assert!(!called.get());
}
#[test]
fn cutover_finalization_faults_rollback_owner_writer_and_completion_together() {
    use super::durable_persistence::MigrationFinalizationFaultV1::*;
    for fault in [
        MigrationCompletePersisted,
        WriterHandoffPersisted,
        SourceRetired,
    ] {
        let (c, id) = database(None);
        seed(&c);
        let cloud = Arc::new(Mutex::new(Cloud::default()));
        for _ in 0..10 {
            let mut remote = remote(&cloud);
            let state = execute_migration_step_with_adapter_v1(&c, &mut remote, &id, 1, NOW)
                .unwrap()
                .unwrap();
            if state.status == MigrationStatusV1::ActivationVerified {
                break;
            }
        }
        let root = root_id();
        let mut store = SqliteS2LiteStoreV1::open(&c, &root).unwrap();
        store
            .recover_durable_verified_activation_cutover_v1()
            .unwrap();
        assert!(store
            .finalize_verified_migration_with_injected_fault_v1(fault)
            .is_err());
        assert_eq!(
            MigrationStateStoreV1::load(&mut store, &root)
                .unwrap()
                .unwrap()
                .status,
            MigrationStatusV1::ActivationVerified
        );
        assert!(store.migration_source_protected_v1().unwrap());
        assert!(
            store
                .load_root_safety(&root)
                .unwrap()
                .cutover_state
                .remote_s2_activated
        );
        finish(&c, &id, &cloud);
        assert!(!store.migration_source_protected_v1().unwrap());
    }
}
#[test]
fn malformed_migration_authority_is_not_recreated_on_restart() {
    let path = temp_path();
    let (c, id) = database(Some(&path));
    seed(&c);
    admit(&c, &id);
    c.lock()
        .unwrap()
        .execute(
            "UPDATE s2_lite_migration_v1 SET state_json=?1",
            [b"{}".as_slice()],
        )
        .unwrap();
    drop(c);
    let c = reopen(&path);
    let cloud = Arc::new(Mutex::new(Cloud::default()));
    let mut remote = remote(&cloud);
    assert!(execute_migration_step_with_adapter_v1(&c, &mut remote, &id, 1, NOW).is_err());
    assert!(run_legacy_put_with_adapter_v1(&c, &mut remote, &id, 1, || Ok(())).is_err());
    assert!(cloud.lock().unwrap().puts.is_empty());
    drop(c);
    std::fs::remove_file(path).unwrap();
}

fn seed_all(c: &Mutex<Connection>) {
    seed(c);
    let guard = c.lock().unwrap();
    let record=serde_json::from_value(json!({"id":"r1","originalName":"Series","chineseName":"Series","progress":"","totalEpisodes":6,"status":"未看","platform":"","notes":"","createdAt":NOW,"updatedAt":NOW,"mediaType":"剧集","rev":3,"revActor":"seed"})).unwrap();
    crate::db::insert_record(&guard, record).unwrap();
    let member = crate::collections::member_id("c1", "r1");
    guard.execute("INSERT INTO collection_members(id,collectionId,recordId,position,sourceKind,createdAt,updatedAt,rev,revActor) VALUES(?1,'c1','r1',0,'manual',?2,?2,3,'seed')",rusqlite::params![member,NOW]).unwrap();
    let episode = super::canonical::sha256_hex(b"episode-completion:v1\0r1\x001");
    guard.execute("INSERT INTO episode_completions(id,recordId,episodeNumber,completedAt,createdAt,updatedAt,rev,revActor) VALUES(?1,'r1',1,?2,?2,?2,3,'seed')",rusqlite::params![episode,NOW]).unwrap();
}
#[test]
fn all_four_entity_classes_survive_stage_b_and_apply_to_empty_business_tables() {
    let (c, id) = database(None);
    seed_all(&c);
    let cloud = Arc::new(Mutex::new(Cloud::default()));
    let state = finish(&c, &id, &cloud);
    assert!(!state.stage_b.is_empty());
    let (other, _) = database(None);
    let root = root_id();
    let mut read_remote = remote(&cloud);
    read_remote.discover(&other).unwrap();
    let mut store = SqliteS2LiteStoreV1::open(&other, &root).unwrap();
    store.refresh_from_read_authority_v1().unwrap();
    store.initialize_desktop_writer_v1().unwrap();
    let projection = store.load_materialized_projection().unwrap().unwrap();
    store
        .update_materialized_projection_generation(projection.projection_generation)
        .unwrap();
    super::business_projection::apply_complete_projection_v1(
        &mut store,
        projection.projection_generation,
    )
    .unwrap();
    let guard = other.lock().unwrap();
    assert_eq!(crate::db::get_all_records(&guard).unwrap().len(), 1);
    assert_eq!(crate::collections::all(&guard).unwrap().len(), 1);
    assert_eq!(crate::collections::all_members(&guard).unwrap().len(), 1);
    assert_eq!(
        crate::episode_history::all_completions(&guard)
            .unwrap()
            .len(),
        1
    );
    assert!(super::local_authority::load_staged_descriptors(&guard)
        .unwrap()
        .is_empty());
    assert_eq!(
        crate::db_atomic_helpers::get_records_generation(&guard).unwrap(),
        0
    );
}
#[test]
fn projection_business_failure_rolls_back_all_rows_and_applied_marker() {
    let (c, id) = database(None);
    seed_all(&c);
    let cloud = Arc::new(Mutex::new(Cloud::default()));
    finish(&c, &id, &cloud);
    let (other, _) = database(None);
    let root = root_id();
    let mut read_remote = remote(&cloud);
    read_remote.discover(&other).unwrap();
    let mut store = SqliteS2LiteStoreV1::open(&other, &root).unwrap();
    store.refresh_from_read_authority_v1().unwrap();
    store.initialize_desktop_writer_v1().unwrap();
    let projection = store.load_materialized_projection().unwrap().unwrap();
    store
        .update_materialized_projection_generation(projection.projection_generation)
        .unwrap();
    other.lock().unwrap().execute_batch("CREATE TRIGGER fail_projection BEFORE INSERT ON episode_completions BEGIN SELECT RAISE(ABORT,'projection failure'); END").unwrap();
    assert!(super::business_projection::apply_complete_projection_v1(
        &mut store,
        projection.projection_generation
    )
    .is_err());
    assert!(store
        .load_materialized_projection()
        .unwrap()
        .unwrap()
        .business_projection_applied_generation
        .is_none());
    let guard = other.lock().unwrap();
    assert!(crate::db::get_all_records(&guard).unwrap().is_empty());
    assert!(crate::collections::all(&guard).unwrap().is_empty());
}
#[test]
fn legacy_put_before_activation_is_allowed_with_exact_original_payload() {
    let (c, id) = database(None);
    let cloud = Arc::new(Mutex::new(Cloud::default()));
    let mut remote = remote(&cloud);
    let original = b"S1 exact payload";
    let result =
        run_legacy_put_with_adapter_v1(&c, &mut remote, &id, 1, || Ok(original.to_vec())).unwrap();
    assert_eq!(
        result,
        super::durable_persistence::LegacyS1PublishAdmissionV1::Executed(original.to_vec())
    );
    assert!(cloud.lock().unwrap().puts.is_empty());
}

#[test]
fn empty_legacy_snapshot_uses_frozen_nullable_nonnull_migration_fingerprint() {
    let (c, id) = database(None);
    let cloud = Arc::new(Mutex::new(Cloud::default()));
    let state = finish(&c, &id, &cloud);
    assert!(state.stage_a.is_empty());
    assert!(state.stage_b.is_empty());
    assert!(state.snapshot.unwrap().canonical_entities.is_empty());
    assert_eq!(cloud.lock().unwrap().puts.len(), 1);
}
#[test]
fn durable_receipt_recovers_latch_even_if_activation_listing_is_later_omitted() {
    let path = temp_path();
    let (c, id) = database(Some(&path));
    seed(&c);
    let cloud = Arc::new(Mutex::new(Cloud::default()));
    for _ in 0..10 {
        let mut remote = remote(&cloud);
        let state = execute_migration_step_with_adapter_v1(&c, &mut remote, &id, 1, NOW)
            .unwrap()
            .unwrap();
        if state.status == MigrationStatusV1::ActivationVerified {
            break;
        }
    }
    let root = root_id();
    let mut store = SqliteS2LiteStoreV1::open(&c, &root).unwrap();
    assert!(
        !store
            .load_root_safety(&root)
            .unwrap()
            .cutover_state
            .remote_s2_activated
    );
    drop(c);
    cloud
        .lock()
        .unwrap()
        .objects
        .retain(|path, _| !path.starts_with("activations/"));
    let c = reopen(&path);
    let state = finish(&c, &id, &cloud);
    assert_eq!(state.status, MigrationStatusV1::MigrationComplete);
    let mut store = SqliteS2LiteStoreV1::open(&c, &root).unwrap();
    assert!(
        store
            .load_root_safety(&root)
            .unwrap()
            .cutover_state
            .remote_s2_activated
    );
    let mut remote = remote(&cloud);
    assert!(run_legacy_put_with_adapter_v1(&c, &mut remote, &id, 1, || Ok(())).is_err());
    assert_eq!(cloud.lock().unwrap().puts.len(), 2);
    drop(c);
    std::fs::remove_file(path).unwrap();
}
#[test]
fn restart_between_local_latch_and_finalization_remains_permanently_closed() {
    let path = temp_path();
    let (c, id) = database(Some(&path));
    seed(&c);
    let cloud = Arc::new(Mutex::new(Cloud::default()));
    for _ in 0..10 {
        let mut remote = remote(&cloud);
        let state = execute_migration_step_with_adapter_v1(&c, &mut remote, &id, 1, NOW)
            .unwrap()
            .unwrap();
        if state.status == MigrationStatusV1::ActivationVerified {
            break;
        }
    }
    let root = root_id();
    let mut store = SqliteS2LiteStoreV1::open(&c, &root).unwrap();
    store
        .recover_durable_verified_activation_cutover_v1()
        .unwrap();
    drop(c);
    let c = reopen(&path);
    let mut remote = remote(&cloud);
    assert!(run_legacy_put_with_adapter_v1(&c, &mut remote, &id, 1, || Ok(())).is_err());
    finish(&c, &id, &cloud);
    drop(c);
    std::fs::remove_file(path).unwrap();
}
#[test]
fn target_epoch_change_rejects_previously_captured_legacy_ticket_at_final_gate() {
    let (c, id) = database(None);
    let root = root_id();
    let binding = super::target_root_binding::resolve_active_target_root_binding_v1(&c, &id, 1)
        .unwrap()
        .binding;
    let ticket = super::durable_persistence::capture_legacy_route_ticket_v1(&c, &binding).unwrap();
    c.lock().unwrap().execute("UPDATE settings SET value=json_set(value,'$.targetEpoch',2) WHERE key='sync_targets_v1'",[]).unwrap();
    let mut store = SqliteS2LiteStoreV1::open(&c, &root).unwrap();
    let called = std::cell::Cell::new(false);
    let result = store.run_legacy_bound_put_v1(&ticket, || {
        called.set(true);
        Ok(())
    });
    assert!(
        result.is_err()
            || !matches!(
                result.unwrap(),
                super::durable_persistence::LegacyS1PublishAdmissionV1::Executed(_)
            )
    );
    assert!(!called.get());
}

#[test]
fn adopted_remote_activation_compatibility_is_not_rebuilt_after_local_edits() {
    let path = temp_path();
    let (c, id) = database(Some(&path));
    let cloud = Arc::new(Mutex::new(Cloud::default()));
    put_activation(&cloud, serde_json::Value::Null, 1, json!([]));
    let mut remote = remote(&cloud);
    assert!(
        execute_migration_step_with_adapter_v1(&c, &mut remote, &id, 1, NOW)
            .unwrap()
            .is_none()
    );
    seed(&c);
    drop(c);
    let c = reopen(&path);
    assert!(
        execute_migration_step_with_adapter_v1(&c, &mut remote, &id, 1, NOW)
            .unwrap()
            .is_none()
    );
    assert!(cloud.lock().unwrap().puts.is_empty());
    drop(c);
    std::fs::remove_file(path).unwrap();
}

#[test]
fn actual_business_update_and_create_capture_live_and_absent_bases_on_restart() {
    let path = temp_path();
    let (c, id) = database(Some(&path));
    seed(&c);
    let cloud = Arc::new(Mutex::new(Cloud::default()));
    finish(&c, &id, &cloud);
    let original = {
        let mut guard = c.lock().unwrap();
        let updated = crate::collections::update(
            &mut guard,
            "c1",
            serde_json::from_value(json!({"name":"Offline","expectedRev":1})).unwrap(),
            "device",
        )
        .unwrap();
        let descriptor = super::local_authority::load_staged_descriptors(&guard)
            .unwrap()
            .into_iter()
            .find(|row| row.entity_id == "c1")
            .unwrap();
        assert!(
            matches!(descriptor.causal_anchor,super::local_authority::StagingAnchorStateV1::Live{ref value} if value["name"]=="One")
        );
        crate::collections::update(
            &mut guard,
            "c1",
            serde_json::from_value(json!({"name":"Offline 2","expectedRev":updated.rev})).unwrap(),
            "device",
        )
        .unwrap();
        let created = crate::collections::create(
            &mut guard,
            serde_json::from_value(json!({"name":"Brand new"})).unwrap(),
            "device",
        )
        .unwrap();
        let rows = super::local_authority::load_staged_descriptors(&guard).unwrap();
        assert_eq!(
            rows.iter()
                .find(|row| row.entity_id == created.id)
                .unwrap()
                .causal_anchor,
            super::local_authority::StagingAnchorStateV1::Absent
        );
        descriptor
    };
    drop(c);
    let c = reopen(&path);
    {
        let guard = c.lock().unwrap();
        let repeated = super::local_authority::load_staged_descriptors(&guard)
            .unwrap()
            .into_iter()
            .find(|row| row.entity_id == "c1")
            .unwrap();
        assert_eq!(repeated.local_mutation_id, original.local_mutation_id);
        assert_eq!(repeated.first_generation, original.first_generation);
        assert_eq!(repeated.causal_anchor, original.causal_anchor);
        assert_eq!(repeated.verified_basis, original.verified_basis);
    }
    drop(c);
    std::fs::remove_file(path).unwrap();
}
#[test]
fn actual_business_update_capture_failure_rolls_back_business_s1_s2_and_generation() {
    let (c, id) = database(None);
    seed(&c);
    let cloud = Arc::new(Mutex::new(Cloud::default()));
    finish(&c, &id, &cloud);
    let mut guard = c.lock().unwrap();
    let before = crate::sync_staging::get_staging(&guard).unwrap();
    let generation = crate::db_atomic_helpers::get_records_generation(&guard).unwrap();
    guard.execute_batch("CREATE TRIGGER fail_capture BEFORE INSERT ON s2_lite_local_staging_descriptor_v1 BEGIN SELECT RAISE(ABORT,'capture failure'); END").unwrap();
    assert!(crate::collections::update(
        &mut guard,
        "c1",
        serde_json::from_value(json!({"name":"Not committed","expectedRev":1})).unwrap(),
        "device"
    )
    .is_err());
    assert_eq!(crate::collections::all(&guard).unwrap()[0].name, "One");
    assert_eq!(
        crate::db_atomic_helpers::get_records_generation(&guard).unwrap(),
        generation
    );
    assert_eq!(crate::sync_staging::get_staging(&guard).unwrap(), before);
    assert!(super::local_authority::load_staged_descriptors(&guard)
        .unwrap()
        .is_empty());
}

#[test]
fn restart_after_prepared_intent_before_first_put_reuses_exact_bytes() {
    let path = temp_path();
    let (c, id) = database(Some(&path));
    seed(&c);
    let planned = admit(&c, &id);
    let cloud = Arc::new(Mutex::new(Cloud {
        deny_object_get: true,
        ..Cloud::default()
    }));
    let mut remote = remote(&cloud);
    let state = execute_migration_step_with_adapter_v1(&c, &mut remote, &id, 1, NOW)
        .unwrap()
        .unwrap();
    assert_eq!(state.status, MigrationStatusV1::StageAPublishing);
    let bytes: Vec<u8> = c
        .lock()
        .unwrap()
        .query_row(
            "SELECT exact_bytes FROM s2_lite_prepared_intent_v1 WHERE intent_kind='commit'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(bytes, planned.stage_a[0].intent.exact_bytes);
    assert!(cloud.lock().unwrap().puts.is_empty());
    drop(c);
    let c = reopen(&path);
    cloud.lock().unwrap().deny_object_get = false;
    let completed = finish(&c, &id, &cloud);
    assert_eq!(completed.stage_a[0].intent, planned.stage_a[0].intent);
    drop(c);
    std::fs::remove_file(path).unwrap();
}
#[test]
fn snapshot_plan_guard_and_execution_capture_failure_is_atomic() {
    let (c, id) = database(None);
    seed(&c);
    c.lock().unwrap().execute_batch("CREATE TRIGGER fail_guard BEFORE INSERT ON s2_lite_migration_source_guard_v1 BEGIN SELECT RAISE(ABORT,'capture crash'); END").unwrap();
    let writer = {
        let guard = c.lock().unwrap();
        super::local_authority::initialize_writer(&guard)
            .unwrap()
            .writer_id
    };
    assert!(super::migration_admission::admit_and_capture_migration_v1(
        &c,
        &id,
        1,
        "30000000-0000-4000-8000-000000000001",
        &writer,
        NOW
    )
    .is_err());
    let guard = c.lock().unwrap();
    for table in [
        "s2_lite_migration_v1",
        "s2_lite_migration_execution_binding_v1",
        "s2_lite_migration_source_guard_v1",
        "s2_lite_migration_source_owner_v1",
    ] {
        let count: i64 = guard
            .query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(count, 0, "{table}");
    }
    assert_eq!(crate::collections::all(&guard).unwrap()[0].name, "One");
    assert!(super::local_authority::load_staged_descriptors(&guard)
        .unwrap()
        .is_empty());
}

#[test]
fn astra_p1_old_legacy_ticket_cannot_put_after_durable_activation_without_refresh() {
    let (c, id) = database(None);
    let root = root_id();
    let binding = super::target_root_binding::resolve_active_target_root_binding_v1(&c, &id, 1)
        .unwrap()
        .binding;
    let ticket = super::durable_persistence::capture_legacy_route_ticket_v1(&c, &binding).unwrap();
    let cloud = Arc::new(Mutex::new(Cloud::default()));
    put_activation(&cloud, serde_json::Value::Null, 1, json!([]));
    let mut remote = remote(&cloud);
    remote.discover(&c).unwrap();
    let mut store = SqliteS2LiteStoreV1::open(&c, &root).unwrap();
    let called = std::cell::Cell::new(false);
    let _ = store.run_legacy_bound_put_v1(&ticket, || {
        called.set(true);
        Ok(())
    });
    assert!(
        !called.get(),
        "PUT must not begin after durable activation, even before refresh"
    );
}
#[test]
fn astra_p1_new_local_update_cannot_capture_stale_live_before_projection_refresh() {
    let (c, id) = database(None);
    seed(&c);
    let cloud = Arc::new(Mutex::new(Cloud::default()));
    finish(&c, &id, &cloud);
    put_collection(&cloud, 1, 1, "Remote conflict");
    let mut remote = remote(&cloud);
    remote.discover(&c).unwrap();
    let mut guard = c.lock().unwrap();
    let read = super::discovery_persistence::load_read_state_v1(&guard, &root_id())
        .unwrap()
        .unwrap();
    assert!(read
        .projection
        .state
        .entities
        .iter()
        .any(|entity| entity.conflict));
    crate::collections::update(
        &mut guard,
        "c1",
        serde_json::from_value(json!({"name":"Offline update","expectedRev":1})).unwrap(),
        "device",
    )
    .unwrap();
    let row = super::local_authority::load_staged_descriptors(&guard)
        .unwrap()
        .into_iter()
        .find(|row| row.entity_id == "c1")
        .unwrap();
    assert_eq!(
        row.causal_anchor,
        super::local_authority::StagingAnchorStateV1::Unavailable
    );
}
#[test]
fn astra_p1_observed_put_verification_mismatch_cannot_be_erased_by_later_exact_get() {
    let (c, id) = database(None);
    seed(&c);
    let cloud = Arc::new(Mutex::new(Cloud {
        mismatch_next_verification: true,
        ..Cloud::default()
    }));
    let mut remote = remote(&cloud);
    let mut state = None;
    for _ in 0..10 {
        let next = execute_migration_step_with_adapter_v1(&c, &mut remote, &id, 1, NOW)
            .unwrap()
            .unwrap();
        let terminal = matches!(
            next.status,
            MigrationStatusV1::RootFrozen | MigrationStatusV1::MigrationComplete
        );
        state = Some(next);
        if terminal {
            break;
        }
    }
    assert_eq!(
        state.unwrap().status,
        MigrationStatusV1::RootFrozen,
        "observed mismatch must not be healed by a second verification GET"
    );
    let root = root_id();
    let mut store = SqliteS2LiteStoreV1::open(&c, &root).unwrap();
    assert!(!store
        .load_root_safety(&root)
        .unwrap()
        .root_fatal_signals
        .is_empty());
}

#[test]
fn direct_legacy_admission_after_discovery_before_refresh_survives_restart() {
    let path = temp_path();
    let (c, id) = database(Some(&path));
    let root = root_id();
    let binding = super::target_root_binding::resolve_active_target_root_binding_v1(&c, &id, 1)
        .unwrap()
        .binding;
    let ticket = super::durable_persistence::capture_legacy_route_ticket_v1(&c, &binding).unwrap();
    let cloud = Arc::new(Mutex::new(Cloud::default()));
    let mut remote = remote(&cloud);
    let mut store = SqliteS2LiteStoreV1::open(&c, &root).unwrap();
    assert!(matches!(
        store.run_legacy_bound_put_v1(&ticket, || Ok(17)).unwrap(),
        super::durable_persistence::LegacyS1PublishAdmissionV1::Executed(17)
    ));
    put_activation(&cloud, serde_json::Value::Null, 1, json!([]));
    remote.discover(&c).unwrap();
    assert_eq!(
        super::durable_persistence::validate_legacy_route_ticket_v1(&c.lock().unwrap(), &ticket)
            .unwrap(),
        super::durable_persistence::LegacyRouteTicketValidationV1::NoLongerLegacy
    );
    drop(c);
    let c = reopen(&path);
    let mut store = SqliteS2LiteStoreV1::open(&c, &root).unwrap();
    let called = std::cell::Cell::new(false);
    assert!(matches!(
        store
            .run_legacy_bound_put_v1(&ticket, || {
                called.set(true);
                Ok(())
            })
            .unwrap(),
        super::durable_persistence::LegacyS1PublishAdmissionV1::RejectedActivation
    ));
    assert!(!called.get());
    assert!(
        store
            .load_root_safety(&root)
            .unwrap()
            .cutover_state
            .remote_s2_activated
    );
    drop(c);
    std::fs::remove_file(path).unwrap();
}

#[test]
fn stale_live_and_absent_capture_after_restart_remain_unavailable_after_refresh() {
    let path = temp_path();
    let (c, id) = database(Some(&path));
    seed(&c);
    let cloud = Arc::new(Mutex::new(Cloud::default()));
    finish(&c, &id, &cloud);
    put_collection(&cloud, 1, 1, "Remote conflict");
    remote(&cloud).discover(&c).unwrap();
    drop(c);
    let c = reopen(&path);
    let original = {
        let mut guard = c.lock().unwrap();
        crate::collections::update(
            &mut guard,
            "c1",
            serde_json::from_value(json!({"name":"Offline", "expectedRev":1})).unwrap(),
            "device",
        )
        .unwrap();
        let created = crate::collections::create(
            &mut guard,
            serde_json::from_value(json!({"name":"New in stale window"})).unwrap(),
            "device",
        )
        .unwrap();
        let rows = super::local_authority::load_staged_descriptors(&guard).unwrap();
        for entity in ["c1", created.id.as_str()] {
            assert_eq!(
                rows.iter()
                    .find(|row| row.entity_id == entity)
                    .unwrap()
                    .causal_anchor,
                super::local_authority::StagingAnchorStateV1::Unavailable
            );
        }
        rows
    };
    let root = root_id();
    SqliteS2LiteStoreV1::open(&c, &root)
        .unwrap()
        .refresh_from_read_authority_v1()
        .unwrap();
    drop(c);
    let c = reopen(&path);
    assert_eq!(
        super::local_authority::load_staged_descriptors(&c.lock().unwrap()).unwrap(),
        original
    );
    drop(c);
    std::fs::remove_file(path).unwrap();
}

#[test]
fn verification_mismatch_on_bootstrap_or_activation_stays_fatal_after_exact_get_and_restart() {
    for prefix in ["writers/", "activations/"] {
        let path = temp_path();
        let (c, id) = database(Some(&path));
        seed(&c);
        let cloud = Arc::new(Mutex::new(Cloud {
            mismatch_on_put_prefix: Some(prefix),
            ..Cloud::default()
        }));
        let mut remote = remote(&cloud);
        let mut frozen = None;
        for _ in 0..10 {
            let state = execute_migration_step_with_adapter_v1(&c, &mut remote, &id, 1, NOW)
                .unwrap()
                .unwrap();
            assert_ne!(state.status, MigrationStatusV1::MigrationComplete);
            if state.status == MigrationStatusV1::RootFrozen {
                frozen = Some(state);
                break;
            }
        }
        let frozen = frozen.expect("verification mismatch freezes migration");
        let root = root_id();
        let fatal = SqliteS2LiteStoreV1::open(&c, &root)
            .unwrap()
            .load_root_safety(&root)
            .unwrap()
            .root_fatal_signals;
        assert!(!fatal.is_empty());
        let (object_path, exact) = cloud.lock().unwrap().puts.last().unwrap().clone();
        assert_eq!(
            super::immutable_publish::ImmutableObjectRemoteV1::get_exact(&mut remote, &object_path),
            super::immutable_publish::RemoteExactGetResultV1::DefinitelyPresent(exact)
        );
        drop(c);
        let c = reopen(&path);
        remote.discover(&c).unwrap();
        let mut store = SqliteS2LiteStoreV1::open(&c, &root).unwrap();
        store.refresh_from_read_authority_v1().unwrap();
        assert_eq!(
            store.load_root_safety(&root).unwrap().root_fatal_signals,
            fatal
        );
        assert_eq!(
            store.load(&root).unwrap().unwrap().status,
            MigrationStatusV1::RootFrozen
        );
        let before = cloud.lock().unwrap().puts.len();
        let result = execute_migration_step_with_adapter_v1(&c, &mut remote, &id, 1, NOW);
        assert!(
            result.is_err()
                || result
                    .unwrap()
                    .is_some_and(|state| state.status == MigrationStatusV1::RootFrozen)
        );
        assert_eq!(cloud.lock().unwrap().puts.len(), before);
        assert_eq!(
            store.load(&root).unwrap().unwrap().migration_id,
            frozen.migration_id
        );
        drop(c);
        std::fs::remove_file(path).unwrap();
    }
}

#[test]
fn prepared_retry_same_path_mismatch_remains_fatal_after_provider_restores_bytes() {
    let path = temp_path();
    let (c, id) = database(Some(&path));
    seed(&c);
    let cloud = Arc::new(Mutex::new(Cloud {
        lost: true,
        unavailable_after_next_put: true,
        ..Cloud::default()
    }));
    let mut remote = remote(&cloud);
    let pending = execute_migration_step_with_adapter_v1(&c, &mut remote, &id, 1, NOW)
        .unwrap()
        .unwrap();
    assert_eq!(pending.status, MigrationStatusV1::StageAPublishing);
    assert!(pending.stage_a[0].receipt.is_none());
    let (object_path, exact) = cloud.lock().unwrap().puts[0].clone();
    cloud
        .lock()
        .unwrap()
        .objects
        .insert(object_path.clone(), b"retry corruption".to_vec());
    let result = execute_migration_step_with_adapter_v1(&c, &mut remote, &id, 1, NOW);
    assert!(
        result.is_err()
            || result
                .unwrap()
                .is_some_and(|state| state.status == MigrationStatusV1::RootFrozen)
    );
    let root = root_id();
    let fatal = SqliteS2LiteStoreV1::open(&c, &root)
        .unwrap()
        .load_root_safety(&root)
        .unwrap()
        .root_fatal_signals;
    assert!(!fatal.is_empty());
    cloud.lock().unwrap().objects.insert(object_path, exact);
    drop(c);
    let c = reopen(&path);
    remote.discover(&c).unwrap();
    let mut store = SqliteS2LiteStoreV1::open(&c, &root).unwrap();
    store.refresh_from_read_authority_v1().unwrap();
    assert_eq!(
        store.load_root_safety(&root).unwrap().root_fatal_signals,
        fatal
    );
    assert_eq!(
        store.load(&root).unwrap().unwrap().status,
        MigrationStatusV1::RootFrozen
    );
    drop(c);
    std::fs::remove_file(path).unwrap();
}

#[test]
fn unchanged_replay_new_read_generation_invalidates_anchor_and_projection_application() {
    let (c, id) = database(None);
    seed(&c);
    let cloud = Arc::new(Mutex::new(Cloud::default()));
    finish(&c, &id, &cloud);
    let root = root_id();
    let mut store = SqliteS2LiteStoreV1::open(&c, &root).unwrap();
    let cached = store.load_materialized_projection().unwrap().unwrap();
    remote(&cloud).discover(&c).unwrap();
    {
        let guard = c.lock().unwrap();
        let current = super::discovery_persistence::load_read_state_v1(&guard, &root)
            .unwrap()
            .unwrap();
        assert_eq!(current.projection.state, cached.state);
        assert!(
            Some(current.discovery.storage_generation) > cached.source_read_discovery_generation
        );
        assert!(matches!(
            super::durable_persistence::admit_applied_projection_for_staging_anchor_v1(
                &guard, &root
            )
            .unwrap(),
            super::durable_persistence::StagingAnchorProjectionAdmissionV1::Unavailable(_)
        ));
    }
    let called = std::cell::Cell::new(false);
    assert!(store
        .run_business_projection_transaction(cached.projection_generation, |_, _| {
            called.set(true);
            Ok(())
        })
        .is_err());
    assert!(!called.get());
    store.refresh_from_read_authority_v1().unwrap();
    let refreshed = store.load_materialized_projection().unwrap().unwrap();
    assert!(refreshed.projection_generation > cached.projection_generation);
    assert_eq!(refreshed.business_projection_applied_generation, None);
}

#[test]
fn original_live_basis_is_not_rewritten_by_conflict_discovery_or_refresh() {
    let (c, id) = database(None);
    seed(&c);
    let cloud = Arc::new(Mutex::new(Cloud::default()));
    finish(&c, &id, &cloud);
    let original = {
        let mut guard = c.lock().unwrap();
        crate::collections::update(
            &mut guard,
            "c1",
            serde_json::from_value(json!({"name":"First local", "expectedRev":1})).unwrap(),
            "device",
        )
        .unwrap();
        super::local_authority::load_staged_descriptors(&guard)
            .unwrap()
            .into_iter()
            .find(|row| row.entity_id == "c1")
            .unwrap()
    };
    assert!(matches!(
        original.causal_anchor,
        super::local_authority::StagingAnchorStateV1::Live { .. }
    ));
    put_collection(&cloud, 1, 1, "Remote conflict");
    remote(&cloud).discover(&c).unwrap();
    let root = root_id();
    SqliteS2LiteStoreV1::open(&c, &root)
        .unwrap()
        .refresh_from_read_authority_v1()
        .unwrap();
    let mut guard = c.lock().unwrap();
    crate::collections::update(
        &mut guard,
        "c1",
        serde_json::from_value(json!({"name":"Repeated local", "expectedRev":2})).unwrap(),
        "device",
    )
    .unwrap();
    let repeated = super::local_authority::load_staged_descriptors(&guard)
        .unwrap()
        .into_iter()
        .find(|row| row.entity_id == "c1")
        .unwrap();
    assert_eq!(repeated.local_mutation_id, original.local_mutation_id);
    assert_eq!(repeated.first_generation, original.first_generation);
    assert_eq!(repeated.causal_anchor, original.causal_anchor);
    assert_eq!(repeated.verified_basis, original.verified_basis);
}

#[test]
fn direct_migration_publish_admission_observes_read_fork_without_refresh() {
    let (c, id) = database(None);
    seed(&c);
    let planned = admit(&c, &id);
    let cloud = Arc::new(Mutex::new(Cloud::default()));
    put_collection(&cloud, 1, 1, "First writer sequence");
    put_collection(&cloud, 1, 2, "Forked writer sequence");
    remote(&cloud).discover(&c).unwrap();
    let root = root_id();
    let mut store = SqliteS2LiteStoreV1::open(&c, &root).unwrap();
    let called = std::cell::Cell::new(false);
    let rejected = store
        .run_publish_exclusive(
            &root,
            &planned.migration_id,
            planned.generation,
            None,
            || {
                called.set(true);
                Ok(())
            },
        )
        .unwrap();
    assert!(
        matches!(rejected, super::migration_orchestration::PublishExclusiveResultV1::Rejected(state) if state.status == MigrationStatusV1::RootFrozen)
    );
    assert!(!called.get());
    assert!(!store
        .load_root_safety(&root)
        .unwrap()
        .root_fatal_signals
        .is_empty());
}
