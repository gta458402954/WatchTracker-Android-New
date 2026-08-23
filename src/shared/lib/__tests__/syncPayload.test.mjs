import assert from 'node:assert/strict';
import test from 'node:test';
import { parseSyncPayloadV3 } from '../syncMerge.ts';
import {
  emptyCollectionState,
  legacyPayload,
  buildSyncPayload,
  sideOfLegacy,
} from '../../../features/sync/domain/syncPayload.ts';

const now = '2026-08-22T00:00:00.000Z';
const record = (fields = {}) => ({
  id: 'r1', originalName: '', chineseName: '测试', progress: '', totalEpisodes: null,
  movieProgress: null, movieDuration: null, releaseYear: null, posterPath: null,
  status: '未看', platform: '', rating: null, startDate: '', endDate: '', notes: '',
  createdAt: now, imdbId: null, mediaType: '电影', ...fields,
});
const current = (schemaVersion) => ({
  schemaVersion, documentId: 'doc', revision: 1, commitId: 'old', parentCommitId: null,
  writerId: 'device', committedAt: now, records: [], tombstones: [],
});

test('legacy payload accepts arrays and v2 objects but rejects future schema', () => {
  const parsed = legacyPayload([record()]);
  assert.equal(parsed.schemaVersion, 2);
  assert.equal(parsed.records.length, 1);
  assert.equal(sideOfLegacy({ ...parsed, tombstones: [{ id: 'r2', deletedAt: now }] }).tombstones[0].revActor, 'legacy-v2');
  assert.throws(() => legacyPayload({ schemaVersion: 7, records: [] }), /unsupported_remote_schema/);
  assert.throws(() => legacyPayload({ schemaVersion: 3, records: [] }), /unexpected_v3_legacy_resource/);
});

test('commit payload selects the minimum V3-V6 envelope for domain data', () => {
  const side = { records: [record()], tombstones: [] };
  const noExtras = buildSyncPayload(current(3), side, [], emptyCollectionState(), 'device', now, 'commit-v3');
  assert.equal(noExtras.schemaVersion, 3);
  assert.equal(noExtras.commitId, 'commit-v3');
  const v4 = buildSyncPayload(current(3), { records: [record({ episodeTrackingEnabled: true })], tombstones: [] }, [], emptyCollectionState(), 'device', now, 'commit-v4');
  assert.equal(v4.schemaVersion, 4);
  const v5 = buildSyncPayload(current(5), side, [], emptyCollectionState(), 'device', now, 'commit-v5');
  assert.equal(v5.schemaVersion, 5);
  const v6 = buildSyncPayload(current(3), { records: [record({ tmdbId: 42 })], tombstones: [] }, [], emptyCollectionState(), 'device', now, 'commit-v6');
  assert.equal(v6.schemaVersion, 6);
  assert.equal(v6.revision, 2);
});

test('existing V3 parser rejects V7+ and validates the constructed envelope', () => {
  assert.throws(() => parseSyncPayloadV3({ schemaVersion: 7, records: [] }), /unsupported_remote_schema/);
  const payload = buildSyncPayload(current(3), { records: [record()], tombstones: [] }, [], emptyCollectionState(), 'device', now, 'commit-parse');
  assert.equal(parseSyncPayloadV3(payload).schemaVersion, 3);
});
