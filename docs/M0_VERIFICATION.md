# M0 verification record

Run from `WatchTracker-Android-New`.

```powershell
npm ci
npm run check:m0
npx tauri android init --ci
npm run android:build
```

The arm64 debug Rust build and Gradle packaging produce:

`src-tauri/gen/android/app/build/outputs/apk/universal/debug/app-universal-debug.apk`

The APK is a universal debug container with the arm64 Rust library included;
the generated project is configured for arm64 as the M0 target. The matching
debug AAB is emitted beside it under `outputs/bundle/universalDebug`.

With an emulator or device online:

```powershell
$adb = 'C:\Users\markp\AppData\Local\Android\Sdk\platform-tools\adb.exe'
& $adb install -r 'src-tauri/gen/android/app/build/outputs/apk/universal/debug/app-universal-debug.apk'
& $adb shell monkey -p com.watchtracker.android.debug 1
Push-Location src-tauri/gen/android
.\gradlew.bat :app:connectedArm64DebugAndroidTest -x :app:rustBuildArm64Debug
Pop-Location
```

`PlatformSpikeTest` proves an app-private writable file, Android Keystore AES
round-trip, and a SAF JSON open intent. The installed-app IPC smoke also
performed real insert/get/update/delete calls, then force-stopped and cold-
started the app; the updated record remained and the deleted record remained
absent. Poster protocol path validation is covered by the Rust boundary tests.
Record emulator serial, install output, and `adb logcat` evidence in the
release checklist; do not claim a device result when `adb devices` is empty.

### 2026-08-23 local evidence

- Rebuilt the APK after the offline-startup and WebDAV log-redaction changes;
  the resulting package was generated at 09:07 local emulator time and
  installed with `adb install -r` before launch. The fresh process emitted the
  private app-data startup log and reached the record-list shell rather than a
  credential/loading error.
- The fresh build's `connectedArm64DebugAndroidTest` completed 2/2 on
  `Medium_Phone` (Android 16). The test task uninstalls the app as part of its
  cleanup, so reinstall the APK before any subsequent manual UI check.

- `emulator-5554` (`Medium_Phone`, Android 16) reached `device` state.
- `adb install -r` returned `Success`; `monkey -p com.watchtracker.android.debug`
  launched `MainActivity`.
- Rust startup log reported the private data root and created
  `watchtracker.db`, `posters/`, and `backups/` under the app sandbox.
- `connectedArm64DebugAndroidTest` passed 2/2 after explicitly configuring
  `androidx.test.runner.AndroidJUnitRunner`.
- The new `DeviceSmokeTest` is checked in and exercises the installed app's
  WebView through `window.__TAURI_INTERNALS__.invoke`, including CRUD, the
  WebDAV probe, and `convertFileSrc` poster assertions. The first run exposed
  and fixed a harness bug that treated its `started` marker as success. A
  subsequent run reached the real WebView/IPC stage but the target activity
  did not expose a WebView before the bounded wait (`Tauri WebView was not
  found`). This remains a failed/blocked gate, not a passing CRUD or network
  claim.
- After `am force-stop` and a second `monkey` launch, the same
  `watchtracker.db` (110,592 bytes) remained and `app.log` recorded a second
  startup against the same app-private path, proving restart persistence of
  the V18 database container. The installed-app IPC smoke separately recorded
  insert/get/update/delete and force-stop/cold-start persistence evidence.
- The direct Gradle instrumentation attempt without the Tauri CLI build
  websocket is expected to fail at `rustBuildArm64Debug`; the reproducible
  workaround is the `-x :app:rustBuildArm64Debug` command above after a
  successful `tauri android build`.

### Evidence limits

- No successful independent device-side HTTPS WebDAV GET run was captured;
  the checked-in smoke currently blocks at the Tauri invoke bridge. Transport
  and redaction unit tests remain the available evidence.
- No successful Android WebView `poster://` load/reject trace was captured;
  the checked-in smoke currently blocks before that assertion. Rust
  path/signature tests cover the fail-closed boundary.

## Known environment blockers

- Kotlin incremental compilation on Windows can report different-root paths
  for Tauri/plugin sources. The Gradle fallback compiler still completes the
  debug package; `gradlew --stop` can clear stale daemons.
- Android instrumentation requires an online emulator/device. A missing ADB
  target is a blocked verification, not a passing result.
