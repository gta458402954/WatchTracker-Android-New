import type { EpisodeCompletion, WatchCollection, CollectionMember, CollectionTombstone, CollectionMemberTombstone, WatchRecord } from '../../../shared/types';
import {
  emptySyncPayload,
  type SyncMergeSide,
  type SyncPayloadV3,
  type SyncTombstoneV3,
} from '../../../shared/lib/syncMerge.ts';
import { emptyCollectionState, type CollectionSyncState } from '../../../shared/lib/collectionSync.ts';

export interface LegacyTombstone { id: string; deletedAt: string; }
export interface LegacyPayload { schemaVersion: 2; updatedAt: string; records: WatchRecord[]; tombstones: LegacyTombstone[]; }

/** Parses the legacy array/object resource while rejecting future formats. */
export function legacyPayload(data: unknown): LegacyPayload {
  if (Array.isArray(data)) return { schemaVersion: 2, updatedAt: '', records: data as WatchRecord[], tombstones: [] };
  if (!data || typeof data !== 'object') throw new Error('invalid_remote_payload');
  const value = data as Omit<Partial<LegacyPayload>, 'schemaVersion'> & { schemaVersion?: number };
  if (typeof value.schemaVersion === 'number' && value.schemaVersion > 3) throw new Error('unsupported_remote_schema');
  if (value.schemaVersion === 3) throw new Error('unexpected_v3_legacy_resource');
  if (!Array.isArray(value.records)) throw new Error('invalid_remote_payload');
  return {
    schemaVersion: 2,
    updatedAt: typeof value.updatedAt === 'string' ? value.updatedAt : '',
    records: value.records,
    tombstones: Array.isArray(value.tombstones) ? value.tombstones : [],
  };
}

export const parseLegacyPayload = legacyPayload;

export function sideOfPayload(payload: Pick<SyncPayloadV3, 'records' | 'tombstones'>): SyncMergeSide {
  return { records: payload.records, tombstones: payload.tombstones };
}

export function collectionSideOfPayload(payload: SyncPayloadV3): CollectionSyncState {
  return {
    collections: payload.collections ?? [],
    collectionMembers: payload.collectionMembers ?? [],
    collectionTombstones: payload.collectionTombstones ?? [],
    collectionMemberTombstones: payload.collectionMemberTombstones ?? [],
  };
}

export function sideOfLegacy(payload: LegacyPayload): SyncMergeSide {
  return {
    records: payload.records,
    tombstones: payload.tombstones.map((item): SyncTombstoneV3 => ({
      ...item,
      rev: 0,
      revActor: 'legacy-v2',
    })),
  };
}

/** Builds the smallest compatible V3-V6 envelope for a merged sync side. */
export function buildSyncPayload(
  current: SyncPayloadV3,
  remote: SyncMergeSide,
  episodeCompletions: EpisodeCompletion[],
  collectionState: CollectionSyncState,
  deviceId: string,
  now: string,
  commitId: string,
): SyncPayloadV3 {
  const useV6 = current.schemaVersion === 6 || collectionState.collections.length > 0
    || remote.records.some(record => record.tmdbId != null || record.tmdbParentId != null || record.tmdbSeasonNumber != null || record.seriesRecordKind != null);
  const useV5 = current.schemaVersion === 5
    || collectionState.collectionMembers.length > 0 || collectionState.collectionTombstones.length > 0
    || collectionState.collectionMemberTombstones.length > 0;
  const useV4 = current.schemaVersion === 4 || episodeCompletions.length > 0
    || remote.records.some(record => record.episodeTrackingEnabled);
  return {
    schemaVersion: useV6 ? 6 : useV5 ? 5 : useV4 ? 4 : 3,
    documentId: current.documentId,
    revision: current.revision + 1,
    commitId,
    parentCommitId: current.commitId || null,
    writerId: deviceId,
    committedAt: now,
    records: remote.records,
    tombstones: remote.tombstones,
    ...(useV4 || useV5 || useV6 ? { episodeCompletions } : {}),
    ...(useV5 || useV6 ? { ...collectionState } : {}),
  };
}

export { emptySyncPayload, emptyCollectionState };
export const payloadForCommit = buildSyncPayload;
export type { CollectionSyncState, SyncMergeSide, SyncPayloadV3, SyncTombstoneV3 };
// Keep these imports visible to type-aware consumers of this domain boundary.
export type { WatchCollection, CollectionMember, CollectionTombstone, CollectionMemberTombstone };
