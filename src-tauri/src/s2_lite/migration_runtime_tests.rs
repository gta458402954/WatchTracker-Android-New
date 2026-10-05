use super::durable_persistence::SqliteS2LiteStoreV1;
use super::migration_orchestration::{MigrationStateStoreV1, MigrationStatusV1};
use super::migration_runtime::*;
use super::webdav_adapter::*;
use async_trait::async_trait;
use reqwest::{Method, Url};
use rusqlite::Connection;
use serde_json::{json, Value};
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

fn corrupt_frozen_batch_entity_id(
    conn: &Connection,
    entity_kind: &str,
    entity_id: &str,
    replacement: &str,
) {
    let bytes: Vec<u8> = conn
        .query_row(
            "SELECT state_json FROM s2_lite_outbound_batch_v1",
            [],
            |row| row.get(0),
        )
        .unwrap();
    let mut persisted: Value = serde_json::from_slice(&bytes).unwrap();
    let mutation = persisted["payload"]["mutations"]
        .as_array_mut()
        .unwrap()
        .iter_mut()
        .find(|mutation| mutation["entityKind"] == entity_kind && mutation["entityId"] == entity_id)
        .unwrap();
    mutation["entityId"] = json!(replacement);
    let corrupted = serde_json::to_vec(&persisted).unwrap();
    conn.execute(
        "UPDATE s2_lite_outbound_batch_v1 SET state_json=?1",
        [&corrupted],
    )
    .unwrap();
}

fn mutate_frozen_batch(conn: &Connection, update: impl FnOnce(&mut Vec<Value>)) {
    let bytes: Vec<u8> = conn
        .query_row(
            "SELECT state_json FROM s2_lite_outbound_batch_v1",
            [],
            |row| row.get(0),
        )
        .unwrap();
    let mut persisted: Value = serde_json::from_slice(&bytes).unwrap();
    update(persisted["payload"]["mutations"].as_array_mut().unwrap());
    conn.execute(
        "UPDATE s2_lite_outbound_batch_v1 SET state_json=?1",
        [serde_json::to_vec(&persisted).unwrap()],
    )
    .unwrap();
}

fn create_and_freeze_all_ordinary_entity_kinds(
    c: &Mutex<Connection>,
    id: &str,
) -> Vec<(String, String)> {
    let record_id = uuid::Uuid::new_v4().to_string();
    let mut guard = c.lock().unwrap();
    crate::db_atomic_crud::insert_record_atomic(
        &mut guard,
        serde_json::from_value(json!({
            "id":record_id, "originalName":"Frozen identities", "chineseName":"Frozen identities",
            "progress":"", "totalEpisodes":3, "status":"未看", "platform":"", "notes":"",
            "createdAt":NOW, "mediaType":"剧集", "episodeTrackingEnabled":true, "nextEpisode":1
        }))
        .unwrap(),
        "device",
    )
    .unwrap();
    crate::episode_history::set_next(&mut guard, &record_id, Some(2), 1, "device").unwrap();
    let collection = crate::collections::create(
        &mut guard,
        serde_json::from_value(json!({"name":"Frozen identity parent"})).unwrap(),
        "device",
    )
    .unwrap();
    crate::collections::add_members(
        &mut guard,
        &collection.id,
        vec![record_id],
        "manual",
        collection.rev,
        "device",
    )
    .unwrap();
    drop(guard);
    match super::outbound_freeze::freeze_active_outbound_v1(c, id, 1, NOW).unwrap() {
        super::outbound_freeze::OutboundFreezeResultV1::Frozen { batch, .. } => batch
            .mutations
            .into_iter()
            .map(|mutation| (mutation.entity_kind, mutation.entity_id))
            .collect(),
        other => panic!("{other:?}"),
    }
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

fn ordinary(
    c: &Mutex<Connection>,
    id: &str,
    cloud: &Arc<Mutex<Cloud>>,
) -> super::ordinary_runtime::OrdinaryCycleResultV1 {
    super::ordinary_runtime::run_ordinary_cycle_v1(c, &mut remote(cloud), id, 1, NOW).unwrap()
}
#[test]
fn i65_first_ordinary_publication_preserves_capture_identity_and_retires_only_after_receipt() {
    let (c, id) = database(None);
    seed(&c);
    let cloud = Arc::new(Mutex::new(Cloud::default()));
    finish(&c, &id, &cloud);
    let captured = {
        let mut guard = c.lock().unwrap();
        crate::collections::update(
            &mut guard,
            "c1",
            serde_json::from_value(json!({"name":"Ordinary","expectedRev":1})).unwrap(),
            "device",
        )
        .unwrap();
        super::local_authority::load_staged_descriptors(&guard).unwrap()[0].clone()
    };
    let root = root_id();
    let before = SqliteS2LiteStoreV1::open(&c, &root)
        .unwrap()
        .load_desktop_root_state()
        .unwrap()
        .unwrap();
    assert_eq!(
        ordinary(&c, &id, &cloud),
        super::ordinary_runtime::OrdinaryCycleResultV1::Success
    );
    let mut store = SqliteS2LiteStoreV1::open(&c, &root).unwrap();
    let after = store.load_desktop_root_state().unwrap().unwrap();
    assert_eq!(after.local_writer_id, before.local_writer_id);
    assert_eq!(after.next_writer_sequence, before.next_writer_sequence + 1);
    let (path, bytes) = cloud.lock().unwrap().puts.last().unwrap().clone();
    let commit = super::causal::decode_frozen_wire_commit_v1(&bytes).unwrap();
    assert_eq!(commit.previous_writer_commit, before.writer_head);
    assert_eq!(
        commit.mutations[0].local_mutation_id,
        captured.local_mutation_id
    );
    assert!(store.load_published_receipt(&path).unwrap().is_some());
    assert!(
        super::local_authority::load_staged_descriptors(&c.lock().unwrap())
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        ordinary(&c, &id, &cloud),
        super::ordinary_runtime::OrdinaryCycleResultV1::Success
    );
    let retried = store.load_desktop_root_state().unwrap().unwrap();
    assert_eq!(retried.next_writer_sequence, after.next_writer_sequence);
    assert_eq!(retried.writer_head, after.writer_head);
    assert_eq!(retried.local_writer_id, after.local_writer_id);
}
#[test]
fn i65_collection_tombstone_is_published_from_durable_evidence_after_source_deletion() {
    let (c, id) = database(None);
    seed(&c);
    let cloud = Arc::new(Mutex::new(Cloud::default()));
    finish(&c, &id, &cloud);
    {
        let mut guard = c.lock().unwrap();
        crate::collections::delete(&mut guard, "c1", 1, "device").unwrap();
    }
    assert_eq!(
        ordinary(&c, &id, &cloud),
        super::ordinary_runtime::OrdinaryCycleResultV1::Success
    );
    let commit =
        super::causal::decode_frozen_wire_commit_v1(&cloud.lock().unwrap().puts.last().unwrap().1)
            .unwrap();
    assert_eq!(commit.mutations[0].entity_type, "collection");
    assert!(crate::collections::all(&c.lock().unwrap())
        .unwrap()
        .is_empty());
}

fn update_ordinary_collection(c: &Mutex<Connection>, name: &str, rev: i64) {
    crate::collections::update(
        &mut c.lock().unwrap(),
        "c1",
        serde_json::from_value(json!({"name":name,"expectedRev":rev})).unwrap(),
        "device",
    )
    .unwrap();
}
fn mobile(
    c: &Mutex<Connection>,
    id: &str,
    cloud: &Arc<Mutex<Cloud>>,
    coordinator: &super::root_coordinator::RootExecutionCoordinatorV1,
    automatic: bool,
    attempt: Option<&str>,
) -> super::ordinary_runtime::OrdinaryCycleResultV1 {
    super::ordinary_runtime::run_mobile_sync_with_adapter_v1(
        c,
        coordinator,
        &mut remote(cloud),
        &super::ordinary_runtime::MobileSyncAdmissionV1 {
            target_id: id.into(),
            target_epoch: 1,
            automatic,
            expected_attempt_at: attempt.map(str::to_owned),
        },
        NOW,
    )
    .unwrap()
}
#[test]
fn i65_coalesced_mutations_publish_one_batch_with_stable_ids() {
    let (c, id) = database(None);
    seed(&c);
    let cloud = Arc::new(Mutex::new(Cloud::default()));
    finish(&c, &id, &cloud);
    update_ordinary_collection(&c, "First", 1);
    let first =
        super::local_authority::load_staged_descriptors(&c.lock().unwrap()).unwrap()[0].clone();
    update_ordinary_collection(&c, "Final", 2);
    crate::collections::create(
        &mut c.lock().unwrap(),
        serde_json::from_value(json!({"name":"Second entity"})).unwrap(),
        "device",
    )
    .unwrap();
    let before = cloud.lock().unwrap().puts.len();
    assert_eq!(
        ordinary(&c, &id, &cloud),
        super::ordinary_runtime::OrdinaryCycleResultV1::Success
    );
    assert_eq!(cloud.lock().unwrap().puts.len(), before + 1);
    let commit =
        super::causal::decode_frozen_wire_commit_v1(&cloud.lock().unwrap().puts.last().unwrap().1)
            .unwrap();
    assert_eq!(commit.mutations.len(), 2);
    let changed = commit
        .mutations
        .iter()
        .find(|mutation| mutation.local_mutation_id == first.local_mutation_id)
        .unwrap();
    assert_eq!(changed.value["name"], "Final");
}
#[test]
fn i65_restart_captured_before_prepare_keeps_mutation_and_writer_identity() {
    let path = temp_path();
    let (c, id) = database(Some(&path));
    seed(&c);
    let cloud = Arc::new(Mutex::new(Cloud::default()));
    finish(&c, &id, &cloud);
    update_ordinary_collection(&c, "Restarted", 1);
    let original = super::local_authority::load_staged_descriptors(&c.lock().unwrap()).unwrap();
    drop(c);
    let c = reopen(&path);
    assert_eq!(
        super::local_authority::load_staged_descriptors(&c.lock().unwrap()).unwrap(),
        original
    );
    assert_eq!(
        ordinary(&c, &id, &cloud),
        super::ordinary_runtime::OrdinaryCycleResultV1::Success
    );
    let commit =
        super::causal::decode_frozen_wire_commit_v1(&cloud.lock().unwrap().puts.last().unwrap().1)
            .unwrap();
    assert_eq!(
        commit.mutations[0].local_mutation_id,
        original[0].local_mutation_id
    );
    drop(c);
    std::fs::remove_file(path).unwrap();
}
#[test]
fn i65_restart_after_prepared_before_put_reuses_exact_identity() {
    let path = temp_path();
    let (c, id) = database(Some(&path));
    seed(&c);
    let cloud = Arc::new(Mutex::new(Cloud::default()));
    finish(&c, &id, &cloud);
    update_ordinary_collection(&c, "Prepared", 1);
    let (batch, intent) =
        match super::outbound_freeze::freeze_active_outbound_v1(&c, &id, 1, NOW).unwrap() {
            super::outbound_freeze::OutboundFreezeResultV1::Frozen { batch, intent } => {
                (*batch, *intent)
            }
            other => panic!("{other:?}"),
        };
    let before = cloud.lock().unwrap().puts.len();
    drop(c);
    let c = reopen(&path);
    assert_eq!(
        ordinary(&c, &id, &cloud),
        super::ordinary_runtime::OrdinaryCycleResultV1::Success
    );
    assert_eq!(
        cloud.lock().unwrap().puts[before],
        (intent.remote_path.clone(), intent.exact_bytes)
    );
    let root = root_id();
    let mut store = SqliteS2LiteStoreV1::open(&c, &root).unwrap();
    assert_eq!(
        store
            .load_desktop_root_state()
            .unwrap()
            .unwrap()
            .writer_head,
        Some(batch.commit_ref)
    );
    drop(c);
    std::fs::remove_file(path).unwrap();
}
#[test]
fn i65_reservation_and_intent_failure_rolls_back_without_put_or_sequence_loss() {
    let (c, id) = database(None);
    seed(&c);
    let cloud = Arc::new(Mutex::new(Cloud::default()));
    finish(&c, &id, &cloud);
    update_ordinary_collection(&c, "Atomic prepare", 1);
    let root = root_id();
    let before = SqliteS2LiteStoreV1::open(&c, &root)
        .unwrap()
        .load_desktop_root_state()
        .unwrap()
        .unwrap();
    let puts = cloud.lock().unwrap().puts.len();
    c.lock().unwrap().execute_batch("CREATE TRIGGER fail_ordinary_intent BEFORE INSERT ON s2_lite_prepared_intent_v1 BEGIN SELECT RAISE(ABORT,'process death'); END").unwrap();
    assert!(
        super::ordinary_runtime::run_ordinary_cycle_v1(&c, &mut remote(&cloud), &id, 1, NOW)
            .is_err()
    );
    let mut store = SqliteS2LiteStoreV1::open(&c, &root).unwrap();
    let after = store.load_desktop_root_state().unwrap().unwrap();
    assert_eq!(after.next_writer_sequence, before.next_writer_sequence);
    assert_eq!(after.writer_head, before.writer_head);
    assert!(store.load_unfinished_outbound_batch().unwrap().is_none());
    assert_eq!(cloud.lock().unwrap().puts.len(), puts);
    assert_eq!(
        super::local_authority::load_staged_descriptors(&c.lock().unwrap())
            .unwrap()
            .len(),
        1
    );
}
#[test]
fn i65_lost_put_response_and_failed_verification_retry_reuse_path_bytes_and_sequence() {
    let path = temp_path();
    let (c, id) = database(Some(&path));
    seed(&c);
    let cloud = Arc::new(Mutex::new(Cloud::default()));
    finish(&c, &id, &cloud);
    update_ordinary_collection(&c, "Retry", 1);
    {
        let mut fake = cloud.lock().unwrap();
        fake.lost = true;
        fake.unavailable_after_next_put = true;
    }
    assert_eq!(
        ordinary(&c, &id, &cloud),
        super::ordinary_runtime::OrdinaryCycleResultV1::Pending
    );
    let root = root_id();
    let mut store = SqliteS2LiteStoreV1::open(&c, &root).unwrap();
    let batch = store.load_unfinished_outbound_batch().unwrap().unwrap();
    let intent = store
        .load_prepared_intent(&batch.prepared_intent_path)
        .unwrap()
        .unwrap();
    let before = cloud.lock().unwrap().puts.len();
    drop(c);
    let c = reopen(&path);
    assert_eq!(
        ordinary(&c, &id, &cloud),
        super::ordinary_runtime::OrdinaryCycleResultV1::Success
    );
    assert_eq!(cloud.lock().unwrap().puts.len(), before);
    let mut store = SqliteS2LiteStoreV1::open(&c, &root).unwrap();
    assert_eq!(
        store
            .load_prepared_intent(&intent.remote_path)
            .unwrap()
            .unwrap(),
        intent
    );
    assert_eq!(
        store
            .load_desktop_root_state()
            .unwrap()
            .unwrap()
            .writer_head,
        Some(batch.commit_ref)
    );
    drop(c);
    std::fs::remove_file(path).unwrap();
}
#[test]
fn i65_verification_before_receipt_and_receipt_before_retirement_restart_safely() {
    for (table, action) in [
        ("s2_lite_published_receipt_v1", "INSERT"),
        ("s2_lite_local_staging_descriptor_v1", "DELETE"),
    ] {
        let path = temp_path();
        let (c, id) = database(Some(&path));
        seed(&c);
        let cloud = Arc::new(Mutex::new(Cloud::default()));
        finish(&c, &id, &cloud);
        update_ordinary_collection(&c, "Crash boundary", 1);
        c.lock().unwrap().execute_batch(&format!("CREATE TRIGGER fail_ordinary_completion BEFORE {action} ON {table} BEGIN SELECT RAISE(ABORT,'process death'); END")).unwrap();
        assert!(super::ordinary_runtime::run_ordinary_cycle_v1(
            &c,
            &mut remote(&cloud),
            &id,
            1,
            NOW
        )
        .is_err());
        let root = root_id();
        let batch = SqliteS2LiteStoreV1::open(&c, &root)
            .unwrap()
            .load_unfinished_outbound_batch()
            .unwrap()
            .unwrap();
        let puts = cloud.lock().unwrap().puts.len();
        assert!(
            !super::local_authority::load_staged_descriptors(&c.lock().unwrap())
                .unwrap()
                .is_empty()
        );
        drop(c);
        let c = reopen(&path);
        c.lock()
            .unwrap()
            .execute_batch("DROP TRIGGER fail_ordinary_completion")
            .unwrap();
        assert_eq!(
            ordinary(&c, &id, &cloud),
            super::ordinary_runtime::OrdinaryCycleResultV1::Success
        );
        assert_eq!(cloud.lock().unwrap().puts.len(), puts);
        assert_eq!(
            SqliteS2LiteStoreV1::open(&c, &root)
                .unwrap()
                .load_desktop_root_state()
                .unwrap()
                .unwrap()
                .writer_head,
            Some(batch.commit_ref)
        );
        drop(c);
        std::fs::remove_file(path).unwrap();
    }
}
#[test]
fn i65_episode_progress_two_to_five_publishes_each_completion_independently() {
    let (c, id) = database(None);
    {
        let guard = c.lock().unwrap();
        crate::db::insert_record(&guard,serde_json::from_value(json!({"id":"r1","originalName":"Series","chineseName":"Series","progress":"","totalEpisodes":6,"status":"未看","platform":"","notes":"","createdAt":NOW,"mediaType":"剧集","rev":3,"revActor":"seed","episodeTrackingEnabled":true,"nextEpisode":2})).unwrap()).unwrap();
    }
    let cloud = Arc::new(Mutex::new(Cloud::default()));
    finish(&c, &id, &cloud);
    crate::episode_history::set_next(&mut c.lock().unwrap(), "r1", Some(5), 3, "device").unwrap();
    let captured = super::local_authority::load_staged_descriptors(&c.lock().unwrap()).unwrap();
    assert_eq!(
        captured
            .iter()
            .filter(|row| row.entity_kind == "episode-completion")
            .count(),
        3
    );
    assert_eq!(
        ordinary(&c, &id, &cloud),
        super::ordinary_runtime::OrdinaryCycleResultV1::Success
    );
    let commit =
        super::causal::decode_frozen_wire_commit_v1(&cloud.lock().unwrap().puts.last().unwrap().1)
            .unwrap();
    assert_eq!(
        commit
            .mutations
            .iter()
            .filter(|mutation| mutation.entity_type == "episode-completion")
            .count(),
        3
    );
    for row in captured {
        assert!(commit
            .mutations
            .iter()
            .any(|mutation| mutation.local_mutation_id == row.local_mutation_id));
    }
}
#[test]
fn i65_all_entity_tombstones_publish_after_rows_and_s1_staging_are_gone() {
    let path = temp_path();
    let (c, id) = database(Some(&path));
    seed_all(&c);
    let cloud = Arc::new(Mutex::new(Cloud::default()));
    finish(&c, &id, &cloud);
    {
        let mut guard = c.lock().unwrap();
        crate::db_atomic_crud::delete_record_atomic(&mut guard, "r1", "device").unwrap();
        crate::collections::delete(&mut guard, "c1", 2, "device").unwrap();
        crate::sync_staging::set_staging_for_target(
            &guard,
            &id,
            &crate::sync_staging::SyncStaging::default(),
        )
        .unwrap();
    }
    let captured = super::local_authority::load_staged_descriptors(&c.lock().unwrap()).unwrap();
    assert!(captured.iter().all(|row| row.operation == "delete"));
    assert_eq!(captured.len(), 4);
    drop(c);
    let c = reopen(&path);
    assert_eq!(
        ordinary(&c, &id, &cloud),
        super::ordinary_runtime::OrdinaryCycleResultV1::Success
    );
    let commit =
        super::causal::decode_frozen_wire_commit_v1(&cloud.lock().unwrap().puts.last().unwrap().1)
            .unwrap();
    assert_eq!(commit.mutations.len(), 4);
    for row in captured {
        assert!(commit
            .mutations
            .iter()
            .any(
                |mutation| mutation.local_mutation_id == row.local_mutation_id
                    && mutation.value["revActor"] == "device"
                    && mutation.changed_fields == ["$tombstone"]
            ));
    }
    drop(c);
    std::fs::remove_file(path).unwrap();
}
#[test]
fn i65_online_foreground_race_serializes_one_publication() {
    let (c, id) = database(None);
    seed(&c);
    let cloud = Arc::new(Mutex::new(Cloud::default()));
    finish(&c, &id, &cloud);
    update_ordinary_collection(&c, "Raced events", 1);
    let before = cloud.lock().unwrap().puts.len();
    let c = Arc::new(c);
    let coordinator = Arc::new(super::root_coordinator::RootExecutionCoordinatorV1::default());
    let barrier = Arc::new(std::sync::Barrier::new(3));
    let mut threads = vec![];
    for _ in 0..2 {
        let c = c.clone();
        let id = id.clone();
        let cloud = cloud.clone();
        let coordinator = coordinator.clone();
        let barrier = barrier.clone();
        threads.push(std::thread::spawn(move || {
            barrier.wait();
            mobile(&c, &id, &cloud, &coordinator, true, None)
        }));
    }
    barrier.wait();
    let results = threads
        .into_iter()
        .map(|thread| thread.join().unwrap())
        .collect::<Vec<_>>();
    assert!(results.contains(&super::ordinary_runtime::OrdinaryCycleResultV1::Success));
    assert!(results.contains(&super::ordinary_runtime::OrdinaryCycleResultV1::AutomaticSkipped));
    assert_eq!(cloud.lock().unwrap().puts.len(), before + 1);
}
#[test]
fn i65_stale_retry_after_newer_manual_success_cannot_override_bookkeeping() {
    let (c, id) = database(None);
    seed(&c);
    let cloud = Arc::new(Mutex::new(Cloud::default()));
    finish(&c, &id, &cloud);
    update_ordinary_collection(&c, "Manual wins", 1);
    let coordinator = super::root_coordinator::RootExecutionCoordinatorV1::default();
    assert_eq!(
        mobile(&c, &id, &cloud, &coordinator, false, None),
        super::ordinary_runtime::OrdinaryCycleResultV1::Success
    );
    let before = crate::sync_state::runtime_state(&c.lock().unwrap())
        .unwrap()
        .scheduler;
    assert_eq!(
        mobile(&c, &id, &cloud, &coordinator, true, None),
        super::ordinary_runtime::OrdinaryCycleResultV1::AutomaticSkipped
    );
    assert_eq!(
        crate::sync_state::runtime_state(&c.lock().unwrap())
            .unwrap()
            .scheduler,
        before
    );
}
#[test]
fn i65_fatal_during_ordinary_verification_retains_local_capture_and_freezes_manual_retry() {
    let (c, id) = database(None);
    seed(&c);
    let cloud = Arc::new(Mutex::new(Cloud::default()));
    finish(&c, &id, &cloud);
    update_ordinary_collection(&c, "Fatal", 1);
    cloud.lock().unwrap().mismatch_on_put_prefix = Some("writers/");
    let coordinator = super::root_coordinator::RootExecutionCoordinatorV1::default();
    assert_eq!(
        mobile(&c, &id, &cloud, &coordinator, false, None),
        super::ordinary_runtime::OrdinaryCycleResultV1::ReadOnlyFrozen
    );
    let puts = cloud.lock().unwrap().puts.len();
    assert!(
        !super::local_authority::load_staged_descriptors(&c.lock().unwrap())
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        mobile(&c, &id, &cloud, &coordinator, false, None),
        super::ordinary_runtime::OrdinaryCycleResultV1::ReadOnlyFrozen
    );
    assert_eq!(cloud.lock().unwrap().puts.len(), puts);
}
#[test]
fn i65_remote_fork_before_publication_blocks_all_writer_puts() {
    let (c, id) = database(None);
    seed(&c);
    let cloud = Arc::new(Mutex::new(Cloud::default()));
    finish(&c, &id, &cloud);
    update_ordinary_collection(&c, "Fork gate", 1);
    put_collection(&cloud, 1, 1, "Fork A");
    put_collection(&cloud, 1, 2, "Fork B");
    let puts = cloud.lock().unwrap().puts.len();
    assert_eq!(
        ordinary(&c, &id, &cloud),
        super::ordinary_runtime::OrdinaryCycleResultV1::ReadOnlyFrozen
    );
    assert_eq!(cloud.lock().unwrap().puts.len(), puts);
}
#[test]
fn i65_pre_cutover_legacy_route_and_post_cutover_permanent_s1_gate() {
    let (c, id) = database(None);
    seed(&c);
    let cloud = Arc::new(Mutex::new(Cloud::default()));
    assert_eq!(
        ordinary(&c, &id, &cloud),
        super::ordinary_runtime::OrdinaryCycleResultV1::LegacyS1Required
    );
    assert!(cloud.lock().unwrap().puts.is_empty());
    finish(&c, &id, &cloud);
    let called = std::cell::Cell::new(false);
    assert!(
        run_legacy_put_with_adapter_v1(&c, &mut remote(&cloud), &id, 1, || {
            called.set(true);
            Ok(())
        })
        .is_err()
    );
    assert!(!called.get());
}
#[test]
fn i65_pending_backoff_survives_restart_and_cannot_report_success() {
    let path = temp_path();
    let (c, id) = database(Some(&path));
    seed(&c);
    let cloud = Arc::new(Mutex::new(Cloud::default()));
    finish(&c, &id, &cloud);
    update_ordinary_collection(&c, "Pending", 1);
    cloud.lock().unwrap().deny_object_get = true;
    let coordinator = super::root_coordinator::RootExecutionCoordinatorV1::default();
    assert_eq!(
        mobile(&c, &id, &cloud, &coordinator, false, None),
        super::ordinary_runtime::OrdinaryCycleResultV1::Pending
    );
    let state = crate::sync_state::runtime_state(&c.lock().unwrap())
        .unwrap()
        .scheduler;
    assert!(state.last_success_at.is_none());
    assert!(state.next_attempt_at.is_some());
    drop(c);
    let c = reopen(&path);
    assert_eq!(
        crate::sync_state::runtime_state(&c.lock().unwrap())
            .unwrap()
            .scheduler,
        state
    );
    assert_eq!(
        mobile(
            &c,
            &id,
            &cloud,
            &coordinator,
            true,
            state.last_attempt_at.as_deref()
        ),
        super::ordinary_runtime::OrdinaryCycleResultV1::AutomaticSkipped
    );
    drop(c);
    std::fs::remove_file(path).unwrap();
}
#[test]
fn i65_no_semantic_change_retires_without_allocating_writer_sequence() {
    let (c, id) = database(None);
    seed(&c);
    let cloud = Arc::new(Mutex::new(Cloud::default()));
    finish(&c, &id, &cloud);
    update_ordinary_collection(&c, "One", 1);
    let puts = cloud.lock().unwrap().puts.len();
    assert_eq!(
        ordinary(&c, &id, &cloud),
        super::ordinary_runtime::OrdinaryCycleResultV1::Success
    );
    assert_eq!(cloud.lock().unwrap().puts.len(), puts);
    assert!(
        super::local_authority::load_staged_descriptors(&c.lock().unwrap())
            .unwrap()
            .is_empty()
    );
}

#[test]
fn i65_new_local_generation_after_freeze_is_not_retired_by_older_receipt() {
    let (c, id) = database(None);
    seed(&c);
    let cloud = Arc::new(Mutex::new(Cloud::default()));
    finish(&c, &id, &cloud);
    update_ordinary_collection(&c, "Frozen value", 1);
    let root = root_id();
    let batch = match super::outbound_freeze::freeze_active_outbound_v1(&c, &id, 1, NOW).unwrap() {
        super::outbound_freeze::OutboundFreezeResultV1::Frozen { batch, .. } => batch,
        other => panic!("{other:?}"),
    };
    update_ordinary_collection(&c, "Newer local value", 2);
    let newer =
        super::local_authority::load_staged_descriptors(&c.lock().unwrap()).unwrap()[0].clone();
    assert_eq!(
        ordinary(&c, &id, &cloud),
        super::ordinary_runtime::OrdinaryCycleResultV1::Pending
    );
    assert_eq!(
        super::local_authority::load_staged_descriptors(&c.lock().unwrap()).unwrap()[0],
        newer
    );
    let mut store = SqliteS2LiteStoreV1::open(&c, &root).unwrap();
    assert_eq!(
        store
            .load_desktop_root_state()
            .unwrap()
            .unwrap()
            .writer_head,
        Some(batch.commit_ref)
    );
    let puts = cloud.lock().unwrap().puts.len();
    assert_eq!(
        ordinary(&c, &id, &cloud),
        super::ordinary_runtime::OrdinaryCycleResultV1::Pending
    );
    assert_eq!(cloud.lock().unwrap().puts.len(), puts);
    assert_eq!(
        crate::collections::all(&c.lock().unwrap()).unwrap()[0].name,
        "Newer local value"
    );
}
#[test]
fn i65_target_epoch_replacement_after_freeze_rejects_direct_publication_callback() {
    let (c, id) = database(None);
    seed(&c);
    let cloud = Arc::new(Mutex::new(Cloud::default()));
    finish(&c, &id, &cloud);
    update_ordinary_collection(&c, "Epoch protected", 1);
    let intent = match super::outbound_freeze::freeze_active_outbound_v1(&c, &id, 1, NOW).unwrap() {
        super::outbound_freeze::OutboundFreezeResultV1::Frozen { intent, .. } => intent,
        other => panic!("{other:?}"),
    };
    c.lock().unwrap().execute("UPDATE settings SET value=json_set(value,'$.targetEpoch',2) WHERE key='sync_targets_v1'",[]).unwrap();
    let root = root_id();
    let mut store = SqliteS2LiteStoreV1::open(&c, &root).unwrap();
    let called = std::cell::Cell::new(false);
    let result = store.run_ordinary_publish_exclusive(&root, &intent, || {
        called.set(true);
        Ok(())
    });
    assert!(!called.get());
    assert!(
        result.is_err()
            || matches!(
                result.unwrap(),
                super::durable_persistence::OrdinaryPublishExclusiveResultV1::RejectedAuthority
            )
    );
}
#[test]
fn i65_foreign_commit_at_reserved_own_writer_sequence_freezes_before_local_put() {
    let path = temp_path();
    let (c, id) = database(Some(&path));
    seed(&c);
    let cloud = Arc::new(Mutex::new(Cloud::default()));
    finish(&c, &id, &cloud);
    update_ordinary_collection(&c, "Local pending", 1);
    let intent = match super::outbound_freeze::freeze_active_outbound_v1(&c, &id, 1, NOW).unwrap() {
        super::outbound_freeze::OutboundFreezeResultV1::Frozen { intent, .. } => intent,
        other => panic!("{other:?}"),
    };
    let mut foreign = super::causal::parse_frozen_json_value_v1(&intent.exact_bytes).unwrap();
    foreign["commitId"] = json!(uuid::Uuid::new_v4().to_string());
    foreign["mutations"][0]["localMutationId"] = json!(uuid::Uuid::new_v4().to_string());
    foreign["mutations"][0]["value"]["name"] = json!("Foreign sequence");
    foreign["mutations"][0]["value"]["normalizedName"] = json!("foreign sequence");
    let foreign = super::immutable_publish::prepare_commit_intent_v1(
        &super::canonical::jcs_bytes(&foreign).unwrap(),
        NOW,
    )
    .unwrap();
    cloud
        .lock()
        .unwrap()
        .objects
        .insert(foreign.remote_path.clone(), foreign.exact_bytes);
    let puts = cloud.lock().unwrap().puts.len();
    assert_eq!(
        ordinary(&c, &id, &cloud),
        super::ordinary_runtime::OrdinaryCycleResultV1::ReadOnlyFrozen
    );
    assert_eq!(cloud.lock().unwrap().puts.len(), puts);
    drop(c);
    let c = reopen(&path);
    let root = root_id();
    let mut store = SqliteS2LiteStoreV1::open(&c, &root).unwrap();
    assert!(store
        .load_root_safety(&root)
        .unwrap()
        .root_fatal_signals
        .iter()
        .any(|fatal| fatal.code == "S2_LOCAL_WRITER_OWNERSHIP_MISMATCH"));
    assert_eq!(
        store
            .load_prepared_intent(&intent.remote_path)
            .unwrap()
            .unwrap(),
        *intent
    );
    assert!(
        super::discovery_persistence::load_read_state_v1(&c.lock().unwrap(), &root)
            .unwrap()
            .unwrap()
            .discovery
            .state
            .verified_objects
            .iter()
            .any(|object| object.path == foreign.remote_path)
    );
    drop(c);
    std::fs::remove_file(path).unwrap();
}
#[test]
fn i65_final_success_cannot_overwrite_newer_local_capture_or_durable_fatal() {
    for fatal in [false, true] {
        let (c, id) = database(None);
        seed(&c);
        let cloud = Arc::new(Mutex::new(Cloud::default()));
        finish(&c, &id, &cloud);
        let root = root_id();
        let binding = super::target_root_binding::resolve_active_target_root_binding_v1(&c, &id, 1)
            .unwrap()
            .binding;
        if fatal {
            SqliteS2LiteStoreV1::open(&c, &root)
                .unwrap()
                .persist_root_fatal(&root, "late_fatal")
                .unwrap();
        } else {
            update_ordinary_collection(&c, "Late local write", 1);
        }
        let result = crate::sync_state::record_mobile_s2_result_v1(
            &mut c.lock().unwrap(),
            &binding,
            super::ordinary_runtime::OrdinaryCycleResultV1::Success,
        )
        .unwrap();
        assert_ne!(
            result,
            super::ordinary_runtime::OrdinaryCycleResultV1::Success
        );
        assert!(crate::sync_state::runtime_state(&c.lock().unwrap())
            .unwrap()
            .scheduler
            .last_success_at
            .is_none());
    }
}
#[test]
fn i65_late_s1_failure_cannot_overwrite_s2_success() {
    let (c, id) = database(None);
    seed(&c);
    let cloud = Arc::new(Mutex::new(Cloud::default()));
    finish(&c, &id, &cloud);
    update_ordinary_collection(&c, "S2 wins", 1);
    let coordinator = super::root_coordinator::RootExecutionCoordinatorV1::default();
    assert_eq!(
        mobile(&c, &id, &cloud, &coordinator, false, None),
        super::ordinary_runtime::OrdinaryCycleResultV1::Success
    );
    let guard = c.lock().unwrap();
    let before = crate::sync_state::runtime_state(&guard).unwrap().scheduler;
    assert!(
        crate::sync_state::record_failure(&guard, "network", None, Some(&id), Some(1)).is_err()
    );
    assert_eq!(
        crate::sync_state::runtime_state(&guard).unwrap().scheduler,
        before
    );
}

#[test]
fn i65_resume_after_pause_admits_pending_retry_without_rewriting_basis() {
    let (c, id) = database(None);
    seed(&c);
    let cloud = Arc::new(Mutex::new(Cloud::default()));
    finish(&c, &id, &cloud);
    let binding = super::target_root_binding::resolve_active_target_root_binding_v1(&c, &id, 1)
        .unwrap()
        .binding;
    let mut guard = c.lock().unwrap();
    crate::sync_state::record_mobile_s2_result_v1(
        &mut guard,
        &binding,
        super::ordinary_runtime::OrdinaryCycleResultV1::Pending,
    )
    .unwrap();
    crate::sync_state::set_paused(&guard, true, Some(&id), Some(1)).unwrap();
    crate::sync_state::set_paused(&guard, false, Some(&id), Some(1)).unwrap();
    let scheduler = crate::sync_state::runtime_state(&guard).unwrap().scheduler;
    assert_eq!(scheduler.last_error_code.as_deref(), Some("s2_pending"));
    assert!(crate::sync_state::admit_mobile_automatic_v1(
        &mut guard,
        &id,
        1,
        scheduler.last_attempt_at.as_deref()
    )
    .unwrap());
}
#[test]
fn i65_historical_own_sequence_alternative_freezes_without_prior_remote_observation() {
    let path = temp_path();
    let (c, id) = database(Some(&path));
    seed(&c);
    let cloud = Arc::new(Mutex::new(Cloud::default()));
    finish(&c, &id, &cloud);
    update_ordinary_collection(&c, "Published local", 1);
    c.lock().unwrap().execute_batch("CREATE TRIGGER fail_ack BEFORE DELETE ON s2_lite_local_staging_descriptor_v1 BEGIN SELECT RAISE(ABORT,'process death'); END").unwrap();
    assert!(
        super::ordinary_runtime::run_ordinary_cycle_v1(&c, &mut remote(&cloud), &id, 1, NOW)
            .is_err()
    );
    c.lock()
        .unwrap()
        .execute_batch("DROP TRIGGER fail_ack")
        .unwrap();
    let root = root_id();
    let mut store = SqliteS2LiteStoreV1::open(&c, &root).unwrap();
    let batch = store.load_unfinished_outbound_batch().unwrap().unwrap();
    let intent = store
        .load_prepared_intent(&batch.prepared_intent_path)
        .unwrap()
        .unwrap();
    store.complete_verified_outbound_batch().unwrap();
    let mut alternative = super::causal::parse_frozen_json_value_v1(&intent.exact_bytes).unwrap();
    alternative["commitId"] = json!(uuid::Uuid::new_v4().to_string());
    alternative["mutations"][0]["localMutationId"] = json!(uuid::Uuid::new_v4().to_string());
    let alternative = super::immutable_publish::prepare_commit_intent_v1(
        &super::canonical::jcs_bytes(&alternative).unwrap(),
        NOW,
    )
    .unwrap();
    {
        let mut cloud = cloud.lock().unwrap();
        cloud.objects.remove(&intent.remote_path);
        cloud
            .objects
            .insert(alternative.remote_path.clone(), alternative.exact_bytes);
    }
    drop(c);
    let c = reopen(&path);
    let puts = cloud.lock().unwrap().puts.len();
    assert_eq!(
        ordinary(&c, &id, &cloud),
        super::ordinary_runtime::OrdinaryCycleResultV1::ReadOnlyFrozen
    );
    assert_eq!(cloud.lock().unwrap().puts.len(), puts);
    assert!(SqliteS2LiteStoreV1::open(&c, &root)
        .unwrap()
        .load_root_safety(&root)
        .unwrap()
        .root_fatal_signals
        .iter()
        .any(|fatal| fatal.code == "S2_LOCAL_WRITER_OWNERSHIP_MISMATCH"));
    drop(c);
    std::fs::remove_file(path).unwrap();
}
#[test]
fn i65_astra_frozen_create_delete_does_not_resurrect_collection() {
    let (c, id) = database(None);
    seed(&c);
    let cloud = Arc::new(Mutex::new(Cloud::default()));
    finish(&c, &id, &cloud);
    let created = crate::collections::create(
        &mut c.lock().unwrap(),
        serde_json::from_value(json!({"name":"Astra frozen create"})).unwrap(),
        "device",
    )
    .unwrap();
    let intent = match super::outbound_freeze::freeze_active_outbound_v1(&c, &id, 1, NOW).unwrap() {
        super::outbound_freeze::OutboundFreezeResultV1::Frozen { intent, .. } => intent,
        other => panic!("{other:?}"),
    };
    crate::collections::delete(&mut c.lock().unwrap(), &created.id, created.rev, "device").unwrap();
    assert_eq!(
        ordinary(&c, &id, &cloud),
        super::ordinary_runtime::OrdinaryCycleResultV1::Pending
    );
    let guard = c.lock().unwrap();
    let rows = super::local_authority::load_staged_descriptors(&guard).unwrap();
    assert!(rows
        .iter()
        .any(|row| row.entity_id == created.id && row.operation == "delete"));
    let count: i64 = guard
        .query_row(
            "SELECT COUNT(*) FROM collections WHERE id=?1",
            [&created.id],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(count, 0);
    assert_eq!(
        cloud.lock().unwrap().objects.get(&intent.remote_path),
        Some(&intent.exact_bytes)
    );
}

#[test]
fn i65_astra_corrupt_frozen_collection_identity_fails_closed_across_restart() {
    let path = temp_path();
    let (mut c, id) = database(Some(&path));
    seed(&c);
    let cloud = Arc::new(Mutex::new(Cloud::default()));
    finish(&c, &id, &cloud);
    let created = crate::collections::create(
        &mut c.lock().unwrap(),
        serde_json::from_value(json!({"name":"Astra corrupt frozen identity"})).unwrap(),
        "device",
    )
    .unwrap();
    super::outbound_freeze::freeze_active_outbound_v1(&c, &id, 1, NOW).unwrap();
    let mut guard = c.lock().unwrap();
    let descriptors = super::local_authority::load_staged_descriptors(&guard).unwrap();
    let staging = crate::sync_staging::get_staging(&guard).unwrap();
    let generation = crate::db_atomic_helpers::get_records_generation(&guard).unwrap();
    corrupt_frozen_batch_entity_id(&guard, "collection", &created.id, "unrelated-entity");
    let frozen: Vec<u8> = guard
        .query_row(
            "SELECT state_json FROM s2_lite_outbound_batch_v1",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert!(crate::collections::delete(&mut guard, &created.id, created.rev, "device").is_err());
    assert_eq!(
        super::local_authority::load_staged_descriptors(&guard).unwrap(),
        descriptors
    );
    assert_eq!(crate::sync_staging::get_staging(&guard).unwrap(), staging);
    assert_eq!(
        crate::db_atomic_helpers::get_records_generation(&guard).unwrap(),
        generation
    );
    assert_eq!(
        guard
            .query_row(
                "SELECT state_json FROM s2_lite_outbound_batch_v1",
                [],
                |row| row.get::<_, Vec<u8>>(0),
            )
            .unwrap(),
        frozen
    );
    assert!(crate::collections::all(&guard)
        .unwrap()
        .iter()
        .any(|row| row.id == created.id));
    drop(guard);
    drop(c);
    c = reopen(&path);
    let mut guard = c.lock().unwrap();
    assert!(crate::collections::delete(&mut guard, &created.id, created.rev, "device").is_err());
    assert!(crate::collections::all(&guard)
        .unwrap()
        .iter()
        .any(|row| row.id == created.id));
    drop(guard);
    drop(c);
    std::fs::remove_file(path).unwrap();
}

#[test]
fn i65_astra_duplicate_frozen_membership_fails_closed_across_restart() {
    let path = temp_path();
    let (mut c, id) = database(Some(&path));
    seed(&c);
    let cloud = Arc::new(Mutex::new(Cloud::default()));
    finish(&c, &id, &cloud);
    let first = crate::collections::create(
        &mut c.lock().unwrap(),
        serde_json::from_value(json!({"name":"Astra frozen A"})).unwrap(),
        "device",
    )
    .unwrap();
    let second = crate::collections::create(
        &mut c.lock().unwrap(),
        serde_json::from_value(json!({"name":"Astra frozen C"})).unwrap(),
        "device",
    )
    .unwrap();
    super::outbound_freeze::freeze_active_outbound_v1(&c, &id, 1, NOW).unwrap();
    let mut guard = c.lock().unwrap();
    let descriptors = super::local_authority::load_staged_descriptors(&guard).unwrap();
    let staging = crate::sync_staging::get_staging(&guard).unwrap();
    mutate_frozen_batch(&guard, |mutations| {
        let first = mutations
            .iter()
            .find(|mutation| mutation["entityId"] == first.id)
            .unwrap()
            .clone();
        let second = mutations
            .iter_mut()
            .find(|mutation| mutation["entityId"] == second.id)
            .unwrap();
        *second = first;
    });
    let frozen: Vec<u8> = guard
        .query_row(
            "SELECT state_json FROM s2_lite_outbound_batch_v1",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert!(crate::collections::delete(&mut guard, &second.id, second.rev, "device").is_err());
    assert_eq!(
        super::local_authority::load_staged_descriptors(&guard).unwrap(),
        descriptors
    );
    assert_eq!(crate::sync_staging::get_staging(&guard).unwrap(), staging);
    assert_eq!(
        guard
            .query_row(
                "SELECT state_json FROM s2_lite_outbound_batch_v1",
                [],
                |row| row.get::<_, Vec<u8>>(0),
            )
            .unwrap(),
        frozen
    );
    assert!(crate::collections::all(&guard)
        .unwrap()
        .iter()
        .any(|row| row.id == second.id));
    drop(guard);
    drop(c);
    c = reopen(&path);
    let mut guard = c.lock().unwrap();
    assert!(crate::collections::delete(&mut guard, &second.id, second.rev, "device").is_err());
    assert!(crate::collections::all(&guard)
        .unwrap()
        .iter()
        .any(|row| row.id == second.id));
    drop(guard);
    drop(c);
    std::fs::remove_file(path).unwrap();
}

#[test]
fn i65_frozen_batch_rejects_record_and_composite_identity_mismatches() {
    for kind in ["record", "collection-member", "episode-completion"] {
        let (c, id) = database(None);
        seed(&c);
        let cloud = Arc::new(Mutex::new(Cloud::default()));
        finish(&c, &id, &cloud);
        let mutations = create_and_freeze_all_ordinary_entity_kinds(&c, &id);
        let (_, entity_id) = mutations
            .iter()
            .find(|(entity_kind, _)| entity_kind == kind)
            .unwrap();
        let collection_id = mutations
            .iter()
            .find(|(entity_kind, _)| entity_kind == "collection")
            .unwrap()
            .1
            .clone();
        let mut guard = c.lock().unwrap();
        let descriptors = super::local_authority::load_staged_descriptors(&guard).unwrap();
        let staging = crate::sync_staging::get_staging(&guard).unwrap();
        corrupt_frozen_batch_entity_id(&guard, kind, entity_id, "unrelated-entity");
        let collection = crate::collections::all(&guard)
            .unwrap()
            .into_iter()
            .find(|row| row.id == collection_id)
            .unwrap();
        assert!(
            crate::collections::delete(&mut guard, &collection.id, collection.rev, "device")
                .is_err()
        );
        assert_eq!(
            super::local_authority::load_staged_descriptors(&guard).unwrap(),
            descriptors,
            "{kind} corruption must not alter local capture"
        );
        assert_eq!(
            crate::sync_staging::get_staging(&guard).unwrap(),
            staging,
            "{kind} corruption must not alter S1 staging"
        );
        assert!(crate::collections::all(&guard)
            .unwrap()
            .iter()
            .any(|row| row.id == collection_id));
    }
}

#[test]
fn i65_frozen_batch_intent_bijection_rejects_missing_extra_and_substitution() {
    for corruption in ["duplicate-id", "missing", "extra", "substitution"] {
        let (c, id) = database(None);
        seed(&c);
        let cloud = Arc::new(Mutex::new(Cloud::default()));
        finish(&c, &id, &cloud);
        let first = crate::collections::create(
            &mut c.lock().unwrap(),
            serde_json::from_value(json!({"name":"Batch A"})).unwrap(),
            "device",
        )
        .unwrap();
        let second = crate::collections::create(
            &mut c.lock().unwrap(),
            serde_json::from_value(json!({"name":"Batch C"})).unwrap(),
            "device",
        )
        .unwrap();
        super::outbound_freeze::freeze_active_outbound_v1(&c, &id, 1, NOW).unwrap();
        let mut guard = c.lock().unwrap();
        let descriptors = super::local_authority::load_staged_descriptors(&guard).unwrap();
        let staging = crate::sync_staging::get_staging(&guard).unwrap();
        mutate_frozen_batch(&guard, |mutations| {
            let first_index = mutations
                .iter()
                .position(|mutation| mutation["entityId"] == first.id)
                .unwrap();
            let second_index = mutations
                .iter()
                .position(|mutation| mutation["entityId"] == second.id)
                .unwrap();
            match corruption {
                "duplicate-id" => {
                    mutations[second_index]["localMutationId"] =
                        mutations[first_index]["localMutationId"].clone();
                }
                "missing" => {
                    mutations.remove(second_index);
                }
                "extra" => mutations.push(mutations[first_index].clone()),
                "substitution" => {
                    mutations[second_index] = json!({
                        "entityKind":"record",
                        "entityId":"replacement-record",
                        "entityKey":["record","replacement-record"],
                        "capturedLastGeneration":0,
                        "localMutationId":uuid::Uuid::new_v4().to_string()
                    });
                }
                _ => unreachable!(),
            }
        });
        assert!(crate::collections::delete(&mut guard, &second.id, second.rev, "device").is_err());
        assert_eq!(
            super::local_authority::load_staged_descriptors(&guard).unwrap(),
            descriptors,
            "{corruption} must roll back descriptor mutation"
        );
        assert_eq!(
            crate::sync_staging::get_staging(&guard).unwrap(),
            staging,
            "{corruption} must roll back S1 staging"
        );
        assert!(crate::collections::all(&guard)
            .unwrap()
            .iter()
            .any(|row| row.id == second.id));
    }
}

#[test]
fn i65_frozen_batch_reordered_exact_membership_remains_valid() {
    let (c, id) = database(None);
    seed(&c);
    let cloud = Arc::new(Mutex::new(Cloud::default()));
    finish(&c, &id, &cloud);
    crate::collections::create(
        &mut c.lock().unwrap(),
        serde_json::from_value(json!({"name":"Ordered A"})).unwrap(),
        "device",
    )
    .unwrap();
    let second = crate::collections::create(
        &mut c.lock().unwrap(),
        serde_json::from_value(json!({"name":"Ordered C"})).unwrap(),
        "device",
    )
    .unwrap();
    super::outbound_freeze::freeze_active_outbound_v1(&c, &id, 1, NOW).unwrap();
    mutate_frozen_batch(&c.lock().unwrap(), |mutations| mutations.reverse());
    crate::collections::delete(&mut c.lock().unwrap(), &second.id, second.rev, "device").unwrap();
    let delete = super::local_authority::load_staged_descriptors(&c.lock().unwrap())
        .unwrap()
        .into_iter()
        .find(|descriptor| descriptor.entity_id == second.id)
        .unwrap();
    assert_eq!(delete.operation, "delete");
}

#[test]
fn i65_frozen_batch_rejects_duplicated_composite_membership() {
    for duplicated_kind in ["collection-member", "episode-completion"] {
        let (c, id) = database(None);
        seed(&c);
        let cloud = Arc::new(Mutex::new(Cloud::default()));
        finish(&c, &id, &cloud);
        let mutations = create_and_freeze_all_ordinary_entity_kinds(&c, &id);
        let collection_id = mutations
            .iter()
            .find(|(kind, _)| kind == "collection")
            .unwrap()
            .1
            .clone();
        let other_kind = if duplicated_kind == "collection-member" {
            "episode-completion"
        } else {
            "collection-member"
        };
        let duplicated_id = mutations
            .iter()
            .find(|(kind, _)| kind == duplicated_kind)
            .unwrap()
            .1
            .clone();
        let omitted_id = mutations
            .iter()
            .find(|(kind, _)| kind == other_kind)
            .unwrap()
            .1
            .clone();
        let mut guard = c.lock().unwrap();
        let descriptors = super::local_authority::load_staged_descriptors(&guard).unwrap();
        mutate_frozen_batch(&guard, |batch| {
            let duplicated = batch
                .iter()
                .find(|mutation| mutation["entityId"] == duplicated_id)
                .unwrap()
                .clone();
            let omitted = batch
                .iter_mut()
                .find(|mutation| mutation["entityId"] == omitted_id)
                .unwrap();
            *omitted = duplicated;
        });
        let collection = crate::collections::all(&guard)
            .unwrap()
            .into_iter()
            .find(|row| row.id == collection_id)
            .unwrap();
        assert!(
            crate::collections::delete(&mut guard, &collection.id, collection.rev, "device")
                .is_err()
        );
        assert_eq!(
            super::local_authority::load_staged_descriptors(&guard).unwrap(),
            descriptors
        );
        assert!(crate::collections::all(&guard)
            .unwrap()
            .iter()
            .any(|row| row.id == collection_id));
    }
}

#[test]
fn i65_post_freeze_delete_crash_interleaving_matrix() {
    for boundary in ["mutable", "prepared", "ambiguous", "receipt"] {
        for restart in [false, true] {
            let path = temp_path();
            let (c, id) = database(Some(&path));
            seed(&c);
            let cloud = Arc::new(Mutex::new(Cloud::default()));
            finish(&c, &id, &cloud);
            let created = crate::collections::create(
                &mut c.lock().unwrap(),
                serde_json::from_value(json!({"name":"Boundary collection"})).unwrap(),
                "device",
            )
            .unwrap();
            let captured = super::local_authority::load_staged_descriptors(&c.lock().unwrap())
                .unwrap()
                .into_iter()
                .find(|row| row.entity_id == created.id)
                .unwrap();
            let intent = if boundary == "mutable" {
                None
            } else {
                match super::outbound_freeze::freeze_active_outbound_v1(&c, &id, 1, NOW).unwrap() {
                    super::outbound_freeze::OutboundFreezeResultV1::Frozen { intent, .. } => {
                        Some(*intent)
                    }
                    other => panic!("{other:?}"),
                }
            };
            if boundary == "ambiguous" {
                let mut fake = cloud.lock().unwrap();
                fake.lost = true;
                fake.unavailable_after_next_put = true;
                drop(fake);
                assert_eq!(
                    ordinary(&c, &id, &cloud),
                    super::ordinary_runtime::OrdinaryCycleResultV1::Pending
                );
            }
            if boundary == "receipt" {
                c.lock().unwrap().execute_batch("CREATE TRIGGER fail_retirement BEFORE DELETE ON s2_lite_local_staging_descriptor_v1 BEGIN SELECT RAISE(ABORT,'crash'); END").unwrap();
                assert!(super::ordinary_runtime::run_ordinary_cycle_v1(
                    &c,
                    &mut remote(&cloud),
                    &id,
                    1,
                    NOW
                )
                .is_err());
                c.lock()
                    .unwrap()
                    .execute_batch("DROP TRIGGER fail_retirement")
                    .unwrap();
                assert!(SqliteS2LiteStoreV1::open(&c, &root_id())
                    .unwrap()
                    .load_published_receipt(&intent.as_ref().unwrap().remote_path)
                    .unwrap()
                    .is_some());
            }
            crate::collections::delete(&mut c.lock().unwrap(), &created.id, created.rev, "device")
                .unwrap();
            let deletes =
                super::local_authority::load_staged_descriptors(&c.lock().unwrap()).unwrap();
            if boundary == "mutable" {
                assert!(deletes.is_empty());
            } else {
                let delete = &deletes[0];
                assert_eq!(delete.operation, "delete");
                assert_ne!(delete.local_mutation_id, captured.local_mutation_id);
                assert!(delete.first_generation > captured.last_generation);
                assert_eq!(delete.first_generation, delete.last_generation);
                assert_eq!(delete.causal_anchor, captured.causal_anchor);
                assert_eq!(delete.verified_basis, captured.verified_basis);
                assert!(delete.frozen_delete_evidence().is_ok());
                assert_eq!(
                    SqliteS2LiteStoreV1::open(&c, &root_id())
                        .unwrap()
                        .load_prepared_intent(&intent.as_ref().unwrap().remote_path)
                        .unwrap()
                        .as_ref(),
                    intent.as_ref()
                );
            }
            let c = if restart {
                drop(c);
                reopen(&path)
            } else {
                c
            };
            assert_eq!(
                super::local_authority::load_staged_descriptors(&c.lock().unwrap()).unwrap(),
                deletes
            );
            let coordinator = super::root_coordinator::RootExecutionCoordinatorV1::default();
            let expected = if boundary == "mutable" {
                super::ordinary_runtime::OrdinaryCycleResultV1::Success
            } else {
                super::ordinary_runtime::OrdinaryCycleResultV1::Pending
            };
            assert_eq!(
                mobile(&c, &id, &cloud, &coordinator, false, None),
                expected,
                "{boundary}, restart={restart}"
            );
            assert_eq!(
                super::local_authority::load_staged_descriptors(&c.lock().unwrap()).unwrap(),
                deletes
            );
            let count: i64 = c
                .lock()
                .unwrap()
                .query_row(
                    "SELECT COUNT(*) FROM collections WHERE id=?1",
                    [&created.id],
                    |row| row.get(0),
                )
                .unwrap();
            assert_eq!(count, 0);
            if let Some(intent) = intent {
                assert_eq!(
                    cloud.lock().unwrap().objects.get(&intent.remote_path),
                    Some(&intent.exact_bytes)
                );
                assert_eq!(
                    cloud
                        .lock()
                        .unwrap()
                        .puts
                        .iter()
                        .filter(|(path, _)| path == &intent.remote_path)
                        .count(),
                    1
                );
                let scheduler = crate::sync_state::runtime_state(&c.lock().unwrap())
                    .unwrap()
                    .scheduler;
                assert!(scheduler.last_success_at.is_none());
                assert_eq!(scheduler.last_error_code.as_deref(), Some("s2_pending"));
                assert_eq!(
                    mobile(&c, &id, &cloud, &coordinator, false, None),
                    super::ordinary_runtime::OrdinaryCycleResultV1::Pending
                );
                assert_eq!(
                    super::local_authority::load_staged_descriptors(&c.lock().unwrap()).unwrap(),
                    deletes
                );
            }
            drop(c);
            std::fs::remove_file(path).unwrap();
        }
    }
}

#[test]
fn i65_post_freeze_delete_all_four_production_entity_classes() {
    let (c, id) = database(None);
    seed(&c);
    let cloud = Arc::new(Mutex::new(Cloud::default()));
    finish(&c, &id, &cloud);
    let record_id = uuid::Uuid::new_v4().to_string();
    let collection = {
        let mut guard = c.lock().unwrap();
        crate::db_atomic_crud::insert_record_atomic(&mut guard, serde_json::from_value(json!({"id":record_id,"originalName":"New series","chineseName":"New series","progress":"","totalEpisodes":6,"status":"未看","platform":"","notes":"","createdAt":NOW,"mediaType":"剧集","episodeTrackingEnabled":true,"nextEpisode":1})).unwrap(), "device").unwrap();
        crate::episode_history::set_next(&mut guard, &record_id, Some(2), 1, "device").unwrap();
        let collection = crate::collections::create(
            &mut guard,
            serde_json::from_value(json!({"name":"New parent"})).unwrap(),
            "device",
        )
        .unwrap();
        crate::collections::add_members(
            &mut guard,
            &collection.id,
            vec![record_id.clone()],
            "manual",
            collection.rev,
            "device",
        )
        .unwrap();
        collection
    };
    let before = super::local_authority::load_staged_descriptors(&c.lock().unwrap()).unwrap();
    assert_eq!(before.len(), 4);
    let intent = match super::outbound_freeze::freeze_active_outbound_v1(&c, &id, 1, NOW).unwrap() {
        super::outbound_freeze::OutboundFreezeResultV1::Frozen { intent, .. } => intent,
        other => panic!("{other:?}"),
    };
    {
        let mut guard = c.lock().unwrap();
        crate::db_atomic_crud::delete_record_atomic(&mut guard, &record_id, "device").unwrap();
        let rev = crate::collections::all(&guard)
            .unwrap()
            .into_iter()
            .find(|row| row.id == collection.id)
            .unwrap()
            .rev;
        crate::collections::delete(&mut guard, &collection.id, rev, "device").unwrap();
    }
    let deletes = super::local_authority::load_staged_descriptors(&c.lock().unwrap()).unwrap();
    assert_eq!(deletes.len(), 4);
    for old in before {
        let delete = deletes
            .iter()
            .find(|row| row.entity_kind == old.entity_kind && row.entity_id == old.entity_id)
            .unwrap();
        assert_eq!(delete.operation, "delete");
        assert_ne!(delete.local_mutation_id, old.local_mutation_id);
        assert!(delete.first_generation > old.last_generation);
        assert_eq!(delete.causal_anchor, old.causal_anchor);
        assert_eq!(delete.verified_basis, old.verified_basis);
        assert!(delete.frozen_delete_evidence().is_ok());
    }
    assert_eq!(
        ordinary(&c, &id, &cloud),
        super::ordinary_runtime::OrdinaryCycleResultV1::Pending
    );
    assert_eq!(
        super::local_authority::load_staged_descriptors(&c.lock().unwrap()).unwrap(),
        deletes
    );
    assert_eq!(
        cloud.lock().unwrap().objects.get(&intent.remote_path),
        Some(&intent.exact_bytes)
    );
    assert!(!crate::db::get_all_records(&c.lock().unwrap())
        .unwrap()
        .iter()
        .any(|row| row.id == record_id));
    assert!(!crate::collections::all(&c.lock().unwrap())
        .unwrap()
        .iter()
        .any(|row| row.id == collection.id));
}
#[test]
fn i65_post_freeze_delete_recreate_delete_keeps_successor_identity() {
    for recovered in [false, true] {
        let (c, id) = database(None);
        seed(&c);
        let cloud = Arc::new(Mutex::new(Cloud::default()));
        finish(&c, &id, &cloud);
        let record_id = uuid::Uuid::new_v4().to_string();
        let created = crate::db_atomic_crud::insert_record_atomic(&mut c.lock().unwrap(), serde_json::from_value(json!({"id":record_id,"originalName":"Recreated record","chineseName":"Recreated record","progress":"","status":"未看","platform":"","notes":"","createdAt":NOW,"mediaType":"电影"})).unwrap(), "device").unwrap();
        super::outbound_freeze::freeze_active_outbound_v1(&c, &id, 1, NOW).unwrap();
        crate::db_atomic_crud::delete_record_atomic(&mut c.lock().unwrap(), &record_id, "device")
            .unwrap();
        let successor =
            super::local_authority::load_staged_descriptors(&c.lock().unwrap()).unwrap()[0].clone();
        if recovered {
            assert_eq!(
                ordinary(&c, &id, &cloud),
                super::ordinary_runtime::OrdinaryCycleResultV1::Pending
            );
        }
        crate::db_atomic_crud::insert_record_atomic(&mut c.lock().unwrap(), created, "device")
            .unwrap();
        crate::db_atomic_crud::delete_record_atomic(&mut c.lock().unwrap(), &record_id, "device")
            .unwrap();
        let delete =
            super::local_authority::load_staged_descriptors(&c.lock().unwrap()).unwrap()[0].clone();
        assert_eq!(delete.operation, "delete");
        assert_eq!(delete.local_mutation_id, successor.local_mutation_id);
        assert_eq!(delete.first_generation, successor.first_generation);
        assert_eq!(delete.verified_basis, successor.verified_basis);
        assert_eq!(
            ordinary(&c, &id, &cloud),
            super::ordinary_runtime::OrdinaryCycleResultV1::Pending
        );
        assert_eq!(
            super::local_authority::load_staged_descriptors(&c.lock().unwrap()).unwrap(),
            vec![delete]
        );
        assert!(crate::db::get_record(&c.lock().unwrap(), &record_id)
            .unwrap()
            .is_none());
    }
}

#[test]
fn i65_post_freeze_delete_invalid_publication_authority_rolls_back_business_capture() {
    for invalid_intent in [false, true] {
        let (c, id) = database(None);
        seed(&c);
        let cloud = Arc::new(Mutex::new(Cloud::default()));
        finish(&c, &id, &cloud);
        let created = crate::collections::create(
            &mut c.lock().unwrap(),
            serde_json::from_value(json!({"name":"Atomic delete"})).unwrap(),
            "device",
        )
        .unwrap();
        let intent =
            match super::outbound_freeze::freeze_active_outbound_v1(&c, &id, 1, NOW).unwrap() {
                super::outbound_freeze::OutboundFreezeResultV1::Frozen { intent, .. } => intent,
                other => panic!("{other:?}"),
            };
        let mut guard = c.lock().unwrap();
        let before = super::local_authority::load_staged_descriptors(&guard).unwrap();
        let staging = crate::sync_staging::get_staging(&guard).unwrap();
        let generation = crate::db_atomic_helpers::get_records_generation(&guard).unwrap();
        if invalid_intent {
            guard
                .execute(
                    "UPDATE s2_lite_prepared_intent_v1 SET exact_bytes=?1 WHERE remote_path=?2",
                    rusqlite::params![b"bad".as_slice(), intent.remote_path],
                )
                .unwrap();
        } else {
            guard
                .execute(
                    "UPDATE s2_lite_outbound_batch_v1 SET state_json=?1",
                    [b"{}".as_slice()],
                )
                .unwrap();
        }
        assert!(
            crate::collections::delete(&mut guard, &created.id, created.rev, "device").is_err()
        );
        assert_eq!(
            super::local_authority::load_staged_descriptors(&guard).unwrap(),
            before
        );
        assert_eq!(crate::sync_staging::get_staging(&guard).unwrap(), staging);
        assert_eq!(
            crate::db_atomic_helpers::get_records_generation(&guard).unwrap(),
            generation
        );
        assert!(crate::collections::all(&guard)
            .unwrap()
            .iter()
            .any(|row| row.id == created.id));
    }
}
