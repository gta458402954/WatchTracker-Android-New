# I6.3 Android read authority

Baseline: `c35ecf415be7eae0eff34de708a59957ab9d9975`.
Frozen desktop semantics: `0c51434e6249a391d6fc1621fe6751e5eba48f27`.

## Execution boundary

`discovery_runtime::discover_read_only_v1` is an explicit Rust API using the
approved `WebDavS2AdapterV1`. It calls only directory PROPFIND and immutable GET.
It is not registered with S1, IPC, lifecycle, or UI. It does not create remote
collections, publish commits/activation, perform cutover, or change S1 eligibility.

A cached transcript bridges async I/O to the unchanged synchronous discovery
core. Each planning pass starts from the same durable prior state. The first
unknown call is resolved before later calls; provisional indeterminate results
are never saved. The resulting scheduling state and fetch budget match the
frozen round. The connection holds no transaction during network I/O.

## Durable schema

Schema version is `settings.s2_lite_read_authority_schema_version = 1`.
Initialization and startup validation are transactional. Missing version metadata
with existing tables, missing tables with existing metadata, unknown versions,
partial rows and corrupt state fail closed; startup never resets their authority.

| Table | Persisted evidence |
| --- | --- |
| `s2_lite_read_root_v1` | Physical root identity and monotonic fatal-code latch |
| `s2_lite_discovery_v1` | Canonical decimal storage generation; complete frozen `DiscoveryStateV1` envelope |
| `s2_lite_read_projection_v1` | Matching source generation; frozen materialized projection and full causal replay result |

Discovery includes retained observations, all verified exact bytes and identities,
active/recent observations, historical closed segments/audit cursor, gap cursor,
known gaps, pending dependency targets, fair exact-work scheduler, reverification
queue, terminal paths, last-round progress and full fatal signals. No omission,
404, or transport failure removes previous knowledge.

A final `BEGIN IMMEDIATE` transaction compares the generation loaded before I/O,
checks evidence retention, rebuilds projection/replay and atomically writes all
three rows. Stale rounds fail without changing storage. Any write or commit error
rolls back discovery, projection, conflict results and root fatal facts together.

Envelopes are strictly decoded with the already-isolated S2 roundtrip parser:
unknown nested fields, silently defaulted missing fields, malformed identifiers,
hex bytes, cursor/scheduler shapes and invalid metadata are rejected. Exact
objects are reverified through frozen validators. Both stored projection and full
replay must equal a rebuild from retained bytes. S1 parser/features are unchanged.

## Projection and conflicts

`materialized_projection.rs` is copied from the frozen desktop source without
semantic changes. Its cache records status, entity semantic/business values,
entity frontiers, basis clock, relation blockers and replay-input fingerprint.
The accompanying full replay retains validity/pending classifications, forks,
unsafe references, forensic alternatives, version provenance, entity conflict
IDs/fields/frontiers/alternatives, relation conflicts and duplicate diagnostics.

`resolve_verified_anchor_v1` is the durable authority query. It rejects fatal,
pending or conflict-blocked authority. Unknown roots/entities return Unavailable;
an empty or omitted listing never proves absence. Explicit replayed absent state
can provide Absent; a verified deletion preserves the frozen Tombstone causal
base and frontier rather than being collapsed into an unknown entity. Verified
live values preserve their frontier and basis clock. Reads never update local
staging, mutation identity or the original first-edit basis.

Same writer/sequence alternatives remain exact separate objects. Frozen
`WRITER_FORK` facts and replay-fatal state are persisted together. Root fatal
codes only accumulate, including late historical forks and protocol corruption.
Restart and additional discovery cannot clear the latch. Corrupt durable storage
returns an error before remote execution and is never replaced with empty state.

## Verification and deferred work

The focused production-boundary tests exercise disk restart, omission, historical
audit, pending/late dependencies, forks, live/tombstone/unobserved authority,
unchanged local capture, conflicts, deterministic ordering, corrupt storage,
remote fatal facts, schema loss, CAS/rollback and frozen async scheduling parity.
Existing discovery/causal, local authority/capture, adapter and S1 import tests
are also run. Required fmt/check/clippy/diff checks are recorded in the review
handoff.

Later phases must handle production writer admission/publication, verified-anchor
capture integration, business-table projection application, migration/cutover,
activation/S1 gating, lifecycle/UI and conflict resolution. No real provider or
emulator is used in I6.3.
