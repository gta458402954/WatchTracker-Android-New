import assert from 'node:assert/strict';
import { describe, test } from 'node:test';
import { collectionStateEquivalent, emptyCollectionState, mergeCollectionStates } from '../collectionSync.ts';
import { parseSyncPayloadV3 } from '../syncMerge.ts';

const collection = (fields = {}) => ({
  id: 'c1', name: '系列', normalizedName: '系列', description: null, sourceKind: 'manual', sourceKey: null,
  collectionKind: 'manual', orderMode: 'manual', createdAt: '2026-01-01T00:00:00Z', updatedAt: '2026-01-01T00:00:00Z', rev: 1, revActor: 'base', ...fields,
});
const member = (fields = {}) => ({
  id: 'm1', collectionId: 'c1', recordId: 'r1', position: 0, sourceKind: 'manual',
  createdAt: '2026-01-01T00:00:00Z', updatedAt: '2026-01-01T00:00:00Z', rev: 1, revActor: 'base', ...fields,
});
const state = (collections = [], collectionMembers = [], collectionTombstones = [], collectionMemberTombstones = []) => ({ collections, collectionMembers, collectionTombstones, collectionMemberTombstones });

describe('TASK-D-UX-003 collection synchronization', () => {
  test('collection equivalence ignores entity order but keeps member position semantic', () => {
    const first = member({ id: 'm1', position: 0 });
    const second = member({ id: 'm2', position: 1 });
    assert.equal(collectionStateEquivalent(state([collection()], [first, second]), state([collection()], [second, first])), true);
    assert.equal(collectionStateEquivalent(state([collection()], [first, second]), state([collection()], [second, { ...first, position: 2 }])), false);
  });
  test('merges disjoint collection fields and stable memberships', () => {
    const base = state([collection()], [member()]);
    const local = state([collection({ name: '本机名称', normalizedName: '本机名称' })], [member()]);
    const remote = state([collection({ description: '云端说明' })], [member()]);
    const result = mergeCollectionStates(base, local, remote, () => 'local');
    assert.equal(result.collections[0].name, '本机名称');
    assert.equal(result.collections[0].description, '云端说明');
    assert.equal(result.collectionMembers.length, 1);
  });

  test('same member order conflict uses explicit resolver', () => {
    const base = state([collection()], [member()]);
    const local = state([collection()], [member({ position: 1024 })]);
    const remote = state([collection()], [member({ position: 2048 })]);
    const result = mergeCollectionStates(base, local, remote, kind => kind === 'collection-member' ? 'remote' : 'local');
    assert.equal(result.collectionMembers[0].position, 2048);
  });

  test('V5 parser requires complete collection arrays and older payloads read empty', () => {
    const base = { documentId: 'd', revision: 1, commitId: 'c', parentCommitId: null, writerId: 'w', committedAt: '2026-01-01T00:00:00Z', records: [], tombstones: [] };
    const v4 = parseSyncPayloadV3({ ...base, schemaVersion: 4, episodeCompletions: [] });
    assert.deepEqual(v4.collections, []);
    assert.throws(() => parseSyncPayloadV3({ ...base, schemaVersion: 5, episodeCompletions: [] }), /invalid_remote_payload/);
    const v5 = parseSyncPayloadV3({ ...base, schemaVersion: 5, episodeCompletions: [], ...emptyCollectionState() });
    assert.equal(v5.schemaVersion, 5);
  });

  test('V6 carries collection kind and order mode while V5 receives safe defaults', () => {
    const base = { documentId: 'd', revision: 1, commitId: 'c', parentCommitId: null, writerId: 'w', committedAt: '2026-01-01T00:00:00Z', records: [], tombstones: [], episodeCompletions: [], collectionMembers: [], collectionTombstones: [], collectionMemberTombstones: [] };
    const legacy = { ...collection() };
    delete legacy.collectionKind; delete legacy.orderMode;
    const v5 = parseSyncPayloadV3({ ...base, schemaVersion: 5, collections: [legacy] });
    assert.equal(v5.collections[0].collectionKind, 'manual');
    assert.equal(v5.collections[0].orderMode, 'manual');
    const v6 = parseSyncPayloadV3({ ...base, schemaVersion: 6, collections: [collection({ collectionKind: 'universe', orderMode: 'chronological' })] });
    assert.equal(v6.collections[0].collectionKind, 'universe');
    assert.throws(() => parseSyncPayloadV3({ ...base, schemaVersion: 6, collections: [legacy] }), /invalid_remote_payload/);
  });
});
