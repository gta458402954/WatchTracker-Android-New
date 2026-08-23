import type {
  CollectionMember,
  CollectionMemberTombstone,
  CollectionTombstone,
  WatchCollection,
} from '../types';
import { syncValuesEqual } from './syncMerge.ts';

export interface CollectionSyncState {
  collections: WatchCollection[];
  collectionMembers: CollectionMember[];
  collectionTombstones: CollectionTombstone[];
  collectionMemberTombstones: CollectionMemberTombstone[];
}

type Entity = WatchCollection | CollectionMember;
type Tombstone = CollectionTombstone | CollectionMemberTombstone;
type Choice = 'local' | 'remote';
export type CollectionConflictResolver = (kind: 'collection' | 'collection-member', id: string, local: Entity | null, remote: Entity | null) => Choice;

const SYSTEM_FIELDS = new Set(['id', 'createdAt', 'updatedAt', 'rev', 'revActor']);

function mapOf<T extends { id: string }>(items: T[]) { return new Map(items.map(item => [item.id, item])); }
function entityState<T extends Entity, D extends Tombstone>(id: string, entities: Map<string, T>, tombstones: Map<string, D>) {
  return { entity: entities.get(id) ?? null, tombstone: tombstones.get(id) ?? null };
}
function stateEqual(left: ReturnType<typeof entityState>, right: ReturnType<typeof entityState>) {
  return syncValuesEqual(left, right);
}

function mergeDisjoint<T extends Entity>(base: T, local: T, remote: T): T | null {
  const keys = new Set([...Object.keys(base), ...Object.keys(local), ...Object.keys(remote)]);
  const localChanged = [...keys].filter(key => !SYSTEM_FIELDS.has(key) && !syncValuesEqual((base as unknown as Record<string, unknown>)[key], (local as unknown as Record<string, unknown>)[key]));
  const remoteChanged = [...keys].filter(key => !SYSTEM_FIELDS.has(key) && !syncValuesEqual((base as unknown as Record<string, unknown>)[key], (remote as unknown as Record<string, unknown>)[key]));
  if (localChanged.some(key => remoteChanged.includes(key) && !syncValuesEqual((local as unknown as Record<string, unknown>)[key], (remote as unknown as Record<string, unknown>)[key]))) return null;
  const merged = { ...base } as unknown as Record<string, unknown>;
  for (const key of localChanged) merged[key] = (local as unknown as Record<string, unknown>)[key];
  for (const key of remoteChanged) merged[key] = (remote as unknown as Record<string, unknown>)[key];
  const latest = (local.updatedAt || '').localeCompare(remote.updatedAt || '') >= 0 ? local : remote;
  merged.updatedAt = latest.updatedAt;
  merged.rev = Math.max(local.rev, remote.rev);
  merged.revActor = latest.revActor;
  return merged as unknown as T;
}

function mergeSet<T extends Entity, D extends Tombstone>(
  kind: 'collection' | 'collection-member',
  baseEntities: T[], localEntities: T[], remoteEntities: T[],
  baseTombstones: D[], localTombstones: D[], remoteTombstones: D[],
  resolve: CollectionConflictResolver,
): { entities: T[]; tombstones: D[] } {
  const [be, le, re] = [baseEntities, localEntities, remoteEntities].map(mapOf);
  const [bt, lt, rt] = [baseTombstones, localTombstones, remoteTombstones].map(mapOf);
  const ids = new Set([...be.keys(), ...le.keys(), ...re.keys(), ...bt.keys(), ...lt.keys(), ...rt.keys()]);
  const entities: T[] = []; const tombstones: D[] = [];
  for (const id of [...ids].sort()) {
    const base = entityState(id, be, bt);
    const local = entityState(id, le, lt);
    const remote = entityState(id, re, rt);
    let selected: typeof local;
    if (stateEqual(local, remote)) selected = local;
    else if (stateEqual(local, base)) selected = remote;
    else if (stateEqual(remote, base)) selected = local;
    else if (base.entity && local.entity && remote.entity) {
      const merged = mergeDisjoint(base.entity, local.entity, remote.entity);
      selected = merged ? { entity: merged, tombstone: null }
        : (resolve(kind, id, local.entity, remote.entity) === 'local' ? local : remote);
    } else {
      selected = resolve(kind, id, local.entity, remote.entity) === 'local' ? local : remote;
    }
    if (selected.entity) entities.push(selected.entity);
    else if (selected.tombstone) tombstones.push(selected.tombstone);
  }
  return { entities, tombstones };
}

export function mergeCollectionStates(
  base: CollectionSyncState,
  local: CollectionSyncState,
  remote: CollectionSyncState,
  resolve: CollectionConflictResolver,
): CollectionSyncState {
  const collectionResult = mergeSet(
    'collection', base.collections, local.collections, remote.collections,
    base.collectionTombstones, local.collectionTombstones, remote.collectionTombstones, resolve,
  );
  const memberResult = mergeSet(
    'collection-member', base.collectionMembers, local.collectionMembers, remote.collectionMembers,
    base.collectionMemberTombstones, local.collectionMemberTombstones, remote.collectionMemberTombstones, resolve,
  );
  const collectionIds = new Set(collectionResult.entities.map(item => item.id));
  const members = memberResult.entities.filter(item => collectionIds.has(item.collectionId));
  const validMemberIds = new Set(members.map(item => item.id));
  return {
    collections: collectionResult.entities,
    collectionMembers: members,
    collectionTombstones: collectionResult.tombstones,
    collectionMemberTombstones: memberResult.tombstones.filter(item => !validMemberIds.has(item.id)),
  };
}

export function emptyCollectionState(): CollectionSyncState {
  return { collections: [], collectionMembers: [], collectionTombstones: [], collectionMemberTombstones: [] };
}
