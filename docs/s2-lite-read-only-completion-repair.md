# Read-only mobile sync completion authority repair

Baseline Android: `31058983abd14c1533edd53fdb6b731c994f5d50`.
Desktop remains unchanged at `ed12d4f551f26c622b7037394e9146ea8f6fa17a`.

## Reproduction and repair boundary

Before implementation, a production SQLite/fake WebDAV regression through
`run_mobile_sync_with_adapter_v1` reproduced `S2_SCHEDULER_FAILURE`: adoption,
replay and business application succeeded without a writer, then final scheduler
authority rejected the missing writer. After repair the complete mobile wrapper
returns Success, records its timestamps, and creates neither writer nor intent.

`sync_state::record_mobile_s2_result_v1` now calls a scheduler-only validator.
Publication callers continue using the original writer-required validator. Both
share existing target/root, migration, fatal and writer validation. An existing
malformed writer is never treated as absent. No schema or protocol changes.

## Exact read-only Success predicate

Within the final scheduler transaction, a writer-less root requires all of:

- Active target/epoch, immutable binding, and current configuration derive the
  same physical root; activation is latched and root has no fatal/frozen facts.
- No local migration state/owner. No migration execution binding or source guard
  remains for the root.
- Existing adoption row strictly decodes, validates its snapshot/provenance and
  root, and matches the cutover's exact nullable compatibility fingerprint.
  Bookkeeping rejects a missing basis on a previously adopted reader rather than
  recreating one. A malformed global local-writer identity also fails closed.
- No root-bound prepared intent (commit or activation), receipt, outbound batch,
  or current-target entity overlay blocker exists.
- No staged S2 descriptors, captured pure mutations, S1 staging entries or S1
  publish intent exists; the active outbox is not pending.
- Authoritative discovery is ready; retained projection is Complete and matches
  current read/discovery/root generations; its business-applied generation equals
  its projection generation; there are no entity conflicts or blocked relations.

Pending work or an unapplied/stale projection cannot yield read-only Success.
Existing final classification retains Pending/backoff, Conflicts, fatal/frozen,
and target-change behavior. Publication still rejects an absent writer even when
the mobile read-only completion predicate could otherwise be satisfied.

## Atomicity and lifecycle evidence

Final admission, read classification, pending-work checks, scheduler writes and
outbox acknowledgement remain inside `record_mobile_s2_result_v1`'s single
`BEGIN IMMEDIATE` transaction. The production mobile caller holds the shared
connection mutex through that call. Production local CRUD uses the same mutex;
SQLite's write transaction also serializes other connections' writes.

A concurrent production collection-update regression blocks local capture until
completion commits. The later write retains its descriptor and dirty outbox,
and the next attempted clean completion is Pending. A scheduler-write trigger
fault rolls back bookkeeping; reopening retains the prior scheduler/outbox and
adoption basis, and retry succeeds without creating a writer.

## Focused regressions

Eight new grouped tests cover the complete mobile wrapper, persistent success
timestamps, clearing Pending backoff, repeat read-only cycles across restart,
first local mutation/publication after adoption, pending descriptor and outbox
negatives, orphan intent/batch/unretired-receipt negatives, unapplied/stale
projection, fatal root, malformed root/global writer, corrupt/deleted basis,
manual/automatic/retry-style admissions with permanent S1 rejection, concurrent
capture serialization, and scheduler fault/restart rollback.

The first local write allocates the stable writer through normal capture/
publication, retains its mutation ID, publishes sequence 1 exactly once, and
keeps the same writer across restart. Read-only completion never allocates one.

| Focused group | Passed |
| --- | ---: |
| Migration/runtime, including previous adoption and I6.5 lifecycle tests | 91 |
| Sync state/scheduler and S1 acknowledgement regressions | 21 |
| Durable discovery/projection regressions | 20 |
| Total unique focused tests | 132 |

Gates passed: `cargo fmt -- --check`, `cargo check --locked`,
`cargo clippy --all-targets --all-features --locked -- -D warnings`, and
`git diff --check`.

Only Android completion authority, its scheduler call site, focused tests and
this report changed. No Desktop, frozen contracts, dependency, transport,
publication wire, or lifecycle implementation changes. No Jianguoyun/final matrix
run and no push. Candidate stops for Astra focused review.
