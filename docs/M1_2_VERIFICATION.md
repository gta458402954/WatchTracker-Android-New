# M1.2 verification record

M1.2 adds the mobile offline library surface on top of the M0 Android
container. The old desktop UI and `WatchTracker-Android` are out of scope and
must remain untouched.

## Scope

- `mobile_library_preferences_v1` persists view mode, sort mode, and basic
  status/media/lock filters only. Search text, scroll position, and form
  drafts are never persisted.
- Library search is debounced by 250 ms and covers Chinese/original title,
  platform, and notes. List and two-column poster views use the safe poster
  protocol with a local fallback; no bulk remote poster download is started.
- Detail and full-screen form routes use typed hashes (`#detail/:id`,
  `#form/new`, `#form/edit/:id`). Android Back restores the previous route.
- The Android bridge synchronously consumes the JavaScript back result: an
  overlay returns `consumed`, route history returns `history`, and only the
  library root returns `exit`. Save success marks the form clean and bypasses
  the dirty-leave guard before returning.
- Preference reads require `version: 1`, validate enum values, deduplicate
  filters, and fall back with a notice on corrupt/read/write failures without
  blocking record CRUD. Poster grids use fallback-only loading; detail poster
  retry is explicit and independent.
- Mobile writes reload the Rust source of truth and compare the displayed
  revision and lock state before issuing unchanged CRUD commands. This is a
  stale-write reduction guard, not an atomic CAS; Rust CRUD/schema was not
  changed for M1.2.

## Repeatable checks

Run from `WatchTracker-Android-New`:

```powershell
npm run check:m0
npm run test:e2e
npm run android:build
npm run android:test
npm run android:smoke
npm run android:m12-smoke
```

The authoritative debug APK path is:

`src-tauri/gen/android/app/build/outputs/apk/universal/debug/app-universal-debug.apk`

After a direct arm64 Gradle test, reinstall the arm64 APK instead of relying
on an older universal APK. Confirm the installed SHA-256 with `Get-FileHash`
and the APK path printed by the build before launching the emulator.

## Evidence checklist

- Node model tests include preference normalization, search/filter, null-last
  deterministic sorting, and canonical dirty comparison.
- Existing Node suite remains green (the M1.2 additions bring the total to
  178 passing tests); the full Playwright suite is 111/111 (104 existing plus
  7 M1.2 tests). Playwright uses a direct Vite process and exits with code 0.
- Android instrumentation remains the M0 gate; M1.2 device smoke checks the
  real installed WebView shell and Tauri bridge. The result is emulator/device
  evidence only, never a physical-device claim.
- Remaining Alpha evidence: a physical Android phone, API 26–32 matrix,
  rotation/font-scale screenshots, and the full automated M1.2 interaction
  suite for device-level filter/detail/form race cases. The deterministic
  1,000-record derived-query budget test completes in under 200 ms (current
  Node run is ~2 ms).

## 2026-08-25 emulator evidence

- `Medium_Phone` (Android 16, `emulator-5554`) was started and reached
  `device` state.
- The freshly built universal debug APK was installed successfully. SHA-256:
  `B83655324E4B8CC8ABB883903178AEC75276050DA9E0EC03A34B35F7DC3808E1`.
- `npm run android:test`: `BUILD SUCCESSFUL`, 2/2 instrumentation tests on
  `Medium_Phone (AVD) - 16`.
- The instrumentation task uninstalls the app during cleanup; the latest APK
  was reinstalled before smoke checks.
- `npm run android:smoke`: `OK CRUD=true WebDAV status=200
  etag="watchtracker-m0-local" endpoint=local-mock poster=true
  traversalRejected=true`. The smoke uses a host-bound stable mock by default
  (`10.0.2.2` from the emulator); set `M0_WEBDAV_URL` to exercise an external
  endpoint. Both modes have a bounded 15-second probe timeout.
- After `pm clear` cold-start validation, `npm run android:m12-smoke`:
  `OK M1.2 shell=true navigation=true library=true add=true formBack=true
  detailBack=true readOk=true readArray=true records=0 sensitiveText=false`
  (repeated successfully on the same installed APK). Mobile initialization
  waits briefly for Android/Tauri setup before the first database read. The smoke now requires a
  successful `get_all_records` result whose value is an array, reconnects to
  a re-enumerated WebView CDP target after navigation, and cleans the socket
  and adb forward on every exit path.

These are Android emulator results. They are not physical-device results.
