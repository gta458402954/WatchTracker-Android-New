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
- Android 16 `Medium_Phone` AVD: `npm run android:test` passed 5/5 after the
  current Debug build. M1.2, M1.3, and M1.4 device smoke all passed; M1.4 also
  verified the installed and built APK SHA-256 matched
  `67ED274D0E7EDD406A29D1D8CD7DC1D368FBBCD0DF19B4E923363C4D671F7574`.
- Real SAF round-trip: M2.1 saved
  `WatchTracker-backup-2026-08-30-091327.json` (2,624 bytes) through
  DocumentsUI. After local mutation, M2.2 opened `ACTION_OPEN_DOCUMENT` and a
  human selected that file without coordinate automation. Preview reported 2
  records, 1 completion, 0 collections/members, 1 update, 1 removal, 0
  unchanged, 1 locked preservation, and final count 2. The live database still
  held all 3 pre-import records and both local locked completions before
  confirmation.
- Confirm succeeded without WebDAV. The unlocked record returned to its backup
  value, the extra record was removed, and the local locked record plus episode
  2 and 3 completions were preserved. One new import recovery point was created
  with record count 3; its SQLite bytes were opened independently,
  `PRAGMA integrity_check` returned `ok`, and its rows contained the complete
  pre-import three-record/two-completion state. Import staging was empty after
  success and the app remained responsive.

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
