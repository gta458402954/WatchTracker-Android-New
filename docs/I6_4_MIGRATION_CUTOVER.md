# Integration 6.4: migration, activation and permanent S1 cutoff

Baseline: `0da02aa00493612f946f5a1b34959842eae9921f`.
Frozen desktop: `WatchTracker-Rust` at `0c51434e6249a391d6fc1621fe6751e5eba48f27`.
Branch: `feat/s2-lite-v1-parity`. This checkpoint is local and awaits independent review.

## Production entry points

`commands::s2_migration_step` is an explicit bounded invocation. It resolves stored
credentials and the exact active target/epoch, acquires the frozen process root
coordinator, and runs one frozen migration step on a blocking worker through the
approved I6.2 adapter. It does not install a timer, UI action, lifecycle callback,
or ordinary S2 writer route.

All existing `commands::webdav_request` PUT requests pass through
`migration_runtime::run_legacy_put_with_adapter_v1`. GET, PROPFIND, MKCOL and the
read-only credential probe retain their existing transport entry points. The
legacy payload, conditional headers and response remain the existing S1 values.
The added discovery reads and durable admission decide whether a legacy PUT may
execute. `probe_webdav_request` remains restricted to GET/PROPFIND.

## Durable schema

The business DB stays at version 18. Frozen S2 persistence has its separate
`s2_lite_persistence_schema_version = 1` marker, strict versioned JSON BLOB
envelopes, canonical decimal generation columns, and separately stored exact
prepared bytes. The new tables are:

| Table | Authority |
| --- | --- |
| `s2_lite_root_authority_v1` | Permanent activation latch, fatal facts, authority generation |
| `s2_lite_migration_v1` | Snapshot, deterministic plan, immutable identities, stages and receipts |
| `s2_lite_migration_discovery_v1` | Frozen migration view of retained I6.3 discovery |
| `s2_lite_prepared_intent_v1` | Root/path-bound metadata and exact immutable bytes |
| `s2_lite_published_receipt_v1` | Verified root-bound commit/activation receipts |
| `s2_lite_desktop_root_state_v1` | Frozen root writer continuity and projection application generations |
| `s2_lite_outbound_batch_v1` | Dormant shared frozen storage; ordinary executor is not connected |
| `s2_lite_materialized_projection_v1` | Canonical projection cache and separate business application marker |
| `s2_lite_target_root_binding_v1` | Historical target/epoch to physical root binding |
| `s2_lite_migration_execution_binding_v1` | Frozen source generation, migration ID and legacy fingerprint |
| `s2_lite_migration_source_guard_v1` | Immutable source ownership during migration |
| `s2_lite_migration_source_owner_v1` | Database-wide exclusive source owner |
| `s2_lite_entity_projection_overlay_blocker_v1` | Local overlays preserved during business application |
| `s2_lite_remote_activation_adoption_v1` | Original nullable compatibility basis for an already activated root |

The migration discovery table is renamed from desktop to avoid colliding with
approved I6.3 storage. I6.3 tables and their forensic replay remain authoritative.
The bridge commits retained discovery, activation/fatal knowledge and the
projection cache together. Omission cannot erase retained observations or reset
cutover/fatal facts.

The Android adoption record stores the original frozen captured snapshot,
records generation, exact nullable fingerprint and whether an S1 baseline
existed. A source with no baseline and no entities supplies null; otherwise the
frozen snapshot fingerprint is non-null. Compatibility is checked under one
SQLite write transaction, and mismatch commits a root fatal before returning.
Later business projection or local edits never regenerate this original basis.

## Migration stages and transactions

Production admission atomically captures the four frozen entity classes, plans
bootstrap chunks, persists the execution binding and installs source guard/owner.
Snapshot capture and planning have no externally observable intermediate gap.
Business INSERT/UPDATE/DELETE triggers protect all four classes until finalization.

The frozen stages are NotStarted, LegacySnapshotCaptured, BootstrapPlanned,
StageAPublishing, StageAComplete, StageBPublishing, StageBComplete,
ActivationPublishing, ActivationVerified, MigrationComplete and RootFrozen.
Vacant stages use the frozen reconciler's canonical recovery form.

Each published object follows durable preparation, exact preflight, conditional
PUT, exact verification and durable receipt. Retry retains commit/activation IDs,
path, content hash and exact bytes. A receipt committed before the later state CAS
is reused. Root write admission remains held through the admitted network operation.
Activation admission revalidates snapshot, execution identity, target/root/epoch,
source owner/guard, prepared bytes and exact nullable fingerprint.

Verified activation receipt recovery establishes the durable latch independently
of remote listing availability. Finalization atomically commits MigrationComplete,
installs the frozen migration writer head/next sequence and retires source ownership.
Local writer identity comes from the existing Android authority. A third-party
activation can establish cutoff without a fictitious local publication receipt or
migration ID. It remains distinct from completed local migration.

## Permanent S1 cutoff

The final target-bound legacy admission is
`SqliteS2LiteStoreV1::run_legacy_bound_put_v1`. Its BEGIN IMMEDIATE transaction
rechecks target/epoch/root, ticket generation, source ownership, fatal state,
activation evidence and the permanent remoteS2Activated latch before invoking the
old S1 network operation. A listed activation still awaiting exact validation is
ambiguous and blocks legacy mutation. Fatal facts and legal activation also block
PUT when no local migration exists.

`sync_state::commit` and `record_remote_unchanged` additionally call
`admit_legacy_business_ack_v1` inside their final business transactions. A stale S1
response cannot overwrite projected rows, acknowledge old staging, or restore S1
state after cutoff. These checks do not depend on UI routing. A new physical root
receives separate authority; switching target epoch cannot make an old ticket valid.

## Projection and local capture

Canonical replay/conflicts remain separate from business tables. A complete,
current, nonfatal projection is applied under one root-authoritative transaction.
Conflicts and relation blockers preserve affected rows; all four Android local
capture classes are overlays. Parent tombstones cannot cascade through protected
local children. SQL dependency order creates parents before children and deletes
children before parents. Failure rolls back business changes and the applied marker.

The reducer intentionally strips row metadata. The SQL boundary obtains metadata
only from retained verified replay, choosing the last variant in the frozen sorted
commit-ref order; it never reconstructs identity/revision from mutable local rows.
All metadata variants and frontier evidence remain retained. Frozen native scalar
validation precedes conversion of decimal int64 wire strings to SQLite model integers.
Protocol business values and semantic hashes are unchanged.

New local capture obtains Live or Absent only from an admitted, applied projection,
and persists physical root, basisClock and baseFrontier with its descriptor in the
business mutation transaction. Pending, stale, conflicted, overlay-blocked or missing
authority supplies Unavailable. Existing mutation ID, first generation, basis and
provenance remain immutable on subsequent updates/deletes. Capture failure rolls
back business, S1 staging, S2 descriptor and generation together.

## Necessary integration adaptations

- Frozen strict SQLite storage rejects a transient StageAComplete when empty
  Stage B canonicalizes it forward. A storage transition hook uses the unchanged
  frozen reconciler for that store. The hook defaults to identity, preserving
  every original pure execution fixture and in-memory stage assertion.
- Collections schema initialization formerly rewrote rows on every restart.
  The data upgrade now runs only when schema readiness requires it; startup
  cannot modify a captured snapshot or hit its protection triggers.
- The obsolete I6.2 source assertion forbidding commands from referring to the
  adapter is replaced with a runtime test that unconditional PUT is refused
  before any network operation. DAV parser and production adapter code are unchanged.
- The frozen shared persistence includes dormant ordinary outbound storage helpers,
  but no ordinary writer executor/coordinator, lifecycle scheduler or UI is imported.

## Verification

I6.4 production tests exercise clean and empty migrations, all four entity classes,
restart after planning and executor boundaries, intent-before-PUT recovery,
PUT-before-receipt and receipt-before-CAS failures for bootstrap and activation,
lost responses, omitted activation listings, latch-before-finalization restart,
finalization faults at all three transaction points, immutable mismatch/fork/fatal
state, corruption, nullable compatibility, target/root replacement, stale tickets,
conflicts, real CRUD Live/Absent capture, immutable earlier basis and capture/projector
rollback. Fake WebDAV only; no provider or emulator execution.

Relevant regressions: migration orchestration, activation/cutover, immutable
publication, discovery/causal and durable read authority, local authority/capture,
ordinary mapping, fixture manifest, WebDAV, S1 sync state, import, atomic CRUD,
collections and DB schema. All ten frozen fixture bytes are unchanged.

| Focused group | Passing tests |
| --- | ---: |
| I6.4 production boundaries | 30 |
| Frozen migration orchestration | 23 |
| Activation/cutover | 7 |
| Immutable publication | 10 |
| Remote discovery | 16 |
| Causal replay | 21 |
| Durable discovery/read projection | 20 |
| Local production capture | 13 |
| Local authority | 7 |
| Ordinary mutation mapping | 3 |
| Frozen fixture manifest | 1 |
| WebDAV adapter/immutable bridge | 22 |
| S1 sync state | 21 |
| S1 local import | 30 |
| Atomic CRUD | 23 |
| Collections | 12 |
| DB/schema | 8 |

Totals: 30 I6.4 tests, 143 relevant S2 regressions, 94 S1/storage regressions.
All pass. The zero-test `sync_staging::tests` filter is excluded from these totals;
staging behavior is exercised by the sync state, atomic CRUD and local capture groups.

Static gates pass: `cargo fmt -- --check`, `cargo check --locked`,
`cargo clippy --all-targets --all-features --locked -- -D warnings`, and
`git diff --check` (including the staged candidate). Cargo dependency/lock files
and all frozen contracts are unchanged. No full repository suite was executed.

## Deferred to I6.5

Mobile scheduling, startup/foreground/background automatic sync integration,
UI activation/conflict resolution, normal production S2 writer execution/publication,
and real-provider validation. This checkpoint stops for independent review.
