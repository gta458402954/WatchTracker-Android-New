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
- The earlier instrumentation WebView harness was removed from the gate after
  it proved sensitive to Activity/WebView timing; the stable external CDP
  smoke below is the reproducible device path.
- The stable external smoke is `npm run android:smoke` (Node 24 native
  WebSocket + adb/CDP). After installing the current APK it completed:
  `OK CRUD=true WebDAV status=200 etag=watchtracker-m0 poster=true
  traversalRejected=true`. It uses a public controlled ETag endpoint, emits
  only status/ETag shape/value, and removes its adb forward in `finally`.
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

- The external CDP smoke now provides the successful device-side HTTPS GET,
  status/ETag/JSON, and WebView poster load/traversal evidence above. The
  public endpoint is a transport Spike only and is not production sync
  configuration.

## Known environment blockers

- Kotlin incremental compilation on Windows can report different-root paths
  for Tauri/plugin sources. The Gradle fallback compiler still completes the
  debug package; `gradlew --stop` can clear stale daemons.
- Android instrumentation requires an online emulator/device. A missing ADB
  target is a blocked verification, not a passing result.
