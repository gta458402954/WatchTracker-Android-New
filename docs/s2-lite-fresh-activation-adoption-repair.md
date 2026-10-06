# Fresh-client activation adoption repair

Android baseline: `17d70069d97237a83a9209ce12947d0dcf0a5d87`.
Desktop remains unchanged at `ed12d4f551f26c622b7037394e9146ea8f6fa17a`.
All verification here uses production SQLite and fake WebDAV; no provider run.

## Durable compatibility authority

The existing `s2_lite_remote_activation_adoption_v1` row establishes the basis:

- No row: UNESTABLISHED.
- Row with `legacyFingerprint: null`: ESTABLISHED(None).
- Row with a fingerprint: ESTABLISHED(Some(fingerprint)).

An additive `adoptedRemoteBasis: true` field identifies first capture from verified
remote evidence. Old local-snapshot rows omit the field and retain their strict
snapshot-derived nullable fingerprint validation. The field defaults to false
and false is omitted when serialized, preserving the strict durable decoder's
round-trip check for existing rows. No SQL schema/version change is needed.

On reconciliation of validated, consistent activation evidence, a root without a
local migration binding captures its first basis before saving the activation
latch. Existing local legacy baseline/business evidence remains a snapshot basis;
only an empty client without that authority captures the remote nullable value.
Retained rows are validated and compared exactly; they are never replaced.

Basis insertion, root latch/fatal changes, and projection cache refresh share the
caller's `BEGIN IMMEDIATE` transaction. Successful adoption cannot commit only
one half. Conflicting/unsupported/fatal remote evidence cannot establish a first
basis. Fatal cutover evidence still permanently blocks S1.

Read-only adoption uses the projection row's durable applied-generation authority
without creating writer state. Existing writer bookkeeping remains checked and
updated when present. Subsequent local work allocates the stable writer normally
and initializes its projection bookkeeping from the retained projection.

## Counterexample and focused evidence

Before implementation, the new fresh-client production-cycle regression failed
with `ProtocolError("SYNC_ROOT_FROZEN_LEGACY_CHANGE")` after replaying a migrated
root with a non-null fingerprint. After repair, the regression uses eight entities
(two records, collections, members, and completions), advances business-applied
generation, populates all four tables, and allocates neither writer nor intent.

Eight new tests cover:

- Fresh non-null migration adoption through discovery/replay/business application.
- Established None/None and Some(A)/Some(A) equality; None/Some, Some(A)/Some(B),
  and Some(A)/None durable legacy-change freezes, including old-format rows.
- First None and Some capture with two consistent activation observations;
  restart before business application; permanent S1 callback rejection and no
  fresh migration/publication.
- SQLite failures at basis insertion, root update, and projection insertion:
  restart sees pre-adoption authority, then deterministically resumes.
- Disagreeing initial and late activation evidence: durable freeze, no basis
  replacement, no writes.
- Later local edit captures a verified Live basis, allocates writer only then,
  publishes sequence 1, and retains identity across restart.
- Corrupt basis fails closed after restart and cannot reopen S1.

The existing four-entity projection and business rollback tests now also exercise
application without a writer. The old nullable-mismatch test explicitly installs
an established null basis rather than incorrectly treating a fresh DB as one.

| Focused group | Passing tests |
| --- | ---: |
| Migration/runtime, including adoption and I6.4/I6.5 | 83 |
| Durable discovery/projection | 20 |
| Frozen remote discovery | 16 |
| Frozen causal replay | 21 |
| Frozen activation/cutover | 7 |
| Frozen migration orchestration | 23 |
| S1 local import | 30 |
| Total unique focused tests | 200 |

Gates: `cargo fmt -- --check`, `cargo check --locked`,
`cargo clippy --all-targets --all-features --locked -- -D warnings`, and
`git diff --check` pass. No frozen contracts, transport, dependencies, Desktop
code, or S1 payload/parser implementation changed. No push; candidate awaits
Astra focused review before interoperability resumes.
