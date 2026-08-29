import assert from 'node:assert/strict';
import test from 'node:test';
import {
  buildLocalBackupV4,
  localBackupFileName,
  serializeLocalBackupV4,
} from '../../../features/backup/localBackupV4.ts';

const record = {
  id: 'record-1', originalName: 'Original', chineseName: '记录', progress: '',
  totalEpisodes: null, movieProgress: null, movieDuration: 7200, releaseYear: '2026',
  posterPath: null, status: '未看', platform: '', rating: null, startDate: '', endDate: '',
  notes: 'user text may contain password without becoming a credential field',
  createdAt: '2026-08-29T00:00:00.000Z', imdbId: null, mediaType: '电影',
};
const completion = {
  id: 'completion-1', recordId: record.id, episodeNumber: 1,
  completedAt: '2026-08-29T01:00:00.000Z', createdAt: '2026-08-29T01:00:00.000Z',
  updatedAt: '2026-08-29T01:00:00.000Z', rev: 1, revActor: 'device',
};
const collection = {
  id: 'collection-1', name: '收藏', normalizedName: '收藏', description: null,
  sourceKind: 'manual', sourceKey: null, collectionKind: 'manual', orderMode: 'manual',
  createdAt: '2026-08-29T00:00:00.000Z', updatedAt: '2026-08-29T00:00:00.000Z',
  rev: 1, revActor: 'device',
};
const member = {
  id: 'member-1', collectionId: collection.id, recordId: record.id, position: 0,
  sourceKind: 'manual', createdAt: '2026-08-29T00:00:00.000Z',
  updatedAt: '2026-08-29T00:00:00.000Z', rev: 1, revActor: 'device',
};

test('local backup V4 reuses the desktop whitelist and preserves all exported entities', () => {
  const now = new Date('2026-08-29T09:35:00.123Z');
  const syncLikeSnapshot = {
    records: [record],
    episodeCompletions: [completion],
    collections: [collection],
    collectionMembers: [member],
    password: 'must-not-export',
    credentials: { username: 'must-not-export' },
    syncRuntime: {}, scheduler: {}, outbox: {}, staging: {}, publishIntent: {},
    conflicts: [], remoteEtag: 'must-not-export', baseline: {},
  };

  const payload = buildLocalBackupV4(syncLikeSnapshot, now);

  assert.equal(payload.formatVersion, 4);
  assert.equal(payload.exportedAt, '2026-08-29T09:35:00.123Z');
  assert.deepEqual(payload.records, [record]);
  assert.deepEqual(payload.episodeCompletions, [completion]);
  assert.deepEqual(payload.collections, [collection]);
  assert.deepEqual(payload.collectionMembers, [member]);
  assert.deepEqual(Object.keys(payload), [
    'formatVersion', 'exportedAt', 'records', 'episodeCompletions', 'collections', 'collectionMembers',
  ]);
  for (const excluded of [
    'password', 'credentials', 'syncRuntime', 'scheduler', 'outbox', 'staging',
    'publishIntent', 'conflicts', 'remoteEtag', 'baseline',
  ]) assert.equal(Object.hasOwn(payload, excluded), false, excluded);
});

test('local backup serialization is readable JSON and round-trips without a second contract', () => {
  const payload = buildLocalBackupV4({
    records: [record], episodeCompletions: [completion], collections: [collection], collectionMembers: [member],
  }, new Date('2026-08-29T09:35:00.123Z'));
  const serialized = serializeLocalBackupV4(payload);

  assert.equal(serialized, JSON.stringify(payload, null, 2));
  assert.deepEqual(JSON.parse(serialized), payload);
});

test('suggested backup filename uses stable local fields and contains no illegal time separator', () => {
  const fileName = localBackupFileName(new Date(2026, 7, 29, 17, 35, 0));
  assert.equal(fileName, 'WatchTracker-backup-2026-08-29-173500.json');
  assert.match(fileName, /\.json$/);
  assert.equal(fileName.includes(':'), false);
});
