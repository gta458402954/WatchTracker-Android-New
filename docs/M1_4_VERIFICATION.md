# M1.4 verification record

M1.4 advances the user path “update viewing progress on Android and reliably
sync it to desktop.” It reuses the existing desktop reliability core and does
not introduce an Android payload, remote file, merge algorithm or downgraded
write path.

## Scope and security invariants

- Mobile CRUD and episode writes notify the shared `useSyncCoordinator` after
  the authoritative Rust transaction succeeds. Record state, tombstones and
  `episodeCompletions` remain in the existing target-scoped outbox flow.
- Android local SQLite becomes usable before startup sync begins. Missing
  credentials, network errors, future schema, 412 and conflicts cannot leave
  the app on the local-library loading state.
- New targets require a read-only Probe and explicit confirmation. Normal sync
  receives target identity and credential state only; the persisted password
  is never returned to the WebView.
- Android stores per-target AES keys as non-exportable AndroidKeyStore entries.
  AES-GCM uses a random IV and target-bound AAD; `AtomicFile` persists only
  versioned ciphertext metadata in app-private storage. Tamper, key loss and
  invalidation fail closed and leave the business database intact.
- Windows `wincred:v1` remains compatible. Android uses
  `androidkeystore:v1`; a foreign platform reference requests re-entry rather
  than being interpreted as legacy plaintext.
- `records-v3.json`, payload V3～V6, conditional validators, 412 retry,
  three-way merge, tombstones, expected generation, staging, publish intent,
  baseline, target ID/epoch and persistent conflicts are unchanged.

## Automated coverage

- Rust tests cover platform-reference classification, Windows compatibility,
  missing/re-entry behavior, migration-journal redaction, logical-secret
  deletion isolation, ordinary activation rollback, and full legacy/re-entry
  replacement rollback when the secure write fails.
- Android instrumentation covers the production adapter write/read/delete
  contract, absence of plaintext at rest, and ciphertext/AAD tamper returning
  re-entry state.
- Node tests cover blocked credential scheduling states and sanitized mobile
  WebDAV display.
- `tests/mobile-sync-m14.spec.ts` covers local-first startup, real settings,
  Probe-before-activate, refreshed target/epoch context, persisted first-sync
  failure, manual sync, password clearing/redaction, status/edit/delete and
  local episode → outbox → remote completion → acknowledgement, failure
  retention, pause/resume, persistent conflict resolution and re-entry cleanup
  without blocking local use.
- `android:m14-smoke` installs the just-built APK, proves installed/local SHA
  equality, and drives the real Tauri/Rust/Keystore path against a bounded
  local WebDAV server with exact decoded Basic credentials, MKCOL, GET, PUT,
  PROPFIND, ETag, If-Match, If-None-Match and forced 412 behavior. One local
  episode write waits for the real debounce, and process restart waits for the
  real startup trigger without injected browser events.

## Repeatable final gate

Run from `WatchTracker-Android-New`:

```powershell
git diff --check
npm run check:m0
npm run test:e2e
npm run android:build
npm run android:test
# android:test uninstalls its test target; reinstall the immediately preceding
# universal APK and verify its SHA before the device smokes.
npm run android:smoke
npm run android:m12-smoke
npm run android:m13-smoke
npm run android:m14-smoke
```

## 2026-08-28 emulator evidence

- Source baseline: `f3d82ad` on `feat/android-m14-sync-mvp`; the final commit
  SHA is recorded by the Git commit/CI evidence after this mandatory pre-commit
  gate. No other repository is part of this working tree.
- Device: `Medium_Phone` Android Emulator, `sdk_gphone64_x86_64`, Android 16,
  adb serial `emulator-5554`.
- Final accepted executions: every command in the repeatable gate above
  returned `0`. `check:m0` passed 183 Node tests and 96 Rust tests; Playwright
  passed 129/129 tests with its normal summary; Android instrumentation passed 3/3;
  the M0, M1.2, M1.3 and M1.4 device smokes all passed. The controlled M1.4
  server observed 3 conditional PUTs, 0 unconditional PUTs and one exercised
  412 precondition retry.
- APK path:
  `src-tauri/gen/android/app/build/outputs/apk/universal/debug/app-universal-debug.apk`.
  Final local/installed SHA-256:
  `A42594B3EB1A18F704A1663E6148A43E4A9EB9EA076F5ED1141E2EC6157E5ECF`.
- Controlled M1.4 smoke cases: local-first cold start; Keystore activation;
  seeded remote pull; episode record/history publish and outbox acknowledgement;
  local-write debounce publish; force-stop credential reuse plus authenticated
  startup pull; foreground remote-field merge; one forced 412 followed by
  conditional retry; offline local completion with retained outbox; credential
  clear preserving local/target-scoped state; and DOM, console, app-private
  files and logcat sensitive-data scans.

These are emulator results, not physical-device results. Full M3 still owns
the broad concurrent-device, multi-target, crash-recovery, conflict-UX,
background/lifecycle extension and physical/API-version matrices.
