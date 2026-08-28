# Android architecture

This project is a mobile Tauri shell around the WatchTracker-Main domain
baseline. The record contract, SQLite V18 schema, atomic CRUD, episode history,
recovery points, WebDAV payload parsing/merge, and poster validation remain
shared Rust/TypeScript code. Mobile UI and Android-only capabilities are kept
at explicit adapter boundaries.

## Runtime and UI boundaries

`src/platform/runtime.ts` is the single runtime decision point. Android is
selected only when a Tauri runtime and Android user agent are both present.
Desktop Tauri and browser/Playwright use the compatibility `App` shell. The
Android path enters `MobileApp`, whose M1 shell uses a replaceable tab entry,
real detail/form history entries, safe-area-aware layout and the existing
Rust-backed local record repository. The library is the mobile root: native
Back returns an explicit Activity-exit action there, without traversing stale
browser history. Filter/dirty-form overlays consume Back before route history.
The form explicitly disables TMDB and collection capabilities on Android until
their platform adapters are complete; desktop defaults remain unchanged.

The shell exposes library, discovery, collections, statistics, and settings
entry points. Discovery, collections, and statistics remain deliberate no-op
development placeholders; settings now contains the M1.4 mobile sync surface.
M1.2 local add/edit/delete, filters and mobile preferences are real. Their
mobile stale check is a reload-and-compare guard around unchanged CRUD and is
not an atomic CAS.

M1.3 episode writes have a stronger boundary. Only non-film records with a
positive integer `totalEpisodes` expose mobile episode controls. The mobile
repository invokes the existing Rust `enable_episode_tracking` and
`set_next_episode` commands with the displayed `expectedRev`; it never derives
episode history from legacy `progress` and never updates episode state
optimistically. Success state comes from the Rust command's persisted
`EpisodeTracking` result. Stale, missing and locked failures reload the Rust
source of truth and keep a safe, visible Chinese failure message. List cards
offer the current-episode completion shortcut; detail owns enable, jump,
retreat, atomic finish, history and resume-after-total-growth controls. Locked
episode surfaces are read-only. This adds no schema, migration or Android
native bridge change; existing cold-start, route and Back contracts remain in
force.

M1.4 connects the mobile repository to the existing `useSyncCoordinator`
through the same ref bridge used by the desktop watch list. Every successful
mobile record or episode transaction queues the existing target-scoped outbox;
the coordinator remains single-flight and owns startup, debounced local-write,
online, foreground, retry and manual triggers. `MobileApp` marks the local
SQLite library ready before starting the coordinator, so network, credential,
schema and conflict failures cannot hold the local loading screen. Android
`MainActivity.onResume` emits `watchtracker:android-resume`; no WorkManager or
background execution contract is introduced in this milestone.

The mobile settings surface uses the existing read-only probe followed by an
explicit activation confirmation. Normal sync receives only target ID, target
epoch and credential state. It never retrieves the saved password. Status,
target-scoped outbox and persistent conflicts come from the existing Rust
runtime; conflict choices call the shared resolution command and queue the
result through the same coordinator.

## Platform boundaries

- `src-tauri/src/app_paths.rs` resolves only `AppHandle.path().app_data_dir()`
  on Android. The desktop portable `data/` convention is not inspected on
  mobile; database, logs, posters, and backups therefore stay in the app
  sandbox.
- `src-tauri/src/secret_store.rs` keeps the Windows Credential Manager adapter
  unchanged for desktop and continues to recognize `wincred:v1`. Android uses
  the explicit `androidkeystore:v1` reference. Foreign-platform or invalidated
  references fail closed as missing/re-entry-required and never fall back to
  plaintext.
- `AndroidSecretStore.kt` stores a non-exportable per-logical-target AES key in
  AndroidKeyStore. AES-GCM uses a random IV and binds the logical target as AAD;
  only versioned IV/ciphertext metadata is written through `AtomicFile` in the
  app-private sandbox. Known invalidated/unrecoverable aliases may be rebuilt
  only after the user explicitly re-enters a secret. General I/O/service
  failures do not destroy an existing key. R8 keep rules preserve the JNI
  entry points.
- Rust calls the adapter on Wry's registered Activity/JNI thread through
  `wry::prelude::dispatch`, with a five-second bounded result channel. JNI
  exceptions are cleared on errors and exposed only as safe credential state.
  Production instrumentation verifies write/read/delete, no plaintext at rest,
  and tamper/AAD failure. React sees availability/state only.
- SAF is represented by a narrow `ACTION_OPEN_DOCUMENT` JSON intent in the same
  instrumentation proof. URI permission and streaming import/export will be
  added behind the platform adapter before M2.
- The `poster://` protocol is registered in `src-tauri/src/lib.rs` and accepts
  one safe filename only; canonical poster bytes still pass the shared image
  signature and cache-size checks.
- Product WebDAV sync uses the shared Rust network command and existing
  TypeScript reliability core. Credentials are resolved at the Rust boundary
  and are never returned to the WebView. Android writes the same
  `records-v3.json` payload as desktop and preserves V3～V6 parsing, conditional
  validators, 412 retry, merge, staging, publish intent and target isolation.

## Data invariants

Every local insert/update/delete uses the existing explicit SQLite transaction
and updates record/tombstone, generation, and outbox state together. Locked
records are preserved during replacement. Unknown database versions and future
payload schemas fail closed. Android must not introduce a schema fork.

## Generated Android project

`src-tauri/gen/android` is generated by Tauri 2 and intentionally checked in
for reproducible Gradle builds. The debug application ID is
`com.watchtracker.android.debug`, minSdk is 26, and the first supported ABI is
`arm64-v8a`. Do not hand-edit generated Rust or Tauri sources; regenerate after
changing the bundle identifier.
