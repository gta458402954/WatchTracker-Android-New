# WatchTracker Android (New)

WatchTracker Android is the mobile M0 shell rebuilt from the V18 data and
`WatchRecord` contracts in `WatchTracker-Main`. It uses React/TypeScript for
the UI and Tauri 2 with Rust/SQLite for the local database. Android's minimum
SDK is 26 and the M0 build target is arm64.

## Current M0 boundary

- Offline local CRUD is the supported path. SQLite schema V18 and atomic
  `WatchRecord` insert/get/update/delete operations are retained.
- The Android app uses its private app-data directory; it does not use the
  desktop portable-data mode.
- The `poster://` protocol is restricted to cached poster filenames inside the
  app's poster directory.
- Keystore and SAF are verified as platform Spikes. A production Android
  secret-store adapter is not implemented yet, so TMDB/WebDAV credentials
  cannot be configured on Android. WebDAV is currently a transport GET Spike
  only; credential configuration is an M1/M2 prerequisite.

## Development

```powershell
npm ci
npm run check:m0
npm run android:init       # only when regenerating the Tauri Android shell
npm run android:build
```

The M0 gate includes contract generation, TypeScript typecheck, ESLint, Node
tests, Vite build, Rust fmt, clippy, and Rust tests. Android instrumentation
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

See [docs/M0_VERIFICATION.md](docs/M0_VERIFICATION.md) and
[docs/ARCHITECTURE.md](docs/ARCHITECTURE.md) for the reproducible checks,
platform boundaries, and known environment limitations.
