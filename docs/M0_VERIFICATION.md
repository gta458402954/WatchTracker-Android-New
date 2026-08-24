# M0 verification record

M0 is the completed Android technical baseline. The repeatable Android 16
emulator evidence below is valid; physical-device Alpha validation remains an
open M1 item.

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

`npm run android:build` is the authoritative command that refreshes the
universal APK/AAB. A direct Gradle `assembleArm64Debug` (including the
instrumentation workflow) instead refreshes:

`src-tauri/gen/android/app/build/outputs/apk/arm64/debug/app-arm64-debug.apk`

After such a Gradle task, install that arm64 APK for manual/device checks. To
use the universal path, rerun `npm run android:build` first; otherwise an older
universal APK can be mistaken for the newly compiled arm64 code.

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

### 2026-08-24 M0 + M1.1 rerun

- `npm run check` passed: contract check, typecheck, lint, 171 Node tests and
  the Vite production build.
- `npm run test:e2e` passed 104/104, including `tests/mobile-shell.spec.ts`.
  The Android-user-agent/Tauri mock entered the Mobile Shell, exercised the
  FAB and local CRUD form, verified TMDB is hidden, verified locked-record
  protections, and verified that form Back traverses one history entry before
  returning to the library. Root Back is asserted as an explicit `exit` action;
  it does not claim a browser-only form-to-Activity test.
- `npm run android:build` produced the arm64 debug APK and AAB at the paths
  above. `Medium_Phone` (Android 16, `emulator-5554`) installed the APK and
  launched `com.watchtracker.android.debug` successfully.
- `npm run android:test` passed 2/2 instrumentation tests. After reinstalling
  the APK (instrumentation cleanup uninstalls it), `npm run android:smoke`
  passed: `OK CRUD=true WebDAV status=200 etag=watchtracker-m0 poster=true
  traversalRejected=true`.
- This is emulator evidence only. No physical Android handset was used, so
  M1 Alpha physical-device validation remains open.
- The final rebuilt APK was relaunched on `Medium_Phone` (Android 16). In the
  exact native sequence `settings -> tap 片库 -> system Back`, the first Back
  from the library finished `MainActivity` and the foreground became
  `com.google.android.apps.nexuslauncher/.NexusLauncherActivity`. Selecting
  片库 replaces the tab entry, while the library handler returns `exit`
  directly; there is no second Back requirement. The Android callback is a
  single `OnBackInvokedCallback` bridge, avoiding duplicate system/key dispatch.
- For the final native callback check, the installed package was reconciled
  with `adb shell pm path com.watchtracker.android.debug`, `adb pull`, and
  SHA-256 hashing. The local arm64 APK and pulled installed `base.apk` were
  both `5C47EE65ADD5C3254EA10859546FDC8DC5E67F4DF8102F18AD549803BFDD1997`
  (205,860,220 bytes). The verified launcher component was
  `com.watchtracker.android.debug/com.watchtracker.android.MainActivity`; after
  root Back the foreground component was
  `com.google.android.apps.nexuslauncher/.NexusLauncherActivity`.

## Known environment blockers

- Kotlin incremental compilation on Windows can report different-root paths
  for Tauri/plugin sources. The Gradle fallback compiler still completes the
  debug package; `gradlew --stop` can clear stale daemons.
- Android instrumentation requires an online emulator/device. A missing ADB
  target is a blocked verification, not a passing result.
