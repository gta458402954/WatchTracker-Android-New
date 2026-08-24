# Upstream baseline

The Android-New branch is based on `WatchTracker-Main` commit
`bbf510f04edbeeda5435bc2f2d301e95818c3bdd` (`fix(tmdb): retain season numbers
in titled seasons`). The baseline is intentionally copied as TypeScript and
tests rather than linked to the desktop repository, so Android CI remains
reproducible and the two repositories can evolve independently.

## Shared files synchronized from Main

- `src/features/collections/lib/seriesDiscovery.ts`
- `src/features/collections/lib/tmdbRecordMapping.ts`
- `src/shared/lib/batchMetadata.ts`
- `src/shared/lib/displayTitle.ts`
- `src/shared/lib/seasonTitles.ts`
- `src/shared/lib/__tests__/batchMetadata.test.mjs`
- `src/shared/lib/__tests__/displayTitle.test.mjs`
- `src/shared/lib/__tests__/seasonTitles.test.mjs`
- `src/shared/lib/__tests__/seriesDiscovery.test.mjs`
- `src/shared/lib/__tests__/tmdbRecordMapping.test.mjs`

These files preserve season numbers while retaining localized/original season
subtitles and reject conflicting season identities. Future Main changes to
these paths should be reviewed and ported deliberately.

## Android-specific differences

- `src/platform/runtime.ts` selects Android only for Tauri + Android, while
  desktop Tauri and browser tests retain the compatibility shell.
- `src/app/MobileApp.tsx` and `src/platform/navigation.ts` provide the M1.1
  mobile shell and history navigation.
- Android startup treats optional credential/settings reads as non-blocking;
  local V18 readiness is not gated on a production Android secret adapter.
- `src-tauri/gen/android`, private app-data paths, Android CI and device smoke
  scripts are platform artifacts and are not synchronized back into Main.

Do not modify `WatchTracker-Main` or the legacy `WatchTracker-Android` while
updating this baseline.
