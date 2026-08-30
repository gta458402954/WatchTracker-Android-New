# M2.2 verification record

M2.2 adds safe Android local JSON import, schema/domain validation, a true
read-only preview, stale-preview protection, a pre-import recovery point and
atomic four-entity replacement. Recovery-point management/restore UI remains
M2.3; database/cache maintenance remains M2.4.

## Accepted backup contract

- Authority audited: `../WatchTracker-Main/src/features/settings/hooks/useImportExport.ts`.
- Accepted here: current local-backup `formatVersion: 4` only, with the exact
  top-level whitelist `formatVersion`, `exportedAt`, `records`,
  `episodeCompletions`, `collections`, and `collectionMembers`.
- `exportedAt` must be RFC3339. Unknown V4 top-level fields and future versions
  fail closed. The desktop V3/V2/bare-array compatibility branches are legacy
  compatibility paths and are intentionally not folded into this destructive
  Android import boundary.
- Old WebDAV `records.json`, V9 SQLite, arbitrary databases and credentials/
  settings/sync runtime are not accepted or restored.

## SAF and staging boundary

- `AndroidDocumentImporter` builds `ACTION_OPEN_DOCUMENT`, adds
  `CATEGORY_OPENABLE`, and uses MIME `application/json`.
- Only `content://` is accepted. No traditional or all-files storage permission
  is present. No persisted URI grant is needed because the selected stream is
  immediately copied into the app sandbox.
- Copying runs off the UI thread and counts actual streamed bytes against a
  128 MiB maximum. `<uuid>.tmp` is flushed/synced and renamed to `<uuid>.json`.
  Partial files are deleted on failure; valid UUID `.tmp`/`.json` stages are
  cleaned at startup.
- Rust commands accept a canonical lowercase UUID token only. React sees token,
  safe filename, byte count, preview and stable error codes—not file contents or
  filesystem paths.

## Validation, preview and commit

- Rust is the validation authority. It validates JSON shape, the strict V4
  envelope, record IDs/duplicates/enums/ranges/revisions, episode bounds and
  references, and collection/member IDs, values and references. Existing
  `prepare_import_batch`, `replace_completions_tx`, collection validators and
  SQLite constraints remain authoritative.
- Preview copies the live SQLite database into memory and runs the same
  `replace_library_atomic` path used by confirm. It reports file metadata, all
  four source counts, and records added/updated/removed/unchanged, final count,
  and locked records protected. Locked episode-history differences also count
  as protected.
- Preview does not write library rows, settings, generation, outbox, staging,
  device identity or recovery state.
- Preview returns SHA-256 of the staged bytes and a deterministic SHA-256 of the
  current four entity sets. Confirm re-reads/revalidates the file and rechecks
  both values while holding the one database mutex. A changed stage returns
  `import_stage_changed`; changed local state returns `import_preview_stale`.
- After validation/freshness succeeds, confirm creates exactly one recovery
  point from the actual pre-import connection and invokes the existing
  transactional four-entity replacement. Locked records and their episode
  history remain local; successful replacement enters the existing local
  mutation/outbox/sync-staging path. The UI reloads records from Rust and only
  wakes the existing sync coordinator.

## Automated coverage

- Rust M2.2 module: 29 tests covering valid V4 and M2.1 round-trip,
  malformed/missing/old/future/
  unknown-field envelopes, metadata, duplicate/invalid records, episode and
  collection relationships, preview read-only behavior, complete diff and
  locked-history semantics, actual recovery contents, four-entity commit,
  locked commit behavior, sync staging, invalid/stale/changed-stage rejection,
  stage lifecycle, unsafe tokens, 128 MiB cap and startup cleanup.
- Playwright `tests/mobile-import-m22.spec.ts`: picker visibility/cancel,
  read-only preview, four counts and five diff classes, authoritative reload,
  invalid/future rejection without confirm, stale re-preview, WebDAV/sync-state
  independence, stage discard and 360×740 overflow coverage.
- Android instrumentation directly tests the production import adapter Intent
  alongside the preserved M0 `ACTION_OPEN_DOCUMENT` and M2.1
  `ACTION_CREATE_DOCUMENT` contracts.

## Current verification evidence

Verified on 2026-08-30 from `feat/android-m22-import` before commit:

- `git diff --check`: pass.
- `npm run check:m0`: pass (189 Node tests and 127 Rust tests).
- `npm run test:e2e`: pass (143/143, including M2.2 7/7 and M2.1 3/3).
- `npm run android:build`: pass; the current universal Debug APK and AAB were
  produced from this working tree.
- `adb devices -l`: no connected device/emulator. Consequently
  `npm run android:test`, M1.2/M1.3/M1.4 device smoke, and the real DocumentsUI
  import smoke remain pending rather than being reported as passed.

## Repeatable gates

```powershell
git diff --check
npm run check:m0
npm run test:e2e
npm run android:build
npm run android:test # device/emulator available; build first
npm run android:m12-smoke
npm run android:m13-smoke
npm run android:m14-smoke
```

The real picker/import smoke must use the debug APK from the immediately
preceding build. It requires a human to select the M2.1 V4 backup in
DocumentsUI; no fixed picker coordinates are automated. Verify preview counts,
unchanged database before confirm, post-import records and episode history,
locked protection, a real pre-import recovery database, and no WebDAV
requirement.

## Remaining M2

- M2.3 recovery-point management and restore UI.
- M2.4 database/cache maintenance.
