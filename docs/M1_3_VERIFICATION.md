# M1.3 verification record

M1.3 adds mobile episode tracking and high-frequency episode progress actions
on top of the completed M1.2 offline library. The shared Rust episode-history
transactions remain authoritative; no schema or migration changes are part of
this milestone.

## Scope and invariants

- Mobile controls appear only for non-film records with a positive integer
  `totalEpisodes`. A legacy `progress` string is displayed and preserved, but
  is never parsed into completions.
- Enabling tracking requires an explicit initial next episode. Complete,
  jump, retreat, finish and resume all call the existing Rust commands with
  `expectedRev` and adopt their persisted `record + completions` response.
- Advancing records skipped episodes with unknown completion time and the
  boundary episode with a known time. Retreat keeps existing history. Finishing
  updates history, status and end date in the same Rust transaction.
- A completed tracked record offers resume only when its current total exceeds
  the highest persisted episode history. The previous end date and history are
  retained by the shared Rust rule.
- Locked records are read-only. Stale, missing and locked command failures do
  not produce success UI; the repository reloads Rust state and reports a safe
  Chinese message.
- Episode controls introduce no overlay. Existing filter-sheet and dirty-form
  Back priority, detail/form history, library-root exit and cold-start routing
  remain unchanged.

## Automated coverage

- `mobileEpisodeTracking.test.mjs` covers eligibility, action derivation,
  complete/finish boundaries, resume eligibility and safe error classification.
- `mobile-episode-m13.spec.ts` covers legacy progress, enable, list quick
  completion, jump, retreat/history retention, atomic finish, stale, locked,
  missing, resume, accessible names and 48 px touch targets.
- `android:m13-smoke` drives the installed app through the real Tauri bridge,
  creates a disposable episodic record, enables tracking, advances, retreats,
  finishes, reads persisted record/history state and deletes the fixture. CDP,
  PID and bridge waits are bounded; failures exit non-zero and adb forwarding
  is cleaned in `finally`.

## Repeatable final gate

Run from `WatchTracker-Android-New`:

```powershell
git diff --check
npm run check:m0
npm run test:e2e
npm run android:build
npm run android:test
# Reinstall the APK produced by the immediately preceding build and verify SHA-256.
npm run android:smoke
npm run android:m12-smoke
npm run android:m13-smoke
```

## 2026-08-26 emulator evidence

- `git diff --check`: exit 0.
- `npm run check:m0`: exit 0; Node 182/182 and Rust 92/92 passed, together
  with contract drift, TypeScript, ESLint, production build, rustfmt and
  clippy gates.
- `npm run test:e2e`: exit 0, Playwright 119/119 passed. The M1.3 subset is
  8/8 and the targeted M1.2/mobile-shell/desktop-episode regression subset is
  19/19.
- `npm run android:build`: exit 0 and produced the universal debug APK/AAB.
  The installed APK was rebuilt from this working tree and reinstalled with
  SHA-256
  `94C5C466E293C8667808F063E9E2C3E865A9D203D75AD5570578E6FBCB2E6C38`.
- `npm run android:test`: the first attempt correctly failed non-zero because
  adb had no connected device. After starting the existing `Medium_Phone` AVD
  (Android 16, `emulator-5554`), the unchanged command passed 2/2
  instrumentation tests with exit 0.
- The old M0 CDP readiness probe encountered a WebView target re-enumeration
  on this cold launch. Its readiness phase now performs the same bounded
  one-time socket reconnect used by the M1.2 pattern; assertions were not
  changed. `npm run android:smoke` then passed with real CRUD, WebDAV mock,
  poster load and traversal rejection.
- `npm run android:m12-smoke`: exit 0 with mobile shell, navigation, form/detail
  Back, Rust array read and sensitive-text checks passing.
- `npm run android:m13-smoke`: exit 0 with real Tauri bridge enable, advance,
  retreat, atomic finish and persisted history `2,3,4,5` at record revision 5;
  the disposable record and adb forward were cleaned.
- After `pm clear`, the cold-start M1.2 smoke and M1.3 bridge smoke both passed
  again with exit 0, starting from an empty local database.

These are Android emulator results, not physical-device results. Physical
phone, API 26–32, rotation and 200% font-scale matrices remain release-level
evidence outside this M1.3 batch.
