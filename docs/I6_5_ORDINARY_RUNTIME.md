# I6.5 Android ordinary S2 runtime

## Review baseline

- Android baseline: `9cb877e49b6aca22cbf6a0686cacfed97e0a734f`.
- Branch: `feat/s2-lite-v1-parity` (unchanged).
- Frozen desktop protocol: `0c51434e6249a391d6fc1621fe6751e5eba48f27`.
- No dependency changes, frozen fixture changes, SQLite table changes, or schema-version changes.
- No provider testing or push. This checkpoint is for independent review.

## Execution and durable authority

The new `s2_sync_cycle` Tauri command obtains protected credentials for the exact
active target/epoch and executes on the blocking Rust runtime. It uses the existing
I6.2 WebDAV adapter and the same managed physical-root coordinator as I6.4.

One bounded ordinary cycle:

1. Resolve current target/root; discover and replay retained remote history.
2. Refresh durable root safety. Fatal knowledge returns frozen immediately.
3. Resume an already admitted explicit migration, if unfinished; never implicitly
   start migration. Before activation return `legacyS1Required`.
4. Admit existing remote activation where necessary, install the local writer
   only if missing, and verify local writer ownership.
5. Require complete discovery/projection, no unresolved entity/relation conflict,
   and apply verified projection without overwriting locally dirty entities.
6. Recover the existing unfinished batch; otherwise freeze captured descriptors.
7. Persist exact intent before publication. At the actual transport boundary,
   hold `BEGIN IMMEDIATE` and validate current root, target/epoch, activation,
   source-owner retirement, writer sequence/head, retained fatal knowledge,
   writer ownership, discovery readiness and projection/conflict blockers.
8. Publish/recover the exact immutable object with frozen conditional semantics.
   Any observed verification mismatch persists fatal immediately. Use frozen
   receipt verification and persist the receipt before local retirement.
9. Atomically complete the batch, advance head, and retire only matching capture
   identities/generations. Discover again, refresh projection, then classify the
   final result from current durable authority and outstanding local captures.

Only `success` becomes UI `ok: true`. Pending, conflicts, frozen, changed target,
auth/capability failure and skipped automatic events do not become success.
The final scheduler transaction rechecks authority and outstanding mutations;
a late success cannot erase newer local work or a newer fatal.

### Mutation freezing

`outbound_freeze.rs` ports the production portion of the frozen desktop ordinary
freezer. JCS, scalar mapping, mutation sorting, commit construction, clock,
previous linkage, sequence and immutable path preparation retain frozen functions.
Android payloads come from I6.1 durable typed descriptors, including complete
four-type tombstones and episode-completion composite identities. Stable capture
mutation IDs are reused instead of generating new IDs while freezing.

The first captured basis/frontier/root must match verified projection. Unknown or
stale bases remain blocked; they are never inferred as absence or retroactively
rebased. Already captured `Unavailable` evidence remains `Unavailable`, including
captures from before verified projection existed. A later edit after a prepared
batch stays staged if the old receipt does not cover its generation. A subsequent
stale first basis stays pending rather than being silently replaced.

Writer reservation, batch metadata and exact intent bytes/hash/path are one
transaction. Retry always recovers that batch and intent. Attempting PUT does not
retire anything. A proven semantic no-op can retire in the freezer transaction
without allocating a sequence or doing network I/O. Mixed batches leave remaining
no-ops for a later bounded cycle. Source-row deletion or clearing S1 staging does
not remove durable S2 delete evidence.

### Ownership and fatal persistence

All verified remote references for the locally owned writer are checked against
strictly decoded durable commit intents, including historical intents. A different
identity at an owned sequence, or an unexplained future sequence, persistently
freezes the root. This also covers process death after local completion but before
remote observation of the original commit. Local intents and remote forensic
alternatives remain available after restart.

## Lifecycle and scheduler

The existing mobile coordinator signals the Rust command for startup,
foreground/focus/visibility and Kotlin Android resume, online restoration, local
write debounce, retry timers, and manual sync. Kotlin lifecycle code is unchanged;
it already emits `watchtracker:android-resume`. TS owns event/timer delivery only.

Frontend coalescing reuses one promise. Rust serializes all callers through the
existing physical-root coordinator and rechecks automatic admission after waiting.
Automatic calls carry the observed last-attempt token; stale tokens, pause, changed
target and not-yet-due durable backoff are rejected. Manual calls can request an
attempt but cannot bypass root/activation/fatal/publication checks. Attempt stamps
increase even for two results in the same clock millisecond.

Rust stores failure count, safe result code, attempt/check times and retry deadline
using the existing target-scoped scheduler. Pending/conflict retry delays use
10/30/120/300/900 seconds. Success alone writes last-success and clears backoff.
Resume after pause can retry pending state without manufacturing protocol authority.
TS reloads Rust bookkeeping and does not make a second S2 failure write. Restart
reloads scheduler/outbox and resumes on startup; no assumption is made that timers
continue running after Android kills the process.

Before activation, an explicit Rust `legacyS1Required` result routes to the existing
S1 service. Other S2 statuses and command errors never fall through to S1. I6.4's
final S1 PUT gate remains intact. Legacy acknowledgement gates remain intact, and
a late S1 failure callback is now checked in the same transaction as its scheduler
write, so it cannot overwrite S2 completion after cutoff.

## Files

- Added: `src-tauri/src/s2_lite/ordinary_runtime.rs`, `outbound_freeze.rs`,
  `outbound_completion.rs`, `tests/mobile-sync-i65.spec.ts`, this document.
- Changed Rust: `commands.rs`, `lib.rs`, `s2_lite/mod.rs`,
  `s2_lite/durable_persistence.rs`, `s2_lite/discovery_persistence.rs`,
  `s2_lite/migration_runtime_tests.rs`, `sync_state.rs`.
- Changed frontend: `shared/lib/webdav.ts`,
  `features/sync/hooks/useSyncCoordinator.ts`,
  `features/sync/services/syncContracts.ts`, `tests/fixtures/mockIpc.ts`.
- Reused storage: writer root, outbound batch, prepared intent, published receipt,
  local descriptor, read/projection/root authority, target-scoped scheduler/outbox.

## Focused verification

- I6.5 production Rust tests: **24/24**.
- I6.4 production migration/cutover regressions: **40/40**; combined runtime **64/64**.
- Relevant S2 regressions: **143/143** (ordinary mapping 3, immutable publication 10,
  discovery 16, causal 21, discovery persistence 20, local capture 13, local writer
  authority 7, adapter 22, migration orchestration 23, activation/cutover 7,
  fixture drift 1).
- S1/storage regressions: **94/94** (scheduler/state 21, local import 30,
  atomic DB 23, collections 12, DB 8).
- Mobile Playwright with mock IPC/local Vite: **15/15** (I6.5 5 plus M1.4 10).
  Mock IPC checks lifecycle/status routing, not protocol correctness; Rust fake
  WebDAV tests cover protocol execution.
- Targeted frontend unit tests: **7/7**; TypeScript check and changed-file ESLint pass.

Production cases include coalescing, independent episode progress 2 to 5,
all four tombstones after rows/S1 staging disappear, atomic prepare failure,
restart before preparation/after intent, lost PUT response, failed verification,
verified-before-receipt and receipt-before-retirement crash boundaries,
exact retry identity, online/foreground overlap, stale retry after manual success,
fatal/manual gating, pre-publish fork, historical ownership collision,
epoch replacement, generation-safe retirement, durable backoff and final success
bookkeeping against newer local/fatal facts.

Required final gates: `cargo fmt -- --check`, `cargo check --locked`,
`cargo clippy --all-targets --all-features --locked -- -D warnings`,
`git diff --check`: **all PASS**.

## Deferred final validation

Only final real-provider/Desktop-to-Android interoperability and release-build
validation remain outside this checkpoint, as requested. No emulator, real
Jianguoyun, final interoperability, release matrix, or full repository gate ran.
Conflict resolution UI is not introduced; frozen conflicts and stale/unknown
causal bases retain durable blocked states.
