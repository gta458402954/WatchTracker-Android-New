import type { CollectionMember, CollectionMemberTombstone, CollectionTombstone, EpisodeCompletion, WatchCollection, WatchRecord } from '../types';

export interface SyncTombstoneV3 {
  id: string;
  deletedAt: string;
  rev: number;
  revActor: string;
}

export interface SyncPayloadV3 {
  schemaVersion: 3 | 4 | 5 | 6;
  documentId: string;
  revision: number;
  commitId: string;
  parentCommitId: string | null;
  writerId: string;
  committedAt: string;
  records: WatchRecord[];
  tombstones: SyncTombstoneV3[];
  episodeCompletions?: EpisodeCompletion[];
  collections?: WatchCollection[];
  collectionMembers?: CollectionMember[];
  collectionTombstones?: CollectionTombstone[];
  collectionMemberTombstones?: CollectionMemberTombstone[];
}

export type SyncConflictKind = 'edit-edit' | 'delete-edit' | 'locked';

export interface SyncConflictV3 {
  id: string;
  kind: SyncConflictKind;
  fields: string[];
  base: WatchRecord | null;
  local: WatchRecord | null;
  remote: WatchRecord | null;
  localDeleted: boolean;
  remoteDeleted: boolean;
  detectedAt: string;
}

export interface SyncMergeSide {
  records: WatchRecord[];
  tombstones: SyncTombstoneV3[];
}

export interface SyncMergeResult {
  local: SyncMergeSide;
  remote: SyncMergeSide;
  conflicts: SyncConflictV3[];
}

interface Entity {
  record?: WatchRecord;
  tombstone?: SyncTombstoneV3;
}

const SYSTEM_FIELDS = new Set(['id', 'createdAt', 'updatedAt', 'rev', 'revActor']);
const OPTIONAL_DATE_FIELDS = new Set(['startDate', 'endDate']);

function stableValue(value: unknown): unknown {
  if (Array.isArray(value)) return value.map(stableValue);
  if (value && typeof value === 'object') {
    return Object.fromEntries(Object.entries(value as Record<string, unknown>)
      .filter(([, item]) => item !== undefined)
      .sort(([left], [right]) => left.localeCompare(right))
      .map(([key, item]) => [key, stableValue(item)]));
  }
  return value;
}

export function syncValuesEqual(left: unknown, right: unknown): boolean {
  return JSON.stringify(stableValue(left)) === JSON.stringify(stableValue(right));
}

function canonicalEntityArray<T extends { id: string }>(items: T[]): unknown[] {
  return [...items].sort((left, right) => left.id.localeCompare(right.id)).map(stableValue);
}

/**
 * Sync-only semantic comparison. Entity order in a payload is not state: the
 * stable entity id is the identity, while every field (including position and
 * revision fields) remains part of the comparison.
 */
export function syncSideEquivalent(left: SyncMergeSide, right: SyncMergeSide): boolean {
  return syncValuesEqual(canonicalEntityArray(left.records), canonicalEntityArray(right.records))
    && syncValuesEqual(canonicalEntityArray(left.tombstones), canonicalEntityArray(right.tombstones));
}

function normalizedBusinessValue(field: string, value: unknown): unknown {
  if (OPTIONAL_DATE_FIELDS.has(field)
    && (value === null || value === undefined || typeof value === 'string' && !value.trim())) return '';
  return value;
}

function businessFieldEqual(field: string, left: unknown, right: unknown): boolean {
  return syncValuesEqual(normalizedBusinessValue(field, left), normalizedBusinessValue(field, right));
}

function normalizeOptionalRecordDates(record: WatchRecord): WatchRecord {
  return {
    ...record,
    startDate: normalizedBusinessValue('startDate', record.startDate) as string,
    endDate: normalizedBusinessValue('endDate', record.endDate) as string,
  };
}

function mapsOf(side: SyncMergeSide) {
  return {
    records: new Map(side.records.map(record => [record.id, record])),
    tombstones: new Map(side.tombstones.map(tombstone => [tombstone.id, tombstone])),
  };
}

function entityOf(
  id: string,
  side: ReturnType<typeof mapsOf>,
  base?: Entity,
): Entity {
  const record = side.records.get(id);
  if (record) return { record };
  const tombstone = side.tombstones.get(id);
  if (tombstone) return { tombstone };
  return base ?? {};
}

function entityEqual(left: Entity, right: Entity): boolean {
  return syncValuesEqual(left.record, right.record)
    && syncValuesEqual(left.tombstone, right.tombstone);
}

function changedBusinessFields(base: WatchRecord, current: WatchRecord): Set<string> {
  const keys = new Set([...Object.keys(base), ...Object.keys(current)]);
  return new Set([...keys].filter(key => !SYSTEM_FIELDS.has(key)
    && !businessFieldEqual(
      key,
      (base as unknown as Record<string, unknown>)[key],
      (current as unknown as Record<string, unknown>)[key],
    )));
}

function businessRecordsEqual(left: WatchRecord, right: WatchRecord): boolean {
  const keys = new Set([...Object.keys(left), ...Object.keys(right)]);
  return [...keys].filter(key => !SYSTEM_FIELDS.has(key)).every(key => businessFieldEqual(
    key,
    (left as unknown as Record<string, unknown>)[key],
    (right as unknown as Record<string, unknown>)[key],
  ));
}

function preferredEquivalentRecord(local: WatchRecord, remote: WatchRecord): WatchRecord {
  const preferred = local.isLocked || (local.rev ?? 0) > (remote.rev ?? 0)
    ? local
    : (remote.rev ?? 0) > (local.rev ?? 0)
      ? remote
      : (local.updatedAt ?? '').localeCompare(remote.updatedAt ?? '') >= 0 ? local : remote;
  return normalizeOptionalRecordDates(preferred);
}

function mergeRecords(
  base: WatchRecord,
  local: WatchRecord,
  remote: WatchRecord,
  deviceId: string,
  now: string,
): { record?: WatchRecord; conflictingFields: string[] } {
  const localFields = changedBusinessFields(base, local);
  const remoteFields = changedBusinessFields(base, remote);
  const conflicts = [...localFields].filter(field => remoteFields.has(field)
    && !syncValuesEqual(
      (local as unknown as Record<string, unknown>)[field],
      (remote as unknown as Record<string, unknown>)[field],
    )).sort();
  if (conflicts.length) return { conflictingFields: conflicts };

  const merged = { ...base } as unknown as Record<string, unknown>;
  for (const field of localFields) merged[field] = (local as unknown as Record<string, unknown>)[field];
  for (const field of remoteFields) merged[field] = (remote as unknown as Record<string, unknown>)[field];
  merged.id = base.id;
  merged.createdAt = base.createdAt || local.createdAt || remote.createdAt;
  merged.updatedAt = now;
  merged.rev = Math.max(base.rev ?? 0, local.rev ?? 0, remote.rev ?? 0) + 1;
  merged.revActor = deviceId;
  return { record: merged as unknown as WatchRecord, conflictingFields: [] };
}

function deletionOf(id: string, entities: Entity[], deviceId: string, now: string): SyncTombstoneV3 {
  const revisions = entities.flatMap(entity => [entity.record?.rev ?? 0, entity.tombstone?.rev ?? 0]);
  return { id, deletedAt: now, rev: Math.max(...revisions) + 1, revActor: deviceId };
}

function conflictOf(
  id: string,
  kind: SyncConflictKind,
  fields: string[],
  base: Entity,
  local: Entity,
  remote: Entity,
  now: string,
): SyncConflictV3 {
  return {
    id,
    kind,
    fields,
    base: base.record ?? null,
    local: local.record ?? null,
    remote: remote.record ?? null,
    localDeleted: Boolean(local.tombstone),
    remoteDeleted: Boolean(remote.tombstone),
    detectedAt: now,
  };
}

function pushEntity(side: SyncMergeSide, entity: Entity) {
  if (entity.record) side.records.push(entity.record);
  else if (entity.tombstone) side.tombstones.push(entity.tombstone);
}

/**
 * Three-way merge using the last locally committed remote state as base.
 * Conflicted IDs are deliberately split: local stays local and remote stays remote.
 */
export function mergeSyncStates(
  baseSide: SyncMergeSide,
  localSide: SyncMergeSide,
  remoteSide: SyncMergeSide,
  deviceId: string,
  now = new Date().toISOString(),
  frozenConflicts: SyncConflictV3[] = [],
): SyncMergeResult {
  const baseMaps = mapsOf(baseSide);
  const localMaps = mapsOf(localSide);
  const remoteMaps = mapsOf(remoteSide);
  const ids = new Set([
    ...baseMaps.records.keys(), ...baseMaps.tombstones.keys(),
    ...localMaps.records.keys(), ...localMaps.tombstones.keys(),
    ...remoteMaps.records.keys(), ...remoteMaps.tombstones.keys(),
  ]);
  const result: SyncMergeResult = {
    local: { records: [], tombstones: [] },
    remote: { records: [], tombstones: [] },
    conflicts: [],
  };
  const frozenById = new Map(frozenConflicts.map(conflict => [conflict.id, conflict]));

  for (const id of [...ids].sort()) {
    const base = entityOf(id, baseMaps);
    const local = entityOf(id, localMaps, base);
    const remote = entityOf(id, remoteMaps, base);
    const frozen = frozenById.get(id);
    if (local.record && remote.record && businessRecordsEqual(local.record, remote.record)) {
      const selected = preferredEquivalentRecord(local.record, remote.record);
      pushEntity(result.local, { record: selected });
      pushEntity(result.remote, { record: selected });
      continue;
    }
    if (frozen) {
      pushEntity(result.local, local);
      pushEntity(result.remote, remote);
      result.conflicts.push({
        ...frozen,
        local: local.record ?? null,
        remote: remote.record ?? null,
        localDeleted: Boolean(local.tombstone),
        remoteDeleted: Boolean(remote.tombstone),
      });
      continue;
    }
    const localChanged = !entityEqual(local, base);
    const remoteChanged = !entityEqual(remote, base);

    if (entityEqual(local, remote)) {
      pushEntity(result.local, local);
      pushEntity(result.remote, local);
      continue;
    }
    if (!localChanged) {
      pushEntity(result.local, remote);
      pushEntity(result.remote, remote);
      continue;
    }
    if (!remoteChanged) {
      pushEntity(result.local, local);
      pushEntity(result.remote, local);
      continue;
    }

    if (local.record && remote.record && base.record && !local.record.isLocked) {
      const merged = mergeRecords(base.record, local.record, remote.record, deviceId, now);
      if (merged.record) {
        pushEntity(result.local, { record: merged.record });
        pushEntity(result.remote, { record: merged.record });
        continue;
      }
      result.conflicts.push(conflictOf(id, 'edit-edit', merged.conflictingFields, base, local, remote, now));
    } else if (local.record?.isLocked) {
      result.conflicts.push(conflictOf(id, 'locked', [], base, local, remote, now));
    } else if (local.record && remote.record) {
      result.conflicts.push(conflictOf(id, 'edit-edit', ['record'], base, local, remote, now));
    } else if ((local.tombstone && remote.record) || (local.record && remote.tombstone)) {
      result.conflicts.push(conflictOf(id, 'delete-edit', [], base, local, remote, now));
    } else {
      const deletion = deletionOf(id, [base, local, remote], deviceId, now);
      pushEntity(result.local, { tombstone: deletion });
      pushEntity(result.remote, { tombstone: deletion });
      continue;
    }

    pushEntity(result.local, local);
    pushEntity(result.remote, remote);
  }
  return result;
}

export function emptySyncPayload(deviceId: string, now = new Date().toISOString()): SyncPayloadV3 {
  return {
    schemaVersion: 3,
    documentId: crypto.randomUUID(),
    revision: 0,
    commitId: '',
    parentCommitId: null,
    writerId: deviceId,
    committedAt: now,
    records: [],
    tombstones: [],
    episodeCompletions: [],
  };
}

export function parseSyncPayloadV3(value: unknown): SyncPayloadV3 {
  if (!value || typeof value !== 'object') throw new Error('invalid_remote_payload');
  const payload = value as Partial<SyncPayloadV3> & { schemaVersion?: number };
  if (typeof payload.schemaVersion === 'number' && payload.schemaVersion > 6) {
    throw new Error('unsupported_remote_schema');
  }
  const validTombstones = Array.isArray(payload.tombstones) && payload.tombstones.every(item => item
    && typeof item.id === 'string' && typeof item.deletedAt === 'string'
    && Number.isInteger(item.rev) && item.rev >= 0 && typeof item.revActor === 'string');
  const validCompletions = payload.schemaVersion === 3 || (Array.isArray(payload.episodeCompletions)
    && payload.episodeCompletions.every(item => item && typeof item.id === 'string'
      && typeof item.recordId === 'string' && Number.isInteger(item.episodeNumber) && item.episodeNumber > 0
      && (item.completedAt === null || typeof item.completedAt === 'string')
      && typeof item.createdAt === 'string' && typeof item.updatedAt === 'string'
      && Number.isInteger(item.rev) && item.rev >= 0 && typeof item.revActor === 'string'));
  const validCollections = (payload.schemaVersion ?? 0) < 5 || (
    Array.isArray(payload.collections) && Array.isArray(payload.collectionMembers)
    && Array.isArray(payload.collectionTombstones) && Array.isArray(payload.collectionMemberTombstones)
    && payload.collections.every(item => item && typeof item.id === 'string' && typeof item.name === 'string' && Number.isInteger(item.rev)
      && (payload.schemaVersion === 5 || (['manual', 'tv-series', 'movie-series', 'universe'].includes(item.collectionKind) && ['manual', 'chronological'].includes(item.orderMode))))
    && payload.collectionMembers.every(item => item && typeof item.id === 'string' && typeof item.collectionId === 'string' && typeof item.recordId === 'string' && Number.isInteger(item.position) && Number.isInteger(item.rev))
    && payload.collectionTombstones.every(item => item && typeof item.id === 'string' && typeof item.deletedAt === 'string' && Number.isInteger(item.rev))
    && payload.collectionMemberTombstones.every(item => item && typeof item.id === 'string' && typeof item.collectionId === 'string' && typeof item.recordId === 'string' && typeof item.deletedAt === 'string' && Number.isInteger(item.rev))
  );
  if (![3, 4, 5, 6].includes(payload.schemaVersion ?? -1) || !Array.isArray(payload.records) || !validTombstones || !validCompletions || !validCollections
    || payload.records.some(record => !record || typeof record.id !== 'string')
    || typeof payload.documentId !== 'string' || !payload.documentId
    || !Number.isInteger(payload.revision) || (payload.revision ?? -1) < 0
    || typeof payload.commitId !== 'string' || typeof payload.writerId !== 'string'
    || typeof payload.committedAt !== 'string'
    || !(payload.parentCommitId === null || typeof payload.parentCommitId === 'string')) {
    throw new Error('invalid_remote_payload');
  }
  return {
    ...(payload as SyncPayloadV3),
    episodeCompletions: (payload.schemaVersion ?? 0) >= 4 ? payload.episodeCompletions : [],
    collections: (payload.schemaVersion ?? 0) >= 5 ? payload.collections?.map(item => ({
      ...item,
      collectionKind: item.collectionKind ?? (item.sourceKind === 'tmdb-tv-show' ? 'tv-series' : item.sourceKind === 'tmdb-movie-collection' ? 'movie-series' : 'manual'),
      orderMode: item.orderMode ?? (item.sourceKind === 'manual' ? 'manual' : 'chronological'),
    })) : [],
    collectionMembers: (payload.schemaVersion ?? 0) >= 5 ? payload.collectionMembers : [],
    collectionTombstones: (payload.schemaVersion ?? 0) >= 5 ? payload.collectionTombstones : [],
    collectionMemberTombstones: (payload.schemaVersion ?? 0) >= 5 ? payload.collectionMemberTombstones : [],
  };
}

function completionKey(item: EpisodeCompletion): string {
  return `${item.recordId}\0${item.episodeNumber}`;
}

/** Episode identity follows the same record/episode key used by the merge. */
export function episodeCompletionStateEquivalent(left: EpisodeCompletion[], right: EpisodeCompletion[]): boolean {
  return syncValuesEqual(
    [...left].sort((a, b) => completionKey(a).localeCompare(completionKey(b))).map(stableValue),
    [...right].sort((a, b) => completionKey(a).localeCompare(completionKey(b))).map(stableValue),
  );
}

/** Monotonic three-way merge for per-episode completion facts. */
export function mergeEpisodeCompletions(
  base: EpisodeCompletion[],
  local: EpisodeCompletion[],
  remote: EpisodeCompletion[],
  resolveConflict?: (local: EpisodeCompletion, remote: EpisodeCompletion) => 'local' | 'remote',
): EpisodeCompletion[] {
  const maps = [base, local, remote].map(items => new Map(items.map(item => [completionKey(item), item])));
  const keys = new Set(maps.flatMap(map => [...map.keys()]));
  const result: EpisodeCompletion[] = [];
  for (const key of [...keys].sort()) {
    const [baseItem, localItem, remoteItem] = maps.map(map => map.get(key));
    if (syncValuesEqual(localItem, remoteItem)) {
      if (localItem) result.push(localItem);
      continue;
    }
    if (syncValuesEqual(localItem, baseItem)) {
      if (remoteItem) result.push(remoteItem);
      continue;
    }
    if (syncValuesEqual(remoteItem, baseItem)) {
      if (localItem) result.push(localItem);
      continue;
    }
    if (localItem && remoteItem) {
      if (localItem.completedAt === null && remoteItem.completedAt !== null) {
        result.push(remoteItem);
        continue;
      }
      if (remoteItem.completedAt === null && localItem.completedAt !== null) {
        result.push(localItem);
        continue;
      }
      if (localItem.completedAt === remoteItem.completedAt) {
        result.push(localItem.rev >= remoteItem.rev ? localItem : remoteItem);
        continue;
      }
      if (resolveConflict) {
        result.push(resolveConflict(localItem, remoteItem) === 'local' ? localItem : remoteItem);
        continue;
      }
      throw new Error(`episode_completion_conflict:${key}`);
    }
    result.push((localItem ?? remoteItem) as EpisodeCompletion);
  }
  return result;
}
