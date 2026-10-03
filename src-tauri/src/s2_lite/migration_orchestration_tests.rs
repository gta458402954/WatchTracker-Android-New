use std::{
    collections::{HashMap, VecDeque},
    fs,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
};

use serde_json::{json, Value};

use super::activation_cutover::{
    create_activation_cutover_state_v1, decide_legacy_put_v1, recover_activation_cutover_v1,
    ActivationCutoverFatalV1, ActivationCutoverStateV1, ActivationFingerprintConsistencyV1,
    LegacyPutDecisionV1, VerifiedActivationEvidenceV1,
};
use super::canonical::{sha256_hex, ProtocolError, Result};
use super::causal::{decode_frozen_wire_commit_v1, detect_writer_forks_v1};
use super::immutable_publish::{
    restart_durable_activation_publish_v1, restart_durable_publish_v1, ImmutableObjectRemoteV1,
    PreparedActivationIntentStoreV1, PreparedActivationIntentV1, PreparedIntentStoreV1,
    PreparedIntentV1, PublishedActivationReceiptStoreV1, PublishedActivationReceiptV1,
    PublishedReceiptStoreV1, RecoverActivationIntentResultV1, RecoverPreparedIntentResultV1,
    RemoteExactGetResultV1, RemotePublishedReceiptV1, RemotePutResultV1,
};
use super::migration_orchestration::{
    capture_legacy_snapshot_v1, create_migration_root_execution_capability_v1,
    create_migration_root_safety_state_v1, create_migration_state_v1,
    create_new_root_migration_handoff_v1, execute_migration_step_v1,
    freeze_old_root_for_new_root_handoff_v1, merge_migration_root_cutover_state_v1,
    migration_projection_v1, plan_captured_migration_v1, recover_migration_activation_cutover_v1,
    retain_captured_snapshot_v1, start_or_attach_migration_v1,
    validate_migration_activation_body_v1, ActivationCutoverStateStoreV1, LegacySnapshotEntryV1,
    MigrationExecutionIdentityV1, MigrationRootFatalV1, MigrationRootSafetyStateV1,
    MigrationStateStoreV1, MigrationStateV1, MigrationStatusV1, PublishExclusiveResultV1,
};
use super::remote_discovery::create_discovery_state_v1;
use super::types::{BootstrapEntity, LegacySemanticAdapterV1};

fn fixture() -> Value {
    serde_json::from_str(
        &fs::read_to_string(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../contracts/s2-lite/v1/migration-golden-v1.json"
        ))
        .unwrap(),
    )
    .unwrap()
}

#[test]
fn migration_activation_validator_preserves_desktop_duplicate_key_parsing() {
    let body = br#"{
        "activationId":"10000000-0000-4000-8000-000000000001",
        "legacyFingerprint":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
        "protocol":"watchtracker-s2-lite",
        "protocol":"watchtracker-s2-lite",
        "protocolVersion":1,
        "requiredFeatures":[],
        "s2SemanticProfileVersion":1
    }"#;

    assert_eq!(
        serde_json::from_slice::<Value>(body).unwrap()["protocol"],
        "watchtracker-s2-lite"
    );
    assert!(validate_migration_activation_body_v1(body).is_ok());
}

fn record(index: usize, timestamp: &str) -> BootstrapEntity {
    let id = format!("record-{index:04}");
    BootstrapEntity {
        entity_type: "record".to_string(),
        entity_key: json!(["record", id]),
        value: json!({
            "id": id, "originalName": format!("Record {index}"), "chineseName": "", "progress": "",
            "totalEpisodes": 1, "episodeTrackingEnabled": false, "nextEpisode": null,
            "movieProgress": null, "movieDuration": null, "releaseYear": null, "posterPath": null,
            "status": "未看", "platform": "", "rating": null, "startDate": null, "endDate": null,
            "notes": "", "createdAt": timestamp, "updatedAt": null, "imdbId": null,
            "isLocked": null, "genres": null, "originCountry": null, "imdbRating": null,
            "tmdbStatus": null, "interestLevel": null, "episodeRuntime": null, "mediaType": "剧集",
            "contentTags": null, "tmdbMediaKind": null, "tmdbId": null, "tmdbParentId": null,
            "tmdbSeasonNumber": null, "seriesRecordKind": null, "rev": "0", "revActor": ""
        }),
    }
}

fn collection(index: usize, timestamp: &str) -> BootstrapEntity {
    let id = format!("collection-{index:04}");
    BootstrapEntity {
        entity_type: "collection".to_string(),
        entity_key: json!(["collection", id]),
        value: json!({
            "id": id, "name": format!("Collection {index}"), "normalizedName": format!("collection {index}"),
            "description": null, "sourceKind": "manual", "sourceKey": null,
            "collectionKind": "manual", "orderMode": "manual", "createdAt": timestamp,
            "updatedAt": timestamp, "rev": "0", "revActor": ""
        }),
    }
}

fn deterministic_id(domain: &str, components: &[String]) -> String {
    let mut bytes = domain.as_bytes().to_vec();
    for component in components {
        bytes.push(0);
        bytes.extend_from_slice(component.as_bytes());
    }
    sha256_hex(&bytes)
}

fn entities_for(scenario: &Value, timestamp: &str) -> Vec<BootstrapEntity> {
    let mut values = Vec::new();
    for index in 0..scenario["records"].as_u64().unwrap() as usize {
        values.push(record(index, timestamp));
    }
    for index in 0..scenario["collections"].as_u64().unwrap() as usize {
        values.push(collection(index, timestamp));
    }
    for index in 0..scenario["episodes"].as_u64().unwrap() as usize {
        let record_id = format!("record-{index:04}");
        values.push(BootstrapEntity {
            entity_type: "episode-completion".to_string(),
            entity_key: json!(["episode-completion", record_id, 1]),
            value: json!({
                "id": deterministic_id("episode-completion:v1", &[record_id.clone(), "1".to_string()]),
                "recordId": record_id, "episodeNumber": 1, "completedAt": timestamp,
                "createdAt": timestamp, "updatedAt": timestamp, "rev": "0", "revActor": ""
            }),
        });
    }
    for index in 0..scenario["members"].as_u64().unwrap() as usize {
        let record_id = format!("record-{index:04}");
        let collection_id = format!("collection-{index:04}");
        values.push(BootstrapEntity {
            entity_type: "collection-member".to_string(),
            entity_key: json!(["collection-member", collection_id, record_id]),
            value: json!({
                "id": deterministic_id("collection-member:v1", &[collection_id.clone(), record_id.clone()]),
                "collectionId": collection_id, "recordId": record_id, "position": "0",
                "sourceKind": "manual", "createdAt": timestamp, "updatedAt": timestamp,
                "rev": "0", "revActor": ""
            }),
        });
    }
    values
}

struct IdentityAdapter;

impl LegacySemanticAdapterV1 for IdentityAdapter {
    fn adapt_live_entity(
        &self,
        entity_type: &str,
        legacy_value: &Value,
    ) -> std::result::Result<BootstrapEntity, String> {
        let entity: BootstrapEntity =
            serde_json::from_value(legacy_value.clone()).map_err(|error| error.to_string())?;
        if entity.entity_type != entity_type {
            return Err("type mismatch".to_string());
        }
        Ok(entity)
    }
}

fn planned_for(fixture: &Value, scenario: &Value) -> MigrationStateV1 {
    let identity = &fixture["identity"];
    let entities = entities_for(scenario, identity["createdAt"].as_str().unwrap());
    let entries = entities
        .iter()
        .map(|entity| LegacySnapshotEntryV1 {
            entity_type: entity.entity_type.clone(),
            value: serde_json::to_value(entity).unwrap(),
        })
        .collect::<Vec<_>>();
    let snapshot = capture_legacy_snapshot_v1(&entries, &IdentityAdapter).unwrap();
    let initial = create_migration_state_v1(
        identity["migrationId"].as_str().unwrap(),
        identity["rootId"].as_str().unwrap(),
        identity["writerId"].as_str().unwrap(),
        identity["createdAt"].as_str().unwrap(),
        "legacy-bootstrap",
    )
    .unwrap();
    plan_captured_migration_v1(&retain_captured_snapshot_v1(&initial, &snapshot).unwrap()).unwrap()
}

fn chunk_sizes(tasks: &[super::migration_orchestration::MigrationCommitTaskV1]) -> Vec<usize> {
    tasks
        .iter()
        .map(|task| {
            serde_json::from_slice::<Value>(&task.intent.exact_bytes).unwrap()["mutations"]
                .as_array()
                .unwrap()
                .len()
        })
        .collect()
}

#[test]
fn shared_planning_fixture_matches_rust_plans_and_deterministic_ids() {
    let fixture = fixture();
    for scenario in fixture["planningScenarios"].as_array().unwrap() {
        let state = planned_for(&fixture, scenario);
        assert_eq!(
            serde_json::to_value(chunk_sizes(&state.stage_a)).unwrap(),
            scenario["expectedStageAChunkSizes"],
            "{}",
            scenario["name"]
        );
        assert_eq!(
            serde_json::to_value(chunk_sizes(&state.stage_b)).unwrap(),
            scenario["expectedStageBChunkSizes"],
            "{}",
            scenario["name"]
        );
        let sequences = state
            .stage_a
            .iter()
            .chain(&state.stage_b)
            .map(|task| task.intent.commit_ref.writer_seq.clone())
            .collect::<Vec<_>>();
        assert_eq!(
            serde_json::to_value(sequences).unwrap(),
            scenario["expectedWriterSeqs"]
        );
        assert_eq!(
            state.activation_intent.unwrap().activation_id,
            fixture["deterministicIdentity"]["activation"]
        );
    }
    let standard = planned_for(&fixture, &fixture["planningScenarios"][2]);
    assert_eq!(
        migration_projection_v1(&standard),
        fixture["standardExpectedProjection"]
    );
    assert_eq!(
        standard.stage_a[0].intent.commit_ref.commit_id,
        fixture["deterministicIdentity"]["commit1"]
    );
    assert_eq!(
        standard.stage_b[0].intent.commit_ref.commit_id,
        fixture["deterministicIdentity"]["commit2"]
    );
}

struct FakeRemote {
    execution_identity: u64,
    root_id: String,
    objects: HashMap<String, Vec<u8>>,
    put_counts: HashMap<String, usize>,
    trace: Vec<String>,
    detailed_trace: Vec<Value>,
    response_lost: bool,
    scripted_gets: VecDeque<RemoteExactGetResultV1>,
    preparation_result: RemotePutResultV1,
    preparation_calls: usize,
    record_preparation: bool,
    fatal_signal: Option<Arc<AtomicBool>>,
    signal_fatal_on_get: bool,
    signal_fatal_on_put: bool,
}

impl Default for FakeRemote {
    fn default() -> Self {
        Self {
            execution_identity: 101,
            root_id: String::new(),
            objects: HashMap::new(),
            put_counts: HashMap::new(),
            trace: Vec::new(),
            detailed_trace: Vec::new(),
            response_lost: false,
            scripted_gets: VecDeque::new(),
            preparation_result: RemotePutResultV1::Success,
            preparation_calls: 0,
            record_preparation: false,
            fatal_signal: None,
            signal_fatal_on_get: false,
            signal_fatal_on_put: false,
        }
    }
}

impl ImmutableObjectRemoteV1 for FakeRemote {
    fn execution_context_identity(&self) -> u64 {
        self.execution_identity
    }

    fn physical_root_id(&self) -> Option<&str> {
        Some(&self.root_id)
    }

    fn get_exact(&mut self, path: &str) -> RemoteExactGetResultV1 {
        self.trace.push("GET".to_string());
        self.detailed_trace
            .push(json!({ "operation": "GET", "path": path }));
        let result = self.scripted_gets.pop_front().unwrap_or_else(|| {
            self.objects.get(path).cloned().map_or(
                RemoteExactGetResultV1::DefinitelyAbsent,
                RemoteExactGetResultV1::DefinitelyPresent,
            )
        });
        if self.signal_fatal_on_get {
            self.signal_fatal_on_get = false;
            if let Some(signal) = &self.fatal_signal {
                signal.store(true, Ordering::SeqCst);
            }
        }
        result
    }

    fn prepare_immutable_parent_collections(&mut self, path: &str) -> RemotePutResultV1 {
        self.preparation_calls += 1;
        if self.record_preparation {
            self.trace.push("PREPARE".to_string());
            self.detailed_trace
                .push(json!({ "operation": "PREPARE", "path": path }));
        }
        self.preparation_result
    }

    fn put_exact(&mut self, path: &str, bytes: &[u8], _: bool) -> RemotePutResultV1 {
        self.trace.push("PUT".to_string());
        self.detailed_trace
            .push(json!({ "operation": "PUT", "path": path }));
        *self.put_counts.entry(path.to_string()).or_default() += 1;
        self.objects.insert(path.to_string(), bytes.to_vec());
        if self.signal_fatal_on_put {
            self.signal_fatal_on_put = false;
            if let Some(signal) = &self.fatal_signal {
                signal.store(true, Ordering::SeqCst);
            }
        }
        if self.response_lost {
            RemotePutResultV1::Indeterminate
        } else {
            RemotePutResultV1::Success
        }
    }
}

struct MigrationStore {
    authority_identity: u64,
    current: Option<MigrationStateV1>,
    history: Vec<MigrationStateV1>,
    freeze_before_publish: bool,
    cutover_state: Option<ActivationCutoverStateV1>,
    external_fatal_signal: Option<Arc<AtomicBool>>,
    actual_cutover_decisions: Vec<String>,
    root_safety: Option<MigrationRootSafetyStateV1>,
}
impl Default for MigrationStore {
    fn default() -> Self {
        Self {
            authority_identity: 201,
            current: None,
            history: Vec::new(),
            freeze_before_publish: false,
            cutover_state: None,
            external_fatal_signal: None,
            actual_cutover_decisions: Vec::new(),
            root_safety: None,
        }
    }
}
impl MigrationStateStoreV1 for MigrationStore {
    fn authority_identity(&self) -> u64 {
        self.authority_identity
    }

    fn claim_or_load(&mut self, candidate: &MigrationStateV1) -> Result<MigrationStateV1> {
        if self.current.is_none() {
            let mut claimed = candidate.clone();
            if let Some(safety) = self.root_safety.as_ref() {
                if safety.root_id != candidate.root_id {
                    return Err(ProtocolError("MIGRATION_ROOT_BINDING_MISMATCH"));
                }
                if !safety.root_fatal_signals.is_empty() {
                    claimed.generation += 1;
                    claimed.status = MigrationStatusV1::RootFrozen;
                    claimed.root_fatal_signals = safety.root_fatal_signals.clone();
                }
            }
            self.current = Some(claimed.clone());
            self.history.push(claimed);
        }
        Ok(self.current.clone().unwrap())
    }

    fn load(&mut self, root_id: &str) -> Result<Option<MigrationStateV1>> {
        Ok(self
            .current
            .clone()
            .filter(|state| state.root_id == root_id))
    }

    fn compare_and_swap(
        &mut self,
        root_id: &str,
        migration_id: &str,
        expected_generation: u64,
        next: &MigrationStateV1,
    ) -> Result<bool> {
        let matches = self.current.as_ref().is_some_and(|state| {
            state.root_id == root_id
                && state.migration_id == migration_id
                && state.generation == expected_generation
        });
        if matches {
            self.current = Some(next.clone());
            self.history.push(next.clone());
        }
        Ok(matches)
    }

    fn load_root_safety(&mut self, root_id: &str) -> Result<MigrationRootSafetyStateV1> {
        let safety = self
            .root_safety
            .get_or_insert_with(|| create_migration_root_safety_state_v1(root_id));
        if safety.root_id != root_id {
            return Err(ProtocolError("MIGRATION_ROOT_BINDING_MISMATCH"));
        }
        Ok(safety.clone())
    }

    fn load_cutover_state(&mut self, root_id: &str) -> Result<Option<ActivationCutoverStateV1>> {
        Ok(Some(self.load_root_safety(root_id)?.cutover_state))
    }

    fn persist_cutover_state(
        &mut self,
        root_id: &str,
        state: &ActivationCutoverStateV1,
    ) -> Result<()> {
        if self
            .current
            .as_ref()
            .is_some_and(|value| value.root_id != root_id)
        {
            return Err(ProtocolError("MIGRATION_ROOT_BINDING_MISMATCH"));
        }
        let prior = self.load_root_safety(root_id)?.cutover_state;
        let merged = merge_migration_root_cutover_state_v1(&prior, state)?;
        let mut root_fatal_codes = self
            .root_safety
            .as_ref()
            .unwrap()
            .root_fatal_signals
            .iter()
            .map(|fatal| fatal.code.clone())
            .chain(
                merged
                    .root_fatal_signals
                    .iter()
                    .map(|fatal| fatal.code.clone()),
            )
            .collect::<Vec<_>>();
        root_fatal_codes.sort();
        root_fatal_codes.dedup();
        let next_root_fatals = root_fatal_codes
            .into_iter()
            .map(|code| MigrationRootFatalV1 { code })
            .collect::<Vec<_>>();
        let changed = merged != prior
            || self.root_safety.as_ref().unwrap().root_fatal_signals != next_root_fatals;
        let safety = self.root_safety.as_mut().unwrap();
        if changed {
            safety.generation += 1;
        }
        safety.cutover_state = merged.clone();
        safety.root_fatal_signals = next_root_fatals;
        self.cutover_state = Some(merged.clone());
        for fatal in &merged.root_fatal_signals {
            let Some(migration) = self.current.as_mut() else {
                continue;
            };
            if !migration
                .root_fatal_signals
                .iter()
                .any(|value| value.code == fatal.code)
            {
                migration.generation += 1;
                migration.status = MigrationStatusV1::RootFrozen;
                migration.root_fatal_signals.push(MigrationRootFatalV1 {
                    code: fatal.code.clone(),
                });
                migration
                    .root_fatal_signals
                    .sort_by(|left, right| left.code.cmp(&right.code));
                self.history.push(migration.clone());
            }
        }
        self.actual_cutover_decisions
            .push(if merged.root_fatal_signals.is_empty() {
                "publish-eligible".to_string()
            } else {
                "root-fatal-denied".to_string()
            });
        Ok(())
    }

    fn persist_root_fatal(
        &mut self,
        root_id: &str,
        code: &str,
    ) -> Result<Option<MigrationStateV1>> {
        let safety = self
            .root_safety
            .get_or_insert_with(|| create_migration_root_safety_state_v1(root_id));
        if safety.root_id != root_id {
            return Err(ProtocolError("MIGRATION_ROOT_BINDING_MISMATCH"));
        }
        if !safety
            .root_fatal_signals
            .iter()
            .any(|fatal| fatal.code == code)
        {
            safety.generation += 1;
            safety.root_fatal_signals.push(MigrationRootFatalV1 {
                code: code.to_string(),
            });
            safety
                .root_fatal_signals
                .sort_by(|a, b| a.code.cmp(&b.code));
        }
        let Some(state) = self.current.as_mut() else {
            return Ok(None);
        };
        if !state
            .root_fatal_signals
            .iter()
            .any(|fatal| fatal.code == code)
        {
            state.generation += 1;
            state.status = MigrationStatusV1::RootFrozen;
            state.root_fatal_signals.push(MigrationRootFatalV1 {
                code: code.to_string(),
            });
            state.root_fatal_signals.sort_by(|a, b| a.code.cmp(&b.code));
            self.history.push(state.clone());
        }
        Ok(Some(state.clone()))
    }

    fn run_publish_exclusive<T, F: FnOnce() -> Result<T>>(
        &mut self,
        root_id: &str,
        migration_id: &str,
        expected_generation: u64,
        _expected_execution_identity: Option<&MigrationExecutionIdentityV1>,
        operation: F,
    ) -> Result<PublishExclusiveResultV1<T>> {
        if self
            .external_fatal_signal
            .as_ref()
            .is_some_and(|signal| signal.swap(false, Ordering::SeqCst))
        {
            let mut cutover = self
                .cutover_state
                .clone()
                .unwrap_or_else(create_activation_cutover_state_v1);
            cutover.root_fatal_signals.push(ActivationCutoverFatalV1 {
                code: "SYNC_ROOT_FROZEN_LEGACY_CHANGE".to_string(),
            });
            self.persist_cutover_state(root_id, &cutover)?;
        }
        if self.freeze_before_publish {
            self.freeze_before_publish = false;
            let state = self.current.as_mut().unwrap();
            state.generation += 1;
            state.status = MigrationStatusV1::RootFrozen;
            state.root_fatal_signals.push(MigrationRootFatalV1 {
                code: "SYNC_ROOT_FROZEN_CORRUPTION".to_string(),
            });
        }
        let current = self.current.clone().unwrap();
        if current.root_id != root_id
            || current.migration_id != migration_id
            || current.generation != expected_generation
            || current.status == MigrationStatusV1::RootFrozen
            || !current.root_fatal_signals.is_empty()
        {
            self.actual_cutover_decisions
                .push("root-fatal-denied".to_string());
            return Ok(PublishExclusiveResultV1::Rejected(Box::new(current)));
        }
        self.actual_cutover_decisions
            .push("publish-eligible".to_string());
        let value = operation()?;
        if self
            .external_fatal_signal
            .as_ref()
            .is_some_and(|signal| signal.swap(false, Ordering::SeqCst))
        {
            let mut cutover = self
                .cutover_state
                .clone()
                .unwrap_or_else(create_activation_cutover_state_v1);
            cutover.root_fatal_signals.push(ActivationCutoverFatalV1 {
                code: "SYNC_ROOT_FROZEN_LEGACY_CHANGE".to_string(),
            });
            self.persist_cutover_state(root_id, &cutover)?;
        }
        Ok(PublishExclusiveResultV1::Executed(value))
    }
}
#[derive(Default)]
struct IntentStore(Vec<PreparedIntentV1>);
impl PreparedIntentStoreV1 for IntentStore {
    fn persist(&mut self, intent: &PreparedIntentV1) -> Result<()> {
        self.0.push(intent.clone());
        Ok(())
    }
}
#[derive(Default)]
struct ReceiptStore(Vec<RemotePublishedReceiptV1>);
impl PublishedReceiptStoreV1 for ReceiptStore {
    fn persist(&mut self, receipt: &RemotePublishedReceiptV1) -> Result<()> {
        self.0.push(receipt.clone());
        Ok(())
    }
}
#[derive(Default)]
struct ActivationIntentStore(Vec<PreparedActivationIntentV1>);
impl PreparedActivationIntentStoreV1 for ActivationIntentStore {
    fn persist(&mut self, intent: &PreparedActivationIntentV1) -> Result<()> {
        self.0.push(intent.clone());
        Ok(())
    }
}
#[derive(Default)]
struct ActivationReceiptStore(Vec<PublishedActivationReceiptV1>);
impl PublishedActivationReceiptStoreV1 for ActivationReceiptStore {
    fn persist(&mut self, receipt: &PublishedActivationReceiptV1) -> Result<()> {
        self.0.push(receipt.clone());
        Ok(())
    }
}
struct CutoverStore {
    authority_identity: u64,
    state: Option<ActivationCutoverStateV1>,
}
impl Default for CutoverStore {
    fn default() -> Self {
        Self {
            authority_identity: 301,
            state: None,
        }
    }
}
impl ActivationCutoverStateStoreV1 for CutoverStore {
    fn authority_identity(&self) -> u64 {
        self.authority_identity
    }

    fn load(&mut self) -> Result<Option<ActivationCutoverStateV1>> {
        Ok(self.state.clone())
    }

    fn persist(&mut self, state: &ActivationCutoverStateV1) -> Result<()> {
        self.state = Some(state.clone());
        Ok(())
    }
}

#[derive(Default)]
struct FakeStores {
    migration: MigrationStore,
    intent: IntentStore,
    receipt: ReceiptStore,
    activation_intent: ActivationIntentStore,
    activation_receipt: ActivationReceiptStore,
    cutover: CutoverStore,
}

fn execute(
    state: &MigrationStateV1,
    remote: &mut FakeRemote,
    stores: &mut FakeStores,
    timestamp: &str,
) -> MigrationStateV1 {
    if remote.root_id.is_empty() {
        remote.root_id = state.root_id.clone();
    }
    if stores.migration.current.is_none() {
        stores.migration.current = Some(state.clone());
    }
    let attachment = start_or_attach_migration_v1(state, &mut stores.migration).unwrap();
    let capability =
        create_migration_root_execution_capability_v1(&attachment, remote, &stores.migration)
            .unwrap();
    execute_migration_step_v1(
        state,
        &capability,
        remote,
        &mut stores.migration,
        &mut stores.intent,
        &mut stores.receipt,
        &mut stores.activation_intent,
        &mut stores.activation_receipt,
        timestamp,
    )
    .unwrap()
}

#[test]
fn shared_execution_scenarios_restart_and_finish_through_cutover() {
    let fixture = fixture();
    let timestamp = fixture["identity"]["createdAt"].as_str().unwrap();
    let mut normal = planned_for(&fixture, &fixture["planningScenarios"][2]);
    let mut normal_remote = FakeRemote {
        response_lost: true,
        ..FakeRemote::default()
    };
    let mut normal_stores = FakeStores::default();
    let mut normal_trace = Vec::new();
    while normal.status != MigrationStatusV1::ActivationVerified {
        normal = execute(&normal, &mut normal_remote, &mut normal_stores, timestamp);
        normal_trace.push(normal.status);
    }
    assert_eq!(
        serde_json::to_value(normal_trace).unwrap(),
        serde_json::Value::Array(fixture["standardStatusTrace"].as_array().unwrap()[..5].to_vec())
    );
    let _ = execute(&normal, &mut normal_remote, &mut normal_stores, timestamp);

    for scenario in &fixture["executionScenarios"].as_array().unwrap()[..4] {
        let plan_name = scenario["planningScenario"].as_str().unwrap();
        let plan_scenario = fixture["planningScenarios"]
            .as_array()
            .unwrap()
            .iter()
            .find(|value| value["name"] == plan_name)
            .unwrap();
        let mut state = planned_for(&fixture, plan_scenario);
        let mut remote = FakeRemote::default();
        let mut stores = FakeStores::default();
        if scenario["name"] == "restart-during-stage-a" {
            state = execute(&state, &mut remote, &mut stores, timestamp);
        } else if scenario["name"] == "restart-during-activation-publish" {
            while state.status != MigrationStatusV1::ActivationPublishing {
                state = execute(&state, &mut remote, &mut stores, timestamp);
            }
        } else {
            state = execute(&state, &mut remote, &mut stores, timestamp);
        }
        assert_eq!(
            serde_json::to_value(state.status).unwrap(),
            scenario["expectedRestartStatus"],
            "{}",
            scenario["name"]
        );
        let first_stage_path = state.stage_a[0].intent.remote_path.clone();
        let puts_before_restart = remote.put_counts.get(&first_stage_path).copied();
        state = serde_json::from_slice(&serde_json::to_vec(&state).unwrap()).unwrap();
        while state.status != MigrationStatusV1::ActivationVerified {
            state = execute(&state, &mut remote, &mut stores, timestamp);
            if state.status == MigrationStatusV1::StageBPublishing {
                assert!(state.stage_a.iter().all(|task| task.receipt.is_some()));
            }
        }
        assert_eq!(state.status, MigrationStatusV1::ActivationVerified);
        let _ = execute(&state, &mut remote, &mut stores, timestamp);
        assert!(
            stores
                .migration
                .cutover_state
                .as_ref()
                .unwrap()
                .remote_s2_activated
        );
        if scenario["expectedNoRepublish"].as_bool() == Some(true) {
            assert_eq!(
                remote.put_counts.get(&first_stage_path).copied(),
                puts_before_restart
            );
        }
    }
}

#[test]
fn admitted_bootstrap_and_activation_preflight_after_collection_preparation() {
    let fixture = fixture();
    let timestamp = fixture["identity"]["createdAt"].as_str().unwrap();

    // An unreceipted mutable attempt enters SQLite admission before its first
    // publication-decision GET, then prepares canonical parents and performs
    // the one authoritative exact preflight.
    let mut bootstrap = planned_for(&fixture, &fixture["planningScenarios"][1]);
    bootstrap.status = MigrationStatusV1::StageAPublishing;
    let mut bootstrap_remote = FakeRemote {
        root_id: bootstrap.root_id.clone(),
        scripted_gets: VecDeque::from([RemoteExactGetResultV1::DefinitelyAbsent]),
        record_preparation: true,
        ..FakeRemote::default()
    };
    let mut bootstrap_stores = FakeStores::default();
    bootstrap_stores.migration.current = Some(bootstrap.clone());
    let bootstrap_next = execute(
        &bootstrap,
        &mut bootstrap_remote,
        &mut bootstrap_stores,
        timestamp,
    );
    assert_eq!(bootstrap_next.status, MigrationStatusV1::StageAComplete);
    assert_eq!(bootstrap_remote.preparation_calls, 1);
    assert_eq!(bootstrap_remote.trace, ["PREPARE", "GET", "PUT", "GET"]);

    let mut activation = planned_for(&fixture, &fixture["planningScenarios"][2]);
    let mut setup_remote = FakeRemote::default();
    let mut activation_stores = FakeStores::default();
    while activation.status != MigrationStatusV1::ActivationPublishing {
        activation = execute(
            &activation,
            &mut setup_remote,
            &mut activation_stores,
            timestamp,
        );
    }
    let mut activation_remote = FakeRemote {
        root_id: activation.root_id.clone(),
        scripted_gets: VecDeque::from([RemoteExactGetResultV1::DefinitelyAbsent]),
        record_preparation: true,
        ..FakeRemote::default()
    };
    let activation_next = execute(
        &activation,
        &mut activation_remote,
        &mut activation_stores,
        timestamp,
    );
    assert_eq!(
        activation_next.status,
        MigrationStatusV1::ActivationVerified
    );
    assert_eq!(activation_remote.preparation_calls, 1);
    assert_eq!(activation_remote.trace, ["PREPARE", "GET", "PUT", "GET"]);

    let mut frozen = planned_for(&fixture, &fixture["planningScenarios"][1]);
    frozen.status = MigrationStatusV1::StageAPublishing;
    let mut frozen_remote = FakeRemote::default();
    let mut frozen_stores = FakeStores::default();
    frozen_stores.migration.current = Some(frozen.clone());
    frozen_stores.migration.freeze_before_publish = true;
    assert_eq!(
        execute(&frozen, &mut frozen_remote, &mut frozen_stores, timestamp).status,
        MigrationStatusV1::RootFrozen
    );
    assert_eq!(frozen_remote.preparation_calls, 0);
    assert!(frozen_remote.put_counts.is_empty());
    assert!(frozen_remote.trace.is_empty());
}

#[test]
fn deterministic_replan_and_frozen_new_root_handoff_match_shared_contract() {
    let fixture = fixture();
    let first = planned_for(&fixture, &fixture["planningScenarios"][2]);
    let second = planned_for(&fixture, &fixture["planningScenarios"][2]);
    assert_eq!(
        migration_projection_v1(&first),
        migration_projection_v1(&second)
    );
    assert_eq!(first.stage_a, second.stage_a);
    assert_eq!(first.stage_b, second.stage_b);

    let handoff = fixture["executionScenarios"]
        .as_array()
        .unwrap()
        .last()
        .unwrap();
    let frozen = freeze_old_root_for_new_root_handoff_v1(
        &first,
        &[handoff["fatalCode"].as_str().unwrap().to_string()],
    )
    .unwrap();
    let mut frozen_remote = FakeRemote::default();
    let mut frozen_stores = FakeStores::default();
    let unchanged = execute(
        &frozen,
        &mut frozen_remote,
        &mut frozen_stores,
        fixture["identity"]["createdAt"].as_str().unwrap(),
    );
    assert_eq!(unchanged, frozen);
    assert!(frozen_remote.objects.is_empty());
    let identity = &fixture["identity"];
    let next = create_new_root_migration_handoff_v1(
        &frozen,
        identity["newMigrationId"].as_str().unwrap(),
        identity["newRootId"].as_str().unwrap(),
        identity["newWriterId"].as_str().unwrap(),
        identity["createdAt"].as_str().unwrap(),
    )
    .unwrap();
    let planned = plan_captured_migration_v1(&next).unwrap();
    assert_eq!(frozen.status, MigrationStatusV1::RootFrozen);
    assert_eq!(planned.source_type, handoff["expectedSourceType"]);
    assert_eq!(
        planned.snapshot.as_ref().unwrap().legacy_fingerprint,
        first.snapshot.as_ref().unwrap().legacy_fingerprint
    );
    let mut new_root_remote = FakeRemote {
        response_lost: true,
        ..FakeRemote::default()
    };
    let mut new_root_stores = FakeStores::default();
    let mut completed = planned;
    while completed.status != MigrationStatusV1::ActivationVerified {
        completed = execute(
            &completed,
            &mut new_root_remote,
            &mut new_root_stores,
            identity["createdAt"].as_str().unwrap(),
        );
    }
    let _ = execute(
        &completed,
        &mut new_root_remote,
        &mut new_root_stores,
        identity["createdAt"].as_str().unwrap(),
    );
    assert!(
        new_root_stores
            .migration
            .cutover_state
            .unwrap()
            .remote_s2_activated
    );
}

#[test]
fn activation_same_path_mismatch_freezes_without_overwrite() {
    let fixture = fixture();
    let timestamp = fixture["identity"]["createdAt"].as_str().unwrap();
    let mut state = planned_for(&fixture, &fixture["planningScenarios"][1]);
    let mut remote = FakeRemote::default();
    let mut stores = FakeStores::default();
    while state.status != MigrationStatusV1::StageAComplete {
        state = execute(&state, &mut remote, &mut stores, timestamp);
    }
    let path = state
        .activation_intent
        .as_ref()
        .unwrap()
        .remote_path
        .clone();
    remote.objects.insert(path.clone(), vec![1, 2, 3]);
    state = execute(&state, &mut remote, &mut stores, timestamp);
    assert_eq!(state.status, MigrationStatusV1::RootFrozen);
    assert_eq!(
        state.root_fatal_signals[0].code,
        "SYNC_ROOT_FROZEN_CORRUPTION"
    );
    assert_eq!(remote.objects[&path], vec![1, 2, 3]);
    assert!(!remote.put_counts.contains_key(&path));
}

#[test]
fn empty_migration_with_lost_publish_response_verifies_before_cutover() {
    let fixture = fixture();
    let timestamp = fixture["identity"]["createdAt"].as_str().unwrap();
    let mut state = planned_for(&fixture, &fixture["planningScenarios"][0]);
    let mut remote = FakeRemote {
        response_lost: true,
        ..FakeRemote::default()
    };
    let mut stores = FakeStores::default();
    let mut trace = Vec::new();
    while state.status != MigrationStatusV1::ActivationVerified {
        state = execute(&state, &mut remote, &mut stores, timestamp);
        trace.push(state.status);
    }
    assert_eq!(
        serde_json::to_value(trace).unwrap(),
        serde_json::Value::Array(fixture["emptyStatusTrace"].as_array().unwrap()[..1].to_vec())
    );
    let _ = execute(&state, &mut remote, &mut stores, timestamp);
    assert_eq!(remote.objects.len(), 1);
    assert!(stores.migration.cutover_state.unwrap().remote_s2_activated);
}

#[test]
fn serialized_snapshot_corruption_is_rejected() {
    let fixture = fixture();
    let state = planned_for(&fixture, &fixture["planningScenarios"][2]);
    let mut encoded = serde_json::to_value(state).unwrap();
    encoded["snapshot"]["canonicalEntities"][0]["value"]["originalName"] = json!("CORRUPTED");
    let decoded: MigrationStateV1 = serde_json::from_value(encoded).unwrap();
    assert_eq!(
        super::migration_orchestration::reconcile_migration_state_v1(&decoded),
        Err(ProtocolError("LOCAL_MIGRATION_STATE_CORRUPTION"))
    );
    let mut identity_changed = planned_for(&fixture, &fixture["planningScenarios"][2]);
    identity_changed.migration_id = fixture["identity"]["newMigrationId"]
        .as_str()
        .unwrap()
        .to_string();
    assert_eq!(
        super::migration_orchestration::reconcile_migration_state_v1(&identity_changed),
        Err(ProtocolError("LOCAL_MIGRATION_STATE_CORRUPTION"))
    );
}

#[test]
fn shared_root_binding_failures_reject_confusion_receipts_and_same_root_handoff() {
    let fixture = fixture();
    let failures = fixture["failureScenarios"].as_array().unwrap();
    assert_eq!(failures[0]["name"], "root-a-state-on-root-b");
    assert_eq!(failures[1]["name"], "old-receipt-on-new-root");
    assert_eq!(failures[2]["name"], "same-root-handoff");
    let timestamp = fixture["identity"]["createdAt"].as_str().unwrap();
    let planned = planned_for(&fixture, &fixture["planningScenarios"][1]);
    let mut wrong_remote = FakeRemote {
        root_id: fixture["identity"]["newRootId"]
            .as_str()
            .unwrap()
            .to_string(),
        ..FakeRemote::default()
    };
    let mut stores = FakeStores::default();
    stores.migration.current = Some(planned.clone());
    let attachment = start_or_attach_migration_v1(&planned, &mut stores.migration).unwrap();
    let bound_remote = FakeRemote {
        root_id: planned.root_id.clone(),
        ..FakeRemote::default()
    };
    let capability = create_migration_root_execution_capability_v1(
        &attachment,
        &bound_remote,
        &stores.migration,
    )
    .unwrap();
    assert_eq!(
        execute_migration_step_v1(
            &planned,
            &capability,
            &mut wrong_remote,
            &mut stores.migration,
            &mut stores.intent,
            &mut stores.receipt,
            &mut stores.activation_intent,
            &mut stores.activation_receipt,
            timestamp,
        ),
        Err(ProtocolError("MIGRATION_ROOT_BINDING_MISMATCH"))
    );
    assert!(wrong_remote.put_counts.is_empty());
    assert_eq!(
        serde_json::to_value(&wrong_remote.trace).unwrap(),
        failures[0]["expectedOperations"]
    );

    let mut remote = FakeRemote::default();
    let mut stores = FakeStores::default();
    let mut receipted = planned.clone();
    while receipted.status != MigrationStatusV1::StageAComplete {
        receipted = execute(&receipted, &mut remote, &mut stores, timestamp);
    }
    let mut moved = receipted.clone();
    moved.root_id = fixture["identity"]["newRootId"]
        .as_str()
        .unwrap()
        .to_string();
    assert_eq!(
        super::migration_orchestration::reconcile_migration_state_v1(&moved),
        Err(ProtocolError("LOCAL_MIGRATION_STATE_CORRUPTION"))
    );
    let frozen = freeze_old_root_for_new_root_handoff_v1(
        &receipted,
        &["SYNC_ROOT_FROZEN_CORRUPTION".to_string()],
    )
    .unwrap();
    assert_eq!(
        create_new_root_migration_handoff_v1(
            &frozen,
            fixture["identity"]["newMigrationId"].as_str().unwrap(),
            &frozen.root_id,
            fixture["identity"]["newWriterId"].as_str().unwrap(),
            timestamp,
        ),
        Err(ProtocolError("invalid_frozen_root_handoff"))
    );
}

#[test]
fn fatal_authority_and_interleaved_freeze_block_every_put() {
    let fixture = fixture();
    let failure = |name: &str| {
        fixture["failureScenarios"]
            .as_array()
            .unwrap()
            .iter()
            .find(|value| value["name"] == name)
            .unwrap()
    };
    let timestamp = fixture["identity"]["createdAt"].as_str().unwrap();
    let planned = planned_for(&fixture, &fixture["planningScenarios"][1]);
    let frozen = freeze_old_root_for_new_root_handoff_v1(
        &planned,
        &["SYNC_ROOT_FROZEN_WRITER_FORK".to_string()],
    )
    .unwrap();
    let mut remote = FakeRemote::default();
    let mut stores = FakeStores::default();
    stores.migration.current = Some(frozen.clone());
    let retained = execute(&planned, &mut remote, &mut stores, timestamp);
    assert_eq!(retained.status, MigrationStatusV1::RootFrozen);
    assert_eq!(retained.root_fatal_signals, frozen.root_fatal_signals);
    assert!(remote.put_counts.is_empty());
    assert_eq!(
        serde_json::to_value(&remote.trace).unwrap(),
        failure("known-fatal-blocks-publish")["expectedOperations"]
    );

    let mut remote = FakeRemote::default();
    let mut stores = FakeStores::default();
    stores.migration.freeze_before_publish = true;
    let stopped = execute(&planned, &mut remote, &mut stores, timestamp);
    assert_eq!(stopped.status, MigrationStatusV1::RootFrozen);
    assert!(remote.put_counts.is_empty());
    assert_eq!(
        serde_json::to_value(&remote.trace).unwrap(),
        failure("freeze-before-exclusive-publish")["expectedOperations"]
    );
    assert_eq!(
        stores.migration.current.unwrap().status,
        MigrationStatusV1::RootFrozen
    );
}

#[test]
fn activation_receipt_recovers_cutover_and_stale_complete_is_not_ready() {
    let fixture = fixture();
    let expected_operations = fixture["failureScenarios"]
        .as_array()
        .unwrap()
        .iter()
        .find(|value| value["name"] == "activation-cutover-crash")
        .unwrap()["expectedOperations"]
        .clone();
    let timestamp = fixture["identity"]["createdAt"].as_str().unwrap();
    let mut state = planned_for(&fixture, &fixture["planningScenarios"][0]);
    let mut remote = FakeRemote {
        response_lost: true,
        ..FakeRemote::default()
    };
    let mut stores = FakeStores::default();
    while state.status != MigrationStatusV1::ActivationVerified {
        state = execute(&state, &mut remote, &mut stores, timestamp);
    }
    assert_eq!(
        serde_json::to_value(&remote.trace).unwrap(),
        expected_operations
    );
    assert!(stores.cutover.state.is_none());
    let recovery = recover_migration_activation_cutover_v1(&state, &mut stores.cutover).unwrap();
    assert_eq!(
        decide_legacy_put_v1(&recovery),
        LegacyPutDecisionV1::DeniedRemoteS2Activated
    );
    let restarted: MigrationStateV1 =
        serde_json::from_slice(&serde_json::to_vec(&state).unwrap()).unwrap();
    assert_eq!(
        execute(&restarted, &mut remote, &mut stores, timestamp).status,
        MigrationStatusV1::ActivationVerified
    );

    let mut stale = planned_for(&fixture, &fixture["planningScenarios"][0]);
    stale.status = MigrationStatusV1::MigrationComplete;
    let mut empty_cutover = CutoverStore::default();
    let recovery = recover_migration_activation_cutover_v1(&stale, &mut empty_cutover).unwrap();
    assert_eq!(
        decide_legacy_put_v1(&recovery),
        LegacyPutDecisionV1::DeniedCutoverRecoveryNotReady
    );
}

#[test]
fn atomic_start_or_attach_and_activation_durable_boundary_are_enforced() {
    let fixture = fixture();
    let identity = &fixture["identity"];
    let timestamp = identity["createdAt"].as_str().unwrap();
    let first = create_migration_state_v1(
        identity["migrationId"].as_str().unwrap(),
        identity["rootId"].as_str().unwrap(),
        identity["writerId"].as_str().unwrap(),
        timestamp,
        "legacy-bootstrap",
    )
    .unwrap();
    let second = create_migration_state_v1(
        identity["newMigrationId"].as_str().unwrap(),
        identity["rootId"].as_str().unwrap(),
        identity["newWriterId"].as_str().unwrap(),
        timestamp,
        "legacy-bootstrap",
    )
    .unwrap();
    let mut store = MigrationStore::default();
    let attached_a = start_or_attach_migration_v1(&first, &mut store).unwrap();
    let attached_b = start_or_attach_migration_v1(&second, &mut store).unwrap();
    assert_eq!(attached_a.state.migration_id, attached_b.state.migration_id);
    assert_eq!(store.history.len(), 1);

    let mut state = planned_for(&fixture, &fixture["planningScenarios"][0]);
    let mut remote = FakeRemote::default();
    let mut stores = FakeStores::default();
    while state.status != MigrationStatusV1::ActivationVerified {
        state = execute(&state, &mut remote, &mut stores, timestamp);
    }
    assert_eq!(stores.activation_intent.0.len(), 1);
    assert_eq!(stores.activation_receipt.0.len(), 1);
    assert_eq!(remote.put_counts.values().sum::<usize>(), 1);
}

#[test]
fn shared_stage_b_partial_crash_never_republishes_stage_a() {
    let fixture = fixture();
    let timestamp = fixture["identity"]["createdAt"].as_str().unwrap();
    let scenario = fixture["planningScenarios"]
        .as_array()
        .unwrap()
        .iter()
        .find(|value| value["name"] == "stage-b-more-than-256")
        .unwrap();
    let mut state = planned_for(&fixture, scenario);
    let mut remote = FakeRemote {
        response_lost: true,
        ..FakeRemote::default()
    };
    let mut stores = FakeStores::default();
    while !(state.status == MigrationStatusV1::StageBPublishing
        && state.stage_b[0].receipt.is_some())
    {
        state = execute(&state, &mut remote, &mut stores, timestamp);
    }
    let stage_a_puts = state
        .stage_a
        .iter()
        .map(|task| remote.put_counts.get(&task.intent.remote_path).copied())
        .collect::<Vec<_>>();
    state = serde_json::from_slice(&serde_json::to_vec(&state).unwrap()).unwrap();
    while state.status != MigrationStatusV1::ActivationVerified {
        state = execute(&state, &mut remote, &mut stores, timestamp);
    }
    assert_eq!(
        state
            .stage_a
            .iter()
            .map(|task| remote.put_counts.get(&task.intent.remote_path).copied())
            .collect::<Vec<_>>(),
        stage_a_puts
    );
    assert!(state.stage_b.iter().all(|task| task.receipt.is_some()));
}

#[test]
fn invalid_legacy_adapter_is_fail_closed() {
    let broken = LegacySnapshotEntryV1 {
        entity_type: "record".to_string(),
        value: json!({ "broken": true }),
    };
    assert_eq!(
        capture_legacy_snapshot_v1(&[broken], &IdentityAdapter),
        Err(ProtocolError("LEGACY_SNAPSHOT_INVALID"))
    );
}

fn correctness_scenario<'a>(fixture: &'a Value, name: &str) -> &'a Value {
    fixture["correctnessScenarios"]
        .as_array()
        .unwrap()
        .iter()
        .find(|value| value["name"] == name)
        .unwrap()
}

fn install_cutover_fatal(stores: &mut FakeStores) {
    let fingerprint_one = "f".repeat(64);
    let fingerprint_two = "e".repeat(64);
    let state = cutover_input(vec![
        activation_evidence("activations/a.json", &fingerprint_one, 'a'),
        activation_evidence("activations/b.json", &fingerprint_two, 'b'),
    ]);
    let root_id = stores.migration.current.as_ref().unwrap().root_id.clone();
    stores
        .migration
        .persist_cutover_state(&root_id, &state)
        .unwrap();
}

fn activation_evidence(
    path: &str,
    fingerprint: &str,
    hash_character: char,
) -> VerifiedActivationEvidenceV1 {
    VerifiedActivationEvidenceV1 {
        path: path.to_string(),
        activation_id: "e9804389-6d1e-4377-9133-94c75c6e11c1".to_string(),
        content_hash: hash_character.to_string().repeat(64),
        exact_bytes_hash: hash_character.to_string().repeat(64),
        legacy_fingerprint: Some(fingerprint.to_string()),
    }
}

fn cutover_input(evidence: Vec<VerifiedActivationEvidenceV1>) -> ActivationCutoverStateV1 {
    ActivationCutoverStateV1 {
        state_version: 1,
        remote_s2_activated: !evidence.is_empty(),
        verified_activation_evidence: evidence,
        fingerprint_consistency: ActivationFingerprintConsistencyV1::NoEvidence,
        root_fatal_signals: vec![],
    }
}

fn assert_correctness_projection(scenario: &Value, state: &MigrationStateV1, remote: &FakeRemote) {
    assert_eq!(state.root_id, scenario["expectedActualRoot"]);
    assert_eq!(
        serde_json::to_value(&remote.detailed_trace).unwrap(),
        scenario["expectedOperations"]
    );
    assert_eq!(
        remote
            .detailed_trace
            .iter()
            .any(|value| value["operation"] == "PUT"),
        scenario["expectedPutOccurred"].as_bool().unwrap()
    );
    assert_eq!(
        state.generation,
        scenario["expectedGeneration"].as_u64().unwrap()
    );
    assert_eq!(
        serde_json::to_value(state.status).unwrap(),
        scenario["expectedStatus"]
    );
    assert_eq!(
        state.snapshot.as_ref().unwrap().legacy_fingerprint,
        scenario["expectedSnapshotIdentity"]
    );
    assert_eq!(
        state.stage_a[0].intent.intent_fingerprint,
        scenario["expectedPlanIdentity"]
    );
    let receipts = state
        .stage_a
        .iter()
        .chain(&state.stage_b)
        .filter(|task| task.receipt.is_some())
        .count();
    assert_eq!(
        receipts,
        scenario["expectedRetainedReceipts"].as_u64().unwrap() as usize
    );
    assert_eq!(
        serde_json::to_value(
            state
                .root_fatal_signals
                .iter()
                .map(|fatal| &fatal.code)
                .collect::<Vec<_>>()
        )
        .unwrap(),
        scenario["expectedFatalCodes"]
    );
}

#[test]
fn stale_authority_identity_cannot_execute_a_live_capability() {
    let fixture = fixture();
    let scenario = correctness_scenario(&fixture, "stale-authority-copy");
    let timestamp = fixture["identity"]["createdAt"].as_str().unwrap();
    let planned = planned_for(&fixture, &fixture["planningScenarios"][1]);
    let mut remote = FakeRemote {
        root_id: planned.root_id.clone(),
        ..FakeRemote::default()
    };
    let mut live = FakeStores::default();
    live.migration.current = Some(planned.clone());
    let attachment = start_or_attach_migration_v1(&planned, &mut live.migration).unwrap();
    let capability =
        create_migration_root_execution_capability_v1(&attachment, &remote, &live.migration)
            .unwrap();
    let mut frozen = planned.clone();
    frozen.generation = 1;
    frozen.status = MigrationStatusV1::RootFrozen;
    frozen.root_fatal_signals.push(MigrationRootFatalV1 {
        code: "SYNC_ROOT_FROZEN_LEGACY_CHANGE".to_string(),
    });
    live.migration.current = Some(frozen.clone());

    let mut stale = FakeStores::default();
    stale.migration.authority_identity = 202;
    stale.migration.current = Some(planned.clone());
    assert_eq!(
        execute_migration_step_v1(
            &planned,
            &capability,
            &mut remote,
            &mut stale.migration,
            &mut stale.intent,
            &mut stale.receipt,
            &mut stale.activation_intent,
            &mut stale.activation_receipt,
            timestamp,
        ),
        Err(ProtocolError("MIGRATION_ROOT_BINDING_MISMATCH"))
    );
    assert_correctness_projection(scenario, live.migration.current.as_ref().unwrap(), &remote);
    assert_eq!(stale.migration.current.unwrap(), planned);
}

#[test]
fn same_generation_snapshot_replacement_keeps_original_receipt_and_seq1() {
    let fixture = fixture();
    let scenario = correctness_scenario(&fixture, "same-generation-snapshot-replacement");
    let timestamp = fixture["identity"]["createdAt"].as_str().unwrap();
    let planned_x = planned_for(&fixture, &fixture["planningScenarios"][1]);
    let mut remote = FakeRemote::default();
    let mut stores = FakeStores::default();
    let durable_x = execute(&planned_x, &mut remote, &mut stores, timestamp);
    assert_eq!(durable_x.status, MigrationStatusV1::StageAComplete);
    let original_path = durable_x.stage_a[0].intent.remote_path.clone();
    remote.trace.clear();
    remote.detailed_trace.clear();
    let mut proposed_y = planned_for(&fixture, &fixture["planningScenarios"][2]);
    proposed_y.generation = durable_x.generation;
    let attachment = start_or_attach_migration_v1(&durable_x, &mut stores.migration).unwrap();
    let capability =
        create_migration_root_execution_capability_v1(&attachment, &remote, &stores.migration)
            .unwrap();
    assert_eq!(
        execute_migration_step_v1(
            &proposed_y,
            &capability,
            &mut remote,
            &mut stores.migration,
            &mut stores.intent,
            &mut stores.receipt,
            &mut stores.activation_intent,
            &mut stores.activation_receipt,
            timestamp,
        ),
        Err(ProtocolError("LOCAL_MIGRATION_STATE_CORRUPTION"))
    );
    let retained = stores.migration.current.as_ref().unwrap();
    assert_correctness_projection(scenario, retained, &remote);
    assert_eq!(remote.objects.len(), 1);
    assert!(remote.objects.contains_key(&original_path));
    let commits = remote
        .objects
        .values()
        .map(|bytes| decode_frozen_wire_commit_v1(bytes).unwrap())
        .collect::<Vec<_>>();
    assert!(detect_writer_forks_v1(&commits).is_empty());
}

#[test]
fn cutover_fatal_and_combined_stale_copy_block_publish_at_the_gate() {
    let fixture = fixture();
    let timestamp = fixture["identity"]["createdAt"].as_str().unwrap();
    for name in [
        "cutover-fatal-blocks-publish",
        "combined-stale-context-cutover-fatal",
    ] {
        let scenario = correctness_scenario(&fixture, name);
        let planned = planned_for(&fixture, &fixture["planningScenarios"][1]);
        let mut remote = FakeRemote::default();
        let mut stores = FakeStores::default();
        stores.migration.current = Some(planned.clone());
        install_cutover_fatal(&mut stores);
        let stale_copy = MigrationStore {
            authority_identity: 999,
            current: Some(planned.clone()),
            ..MigrationStore::default()
        };
        assert_ne!(
            stale_copy.authority_identity,
            stores.migration.authority_identity
        );
        let result = execute(&planned, &mut remote, &mut stores, timestamp);
        assert_correctness_projection(scenario, &result, &remote);
        assert!(remote.put_counts.is_empty());
    }
}

fn final_race_scenario<'a>(fixture: &'a Value, name: &str) -> &'a Value {
    fixture["finalRaceScenarios"]
        .as_array()
        .unwrap()
        .iter()
        .find(|value| value["name"] == name)
        .unwrap()
}

fn assert_final_race_projection(
    scenario: &Value,
    state: &MigrationStateV1,
    remote: &FakeRemote,
    authoritative_fatal_codes: Option<Vec<String>>,
) {
    assert_eq!(state.root_id, scenario["expectedActualRoot"]);
    assert_eq!(
        serde_json::to_value(&remote.detailed_trace).unwrap(),
        scenario["expectedOperations"]
    );
    assert_eq!(
        remote
            .detailed_trace
            .iter()
            .filter(|value| value["operation"] == "PUT")
            .count(),
        scenario["expectedPutCount"].as_u64().unwrap() as usize
    );
    assert_eq!(
        state.generation,
        scenario["expectedGeneration"].as_u64().unwrap()
    );
    assert_eq!(
        serde_json::to_value(state.status).unwrap(),
        scenario["expectedStatus"]
    );
    assert_eq!(
        state.snapshot.as_ref().unwrap().legacy_fingerprint,
        scenario["expectedSnapshotIdentity"]
    );
    assert_eq!(
        state
            .stage_a
            .first()
            .map(|task| Value::String(task.intent.intent_fingerprint.clone()))
            .unwrap_or(Value::Null),
        scenario["expectedPlanIdentity"]
    );
    assert_eq!(
        state
            .stage_a
            .iter()
            .chain(&state.stage_b)
            .filter(|task| task.receipt.is_some())
            .count(),
        scenario["expectedRetainedReceipts"].as_u64().unwrap() as usize
    );
    let fatal_codes = authoritative_fatal_codes.unwrap_or_else(|| {
        state
            .root_fatal_signals
            .iter()
            .map(|fatal| fatal.code.clone())
            .collect()
    });
    assert_eq!(
        serde_json::to_value(fatal_codes).unwrap(),
        scenario["expectedFatalCodes"]
    );
}

fn assert_final_decision_projection(
    scenario: &Value,
    cutover_decision: &str,
    recovery_decision: Option<&str>,
) {
    assert_eq!(
        json!({
            "cutoverDecision": cutover_decision,
            "recoveryDecision": recovery_decision,
        }),
        json!({
            "cutoverDecision": scenario["expectedCutoverDecision"],
            "recoveryDecision": scenario["expectedRecoveryDecision"],
        })
    );
}

#[test]
fn durable_captured_snapshot_cannot_be_replaced_before_planning() {
    let fixture = fixture();
    let scenario = final_race_scenario(&fixture, "captured-x-restart-proposed-y");
    let identity = &fixture["identity"];
    let timestamp = identity["createdAt"].as_str().unwrap();
    let entities = entities_for(&fixture["planningScenarios"][1], timestamp);
    let entries = entities
        .iter()
        .map(|entity| LegacySnapshotEntryV1 {
            entity_type: entity.entity_type.clone(),
            value: serde_json::to_value(entity).unwrap(),
        })
        .collect::<Vec<_>>();
    let snapshot_x = capture_legacy_snapshot_v1(&entries, &IdentityAdapter).unwrap();
    let initial = create_migration_state_v1(
        identity["migrationId"].as_str().unwrap(),
        identity["rootId"].as_str().unwrap(),
        identity["writerId"].as_str().unwrap(),
        timestamp,
        "legacy-bootstrap",
    )
    .unwrap();
    let captured_x = retain_captured_snapshot_v1(&initial, &snapshot_x).unwrap();
    let mut planned_y = planned_for(&fixture, &fixture["planningScenarios"][2]);
    planned_y.generation = captured_x.generation;
    let mut remote = FakeRemote {
        root_id: captured_x.root_id.clone(),
        ..FakeRemote::default()
    };
    let mut stores = FakeStores::default();
    stores.migration.current = Some(captured_x.clone());
    let attachment = start_or_attach_migration_v1(&captured_x, &mut stores.migration).unwrap();
    let capability =
        create_migration_root_execution_capability_v1(&attachment, &remote, &stores.migration)
            .unwrap();
    assert_eq!(
        execute_migration_step_v1(
            &planned_y,
            &capability,
            &mut remote,
            &mut stores.migration,
            &mut stores.intent,
            &mut stores.receipt,
            &mut stores.activation_intent,
            &mut stores.activation_receipt,
            timestamp,
        ),
        Err(ProtocolError("LOCAL_MIGRATION_STATE_CORRUPTION"))
    );
    assert_final_race_projection(
        scenario,
        stores.migration.current.as_ref().unwrap(),
        &remote,
        None,
    );
}

#[test]
fn fatal_during_each_read_only_preflight_wins_before_put_admission() {
    let fixture = fixture();
    let timestamp = fixture["identity"]["createdAt"].as_str().unwrap();
    for (name, target_status) in [
        (
            "stage-a-fatal-during-preflight",
            MigrationStatusV1::BootstrapPlanned,
        ),
        (
            "stage-b-fatal-during-preflight",
            MigrationStatusV1::StageBPublishing,
        ),
        (
            "activation-fatal-during-preflight",
            MigrationStatusV1::ActivationPublishing,
        ),
    ] {
        let scenario = final_race_scenario(&fixture, name);
        let planning = if name.starts_with("stage-a") {
            &fixture["planningScenarios"][1]
        } else {
            &fixture["planningScenarios"][2]
        };
        let mut state = planned_for(&fixture, planning);
        let mut remote = FakeRemote::default();
        let mut stores = FakeStores::default();
        while state.status != target_status {
            state = execute(&state, &mut remote, &mut stores, timestamp);
        }
        if stores.migration.current.is_none() {
            stores.migration.current = Some(state.clone());
        }
        remote.trace.clear();
        remote.detailed_trace.clear();
        let recovery_decision = if target_status == MigrationStatusV1::ActivationPublishing {
            match restart_durable_activation_publish_v1(
                state.activation_intent.as_ref().unwrap(),
                None,
                &mut remote,
                timestamp,
            )
            .unwrap()
            {
                RecoverActivationIntentResultV1::RetryPublishExact => "RetryPublishExact",
                _ => panic!("unexpected activation recovery decision"),
            }
        } else {
            let task = if target_status == MigrationStatusV1::BootstrapPlanned {
                &state.stage_a[0]
            } else {
                &state.stage_b[0]
            };
            match restart_durable_publish_v1(&task.intent, None, &mut remote, timestamp).unwrap() {
                RecoverPreparedIntentResultV1::RetryPublishExact => "RetryPublishExact",
                _ => panic!("unexpected commit recovery decision"),
            }
        };
        install_cutover_fatal(&mut stores);
        state = execute(&state, &mut remote, &mut stores, timestamp);
        assert_final_race_projection(scenario, &state, &remote, None);
        assert_final_decision_projection(
            scenario,
            stores.migration.actual_cutover_decisions.last().unwrap(),
            Some(recovery_decision),
        );
        assert!(
            remote.put_counts.values().sum::<usize>()
                <= scenario["expectedRetainedReceipts"].as_u64().unwrap() as usize
        );
    }
}

#[test]
fn admitted_put_orders_before_later_fatal_and_next_mutation_is_blocked() {
    let fixture = fixture();
    let timestamp = fixture["identity"]["createdAt"].as_str().unwrap();
    let admission_scenario = final_race_scenario(&fixture, "admission-wins-then-fatal");
    let blocked_scenario = final_race_scenario(&fixture, "fatal-after-admitted-put-blocks-next");
    let mut state = planned_for(&fixture, &fixture["planningScenarios"][1]);
    let mut remote = FakeRemote {
        root_id: state.root_id.clone(),
        ..FakeRemote::default()
    };
    let mut stores = FakeStores::default();
    stores.migration.current = Some(state.clone());
    let recovery_decision =
        match restart_durable_publish_v1(&state.stage_a[0].intent, None, &mut remote, timestamp)
            .unwrap()
        {
            RecoverPreparedIntentResultV1::RetryPublishExact => "RetryPublishExact",
            _ => panic!("unexpected commit recovery decision"),
        };
    let signal = Arc::new(AtomicBool::new(false));
    remote.fatal_signal = Some(Arc::clone(&signal));
    remote.signal_fatal_on_put = true;
    stores.migration.external_fatal_signal = Some(signal);
    state = execute(&state, &mut remote, &mut stores, timestamp);
    assert_final_race_projection(admission_scenario, &state, &remote, None);
    assert_final_decision_projection(
        admission_scenario,
        &stores.migration.actual_cutover_decisions[0],
        Some(recovery_decision),
    );

    let mut stale_safe_cutover = stores.migration.cutover_state.clone().unwrap();
    stale_safe_cutover.root_fatal_signals.clear();
    stores
        .migration
        .persist_cutover_state(&state.root_id, &stale_safe_cutover)
        .unwrap();
    assert_eq!(
        stores
            .migration
            .cutover_state
            .as_ref()
            .unwrap()
            .root_fatal_signals,
        vec![ActivationCutoverFatalV1 {
            code: "SYNC_ROOT_FROZEN_LEGACY_CHANGE".to_string()
        }]
    );

    let trace_before_retry = remote.detailed_trace.clone();
    state = execute(&state, &mut remote, &mut stores, timestamp);
    assert_eq!(remote.detailed_trace, trace_before_retry);
    assert_final_race_projection(blocked_scenario, &state, &remote, None);
    assert_final_decision_projection(
        blocked_scenario,
        stores.migration.actual_cutover_decisions.last().unwrap(),
        Some(recovery_decision),
    );
}

fn root_safety_scenario<'a>(fixture: &'a Value, name: &str) -> &'a Value {
    fixture["rootSafetyScenarios"]
        .as_array()
        .unwrap()
        .iter()
        .find(|value| value["name"] == name)
        .unwrap()
}

fn root_binding_scenario<'a>(fixture: &'a Value, name: &str) -> &'a Value {
    fixture["rootBindingScenarios"]
        .as_array()
        .unwrap()
        .iter()
        .find(|value| value["name"] == name)
        .unwrap()
}

fn assert_root_safety_projection(
    scenario: &Value,
    store: &mut MigrationStore,
    put_count: usize,
    cutover_decision: &str,
) {
    let safety = store.load_root_safety("root://legacy").unwrap();
    let consistency = match safety.cutover_state.fingerprint_consistency {
        ActivationFingerprintConsistencyV1::NoEvidence => "NoEvidence",
        ActivationFingerprintConsistencyV1::Consistent { .. } => "Consistent",
        ActivationFingerprintConsistencyV1::Conflict => "Conflict",
    };
    assert_eq!(
        json!({
            "rootFatalCodes": safety.root_fatal_signals.iter().map(|value| &value.code).collect::<Vec<_>>(),
            "migrationStatus": store.current.as_ref().map(|value| serde_json::to_value(value.status).unwrap()),
            "remoteS2Activated": safety.cutover_state.remote_s2_activated,
            "evidencePaths": safety.cutover_state.verified_activation_evidence.iter().map(|value| &value.path).collect::<Vec<_>>(),
            "fingerprintConsistency": consistency,
            "cutoverDecision": cutover_decision,
            "putCount": put_count,
            "migrationGeneration": store.current.as_ref().map(|value| value.generation),
            "authorityGeneration": safety.generation,
        }),
        json!({
            "rootFatalCodes": scenario["expectedRootFatalCodes"],
            "migrationStatus": scenario["expectedMigrationStatus"],
            "remoteS2Activated": scenario["expectedRemoteS2Activated"],
            "evidencePaths": scenario["expectedEvidencePaths"],
            "fingerprintConsistency": scenario["expectedFingerprintConsistency"],
            "cutoverDecision": scenario["expectedCutoverDecision"],
            "putCount": scenario["expectedPutCount"],
            "migrationGeneration": scenario["expectedMigrationGeneration"],
            "authorityGeneration": scenario["expectedAuthorityGeneration"],
        })
    );
}

#[test]
fn root_safety_predates_claim_and_cutover_merge_retains_stronger_facts() {
    let fixture = fixture();
    let timestamp = fixture["identity"]["createdAt"].as_str().unwrap();
    let mut fatal_stores = FakeStores::default();
    fatal_stores
        .migration
        .persist_root_fatal("root://legacy", "SYNC_ROOT_FROZEN_LEGACY_CHANGE")
        .unwrap();
    let durable_pre_claim_safety = fatal_stores
        .migration
        .load_root_safety("root://legacy")
        .unwrap();
    fatal_stores = FakeStores {
        migration: MigrationStore {
            root_safety: Some(durable_pre_claim_safety),
            ..MigrationStore::default()
        },
        ..FakeStores::default()
    };
    let initial = create_migration_state_v1(
        fixture["identity"]["migrationId"].as_str().unwrap(),
        "root://legacy",
        fixture["identity"]["writerId"].as_str().unwrap(),
        timestamp,
        "legacy-bootstrap",
    )
    .unwrap();
    let attachment = start_or_attach_migration_v1(&initial, &mut fatal_stores.migration).unwrap();
    assert_eq!(attachment.state.status, MigrationStatusV1::RootFrozen);
    let mut remote = FakeRemote {
        root_id: "root://legacy".to_string(),
        ..FakeRemote::default()
    };
    let capability = create_migration_root_execution_capability_v1(
        &attachment,
        &remote,
        &fatal_stores.migration,
    )
    .unwrap();
    let proposed = planned_for(&fixture, &fixture["planningScenarios"][1]);
    let frozen = execute_migration_step_v1(
        &proposed,
        &capability,
        &mut remote,
        &mut fatal_stores.migration,
        &mut fatal_stores.intent,
        &mut fatal_stores.receipt,
        &mut fatal_stores.activation_intent,
        &mut fatal_stores.activation_receipt,
        timestamp,
    )
    .unwrap();
    assert_eq!(frozen.status, MigrationStatusV1::RootFrozen);
    assert_root_safety_projection(
        root_safety_scenario(&fixture, "fatal-before-first-claim"),
        &mut fatal_stores.migration,
        remote.put_counts.values().sum(),
        "root-fatal-denied",
    );

    let fingerprint_one = "f".repeat(64);
    let fingerprint_two = "e".repeat(64);
    let evidence_a = activation_evidence("activations/a.json", &fingerprint_one, 'a');
    let evidence_b_same = activation_evidence("activations/b.json", &fingerprint_one, 'b');
    let evidence_b_different = activation_evidence("activations/b.json", &fingerprint_two, 'b');
    for (name, first, later, restart) in [
        (
            "activated-state-then-stale-preactivation-state",
            cutover_input(vec![evidence_a.clone()]),
            cutover_input(vec![]),
            false,
        ),
        (
            "conflict-fatal-then-stale-safe-state",
            cutover_input(vec![evidence_a.clone(), evidence_b_different]),
            cutover_input(vec![evidence_a.clone()]),
            false,
        ),
        (
            "evidence-retention-subset-write",
            cutover_input(vec![evidence_a.clone(), evidence_b_same]),
            cutover_input(vec![evidence_a.clone()]),
            false,
        ),
        (
            "restart-after-stale-cutover-write",
            cutover_input(vec![evidence_a]),
            cutover_input(vec![]),
            true,
        ),
    ] {
        let mut store = MigrationStore::default();
        store
            .persist_cutover_state("root://legacy", &first)
            .unwrap();
        store
            .persist_cutover_state("root://legacy", &later)
            .unwrap();
        if restart {
            store = MigrationStore {
                root_safety: store.root_safety.clone(),
                ..MigrationStore::default()
            };
        }
        let safety = store.load_root_safety("root://legacy").unwrap();
        let recovery = recover_activation_cutover_v1(
            &create_discovery_state_v1(),
            Some(&safety.cutover_state),
        );
        let decision = match decide_legacy_put_v1(&recovery) {
            LegacyPutDecisionV1::DeniedRemoteS2Activated => "REMOTE_S2_ACTIVATED",
            LegacyPutDecisionV1::AllowedS2NotActivated => "S2_NOT_ACTIVATED",
            LegacyPutDecisionV1::DeniedCutoverRecoveryNotReady => "CUTOVER_RECOVERY_NOT_READY",
        };
        assert_root_safety_projection(
            root_safety_scenario(&fixture, name),
            &mut store,
            0,
            decision,
        );
    }
}

#[test]
fn cross_root_cutover_before_first_claim_preserves_root_authority() {
    let fixture = fixture();
    let scenario = root_binding_scenario(&fixture, "cross-root-cutover-before-first-claim");
    let timestamp = fixture["identity"]["createdAt"].as_str().unwrap();
    let mut store = MigrationStore::default();
    let before = store.load_root_safety("root://legacy").unwrap();
    let conflicting_cutover = cutover_input(vec![
        activation_evidence("activations/a.json", &"f".repeat(64), 'a'),
        activation_evidence("activations/b.json", &"e".repeat(64), 'b'),
    ]);

    let error = store
        .persist_cutover_state(
            scenario["requestedRootId"].as_str().unwrap(),
            &conflicting_cutover,
        )
        .unwrap_err();
    assert_eq!(error.0, scenario["expectedResult"].as_str().unwrap());
    let after_rejection = store.load_root_safety("root://legacy").unwrap();
    assert_eq!(after_rejection, before);
    assert!(store.current.is_none());
    assert!(store.history.is_empty());

    let initial = create_migration_state_v1(
        fixture["identity"]["migrationId"].as_str().unwrap(),
        "root://legacy",
        fixture["identity"]["writerId"].as_str().unwrap(),
        timestamp,
        "legacy-bootstrap",
    )
    .unwrap();
    let attachment = start_or_attach_migration_v1(&initial, &mut store).unwrap();
    let safety = store.load_root_safety("root://legacy").unwrap();
    assert_eq!(
        json!({
            "result": error.0,
            "rootSafetyRootId": safety.root_id,
            "authorityGeneration": safety.generation,
            "evidencePaths": safety.cutover_state.verified_activation_evidence.iter()
                .map(|value| &value.path).collect::<Vec<_>>(),
            "fatalCodes": safety.root_fatal_signals.iter()
                .map(|value| &value.code).collect::<Vec<_>>(),
            "remoteS2Activated": safety.cutover_state.remote_s2_activated,
            "migrationStatus": serde_json::to_value(attachment.state.status).unwrap(),
            "putCount": 0,
        }),
        json!({
            "result": scenario["expectedResult"],
            "rootSafetyRootId": scenario["expectedRootSafetyRootId"],
            "authorityGeneration": scenario["expectedAuthorityGeneration"],
            "evidencePaths": scenario["expectedEvidencePaths"],
            "fatalCodes": scenario["expectedFatalCodes"],
            "remoteS2Activated": scenario["expectedRemoteS2Activated"],
            "migrationStatus": scenario["expectedMigrationStatus"],
            "putCount": scenario["expectedPutCount"],
        })
    );

    let mut claimed_store = MigrationStore::default();
    start_or_attach_migration_v1(&initial, &mut claimed_store).unwrap();
    let claimed_safety_before = claimed_store.root_safety.clone();
    let claimed_migration_before = claimed_store.current.clone();
    let claimed_error = claimed_store
        .persist_cutover_state(
            scenario["requestedRootId"].as_str().unwrap(),
            &conflicting_cutover,
        )
        .unwrap_err();
    assert_eq!(
        claimed_error.0,
        scenario["expectedResult"].as_str().unwrap()
    );
    assert_eq!(claimed_store.root_safety, claimed_safety_before);
    assert_eq!(claimed_store.current, claimed_migration_before);
}

#[test]
fn activation_recovery_exact_present_then_absent_is_read_only() {
    let fixture = fixture();
    let scenario = final_race_scenario(&fixture, "activation-recovery-present-then-absent");
    let timestamp = fixture["identity"]["createdAt"].as_str().unwrap();
    let mut state = planned_for(&fixture, &fixture["planningScenarios"][2]);
    let mut setup_remote = FakeRemote::default();
    let mut stores = FakeStores::default();
    while state.status != MigrationStatusV1::ActivationPublishing {
        state = execute(&state, &mut setup_remote, &mut stores, timestamp);
    }
    let intent = state.activation_intent.as_ref().unwrap();
    let mut remote = FakeRemote {
        root_id: state.root_id.clone(),
        scripted_gets: VecDeque::from([
            RemoteExactGetResultV1::DefinitelyPresent(intent.exact_bytes.clone()),
            RemoteExactGetResultV1::DefinitelyAbsent,
        ]),
        ..FakeRemote::default()
    };
    let result =
        restart_durable_activation_publish_v1(intent, None, &mut remote, timestamp).unwrap();
    let recovery_decision = match result {
        RecoverActivationIntentResultV1::AlreadyPublishedExact(_) => "AlreadyPublishedExact",
        _ => panic!("unexpected activation recovery decision"),
    };
    install_cutover_fatal(&mut stores);
    assert_final_decision_projection(
        scenario,
        stores.migration.actual_cutover_decisions.last().unwrap(),
        Some(recovery_decision),
    );
    assert_eq!(remote.scripted_gets.len(), 1);
    assert_final_race_projection(
        scenario,
        &state,
        &remote,
        Some(
            stores
                .migration
                .current
                .as_ref()
                .unwrap()
                .root_fatal_signals
                .iter()
                .map(|fatal| fatal.code.clone())
                .collect(),
        ),
    );
}
