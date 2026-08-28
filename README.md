# WatchTracker Android (New)

WatchTracker Android is the mobile shell rebuilt from the V18 data and
`WatchRecord` contracts in `WatchTracker-Main`. It uses React/TypeScript for
the UI and Tauri 2 with Rust/SQLite for the local database. Android's minimum
SDK is 26 and the M0 build target is arm64.

## Current milestone boundary

M0 through M1.4 are complete for the current Alpha path. M1.4 brings the
high-value mobile sync MVP forward without declaring the full M3 reliability
Beta complete. Android 16 emulator evidence is recorded in
[docs/M1_4_VERIFICATION.md](docs/M1_4_VERIFICATION.md); physical-device and
multi-version matrices remain pending.

- Offline local CRUD is the supported path. SQLite schema V18 and atomic
  `WatchRecord` insert/get/update/delete operations are retained.
- The Android app uses its private app-data directory; it does not use the
  desktop portable-data mode.
- The `poster://` protocol is restricted to cached poster filenames inside the
  app's poster directory.
- Mobile library CRUD, lock/status operations and atomic episode tracking work
  offline and queue the existing target-scoped sync outbox.
- WebDAV settings use read-only Probe before activation. Passwords are guarded
  by Android Keystore AES-GCM and are not returned to React after saving.
- Android reuses the desktop reliability core: `records-v3.json`, payload
  V3～V6, ETag/conditional PUT, 412 retry, three-way merge, tombstones,
  generation, staging, publish intent and persistent conflicts.
- Startup, local write, network recovery, foreground resume and manual sync are
  supported. Sync failure never blocks the local library. Background
  WorkManager execution remains outside M1.4.
- SAF remains a platform Spike; product import/export is still planned for M2.

## Development

```powershell
npm ci
npm run check:m0
npm run android:init       # only when regenerating the Tauri Android shell
npm run android:build
npm run android:m14-smoke # controlled WebDAV + real Tauri/Keystore device path
```

The M0 gate includes contract generation, TypeScript typecheck, ESLint, Node
tests, Vite build, Rust fmt, clippy, and Rust tests. The general Android CI
also runs Playwright against the browser compatibility shell; its IPC is fully
mocked and does not require a real Tauri runtime. Android instrumentation
can be run after a successful Tauri build:

```powershell
$adb = 'C:\Users\markp\AppData\Local\Android\Sdk\platform-tools\adb.exe'
& $adb install -r 'src-tauri\gen\android\app\build\outputs\apk\universal\debug\app-universal-debug.apk'
& $adb shell monkey -p com.watchtracker.android.debug 1
Push-Location src-tauri\gen\android
.\gradlew.bat :app:connectedArm64DebugAndroidTest -x :app:rustBuildArm64Debug
Pop-Location
```

The APK is emitted at
`src-tauri/gen/android/app/build/outputs/apk/universal/debug/app-universal-debug.apk`;
the matching AAB is under `outputs/bundle/universalDebug`. Build outputs and
local emulator screenshots are ignored and must not be committed.

`npm run android:build` is the command that generates/refreshes the universal
APK above. A direct Gradle `assembleArm64Debug` or the arm64 instrumentation
task writes the ABI-specific APK at
`src-tauri/gen/android/app/build/outputs/apk/arm64/debug/app-arm64-debug.apk`;
install that arm64 file after such a Gradle build, or run
`npm run android:build` again before installing the universal file. Do not
assume the universal APK changed just because an arm64 Gradle task completed.

See [docs/M1_4_VERIFICATION.md](docs/M1_4_VERIFICATION.md) and
[docs/ARCHITECTURE.md](docs/ARCHITECTURE.md) for the reproducible checks,
platform boundaries, and known environment limitations.
