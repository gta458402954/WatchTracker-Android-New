# M2.1 verification record

M2.1 adds Android local JSON export only. Import, validation preview, database
replacement, recovery-point UI, restore, and maintenance remain pending.

## Export contract

- Desktop contract: reused from
  `../WatchTracker-Main/src/features/settings/hooks/useImportExport.ts`.
- Envelope: `formatVersion: 4`, `exportedAt`, `records`,
  `episodeCompletions`, `collections`, and `collectionMembers`.
- Serialization matches desktop `JSON.stringify(payload, null, 2)`.
- `exportedAt` is ISO 8601; the suggested filename uses local time as
  `WatchTracker-backup-YYYY-MM-DD-HHmmss.json`.
- `get_local_export_snapshot` performs the four entity reads under one database
  mutex/connection and does not initialize or modify sync state.

## SAF and security boundary

- Android uses `ACTION_CREATE_DOCUMENT`, `CATEGORY_OPENABLE`, MIME
  `application/json`, and `EXTRA_TITLE`.
- Only a `content://` result URI is accepted. `ContentResolver.openOutputStream`
  then receives the complete UTF-8 JSON stream. User cancellation is a normal
  `cancelled` result; URI validation and write/open/close failures return
  `error`.
- No traditional or all-files storage permission is requested.
- The platform bridge receives a UUID token, not a file path. It can copy only
  a JSON stage in the app-private `export-staging` directory. Stages are
  removed after saved/cancelled/error and stale stages are cleaned at startup.
- The serializer is a top-level whitelist. WebDAV/TMDB credentials, secure
  vault data, settings, outbox, scheduler, staging, publish intent, conflicts,
  baseline, ETag, recovery files, logs, and poster bytes are excluded.

## Automated coverage

- Node tests verify format version, timestamp, all four entity arrays,
  top-level exclusions, JSON round-trip, desktop serialization, and stable
  filename formatting.
- Rust tests verify all four entities are read and the complete settings table
  is byte-for-byte unchanged; staging rejects non-V4 envelopes and unsafe
  tokens.
- Playwright covers settings copy, no-WebDAV success, busy/saved state,
  cancellation without an error notice, write failure, paused/error sync
  independence, exact whitelist, and unchanged record count.
- Android instrumentation preserves the M0 `ACTION_OPEN_DOCUMENT` test and adds
  an independent `ACTION_CREATE_DOCUMENT` intent contract test.

## Repeatable commands

```powershell
git diff --check
npm run check:m0
npm run test:e2e
npm run android:build
npm run android:test # when an emulator/device is connected; build first
```

The Android picker/save smoke must use the APK produced by the immediately
preceding `android:build`. Confirm that the selected JSON is non-empty,
parseable, reports `formatVersion: 4`, has the expected record count, and does
not change the local database.

## 2026-08-29 branch evidence

- `npm run check:m0`: PASS (187 Node tests and 98 Rust tests).
- Full `npm run test:e2e`: PASS (136/136). The M2.1-only suite passed 3/3 and
  includes 360×740 coverage for the settings page.
- `npm run android:build`: PASS; the current universal debug APK was produced
  at `src-tauri/gen/android/app/build/outputs/apk/universal/debug/`.
- `npm run android:test`: PASS (4/4) on physical device `25053RT47C`, Android
  16. An initial ADB-daemon startup race failed non-zero before device
  execution; restarting the configured SDK ADB daemon and rerunning the same
  command completed all tests.
- `npm run android:m12-smoke`: PASS.
- `npm run android:m13-smoke`: PASS.
- `npm run android:m14-smoke`: PASS. The installed APK SHA-256 matched the
  just-built APK: `18E1B84C5EAD55047ED20807CE97E72172F33000CC77713128B571FDA4B37107`.
- System picker/save smoke: PASS. The app opened DocumentsUI in Downloads with
  the suggested `WatchTracker-backup-2026-08-29-231913.json` name; the user
  confirmed Save. The resulting 2,412-byte file parsed as `formatVersion: 4`
  with the exact six-key whitelist, 1 record, 3 episode completions, 0
  collections, and 0 collection members. The library still contained the same
  single visible record after export. DocumentsUI was not driven with fixed
  coordinate automation.

## Remaining M2

- M2.2 import plus schema validation and preview.
- M2.3 recovery-point UI and restore.
- M2.4 database and cache maintenance.
