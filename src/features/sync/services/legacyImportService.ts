import type { WatchRecord } from '../../../shared/types';
import {
  commitSyncResult,
  getSettingAsync,
  getSyncSnapshot,
} from '../../../shared/lib/database.ts';
import { parseSyncPayloadV3, syncValuesEqual, type SyncConflictV3 } from '../../../shared/lib/syncMerge.ts';
import { legacyPayload } from '../domain/syncPayload.ts';
import { syncError } from '../domain/syncErrors.ts';
import type { WebDAVCreds, WebDavTransport } from '../infrastructure/webdavTransport.ts';
import { webdavTransport } from '../infrastructure/webdavTransport.ts';
import { conditionalValidatorForResource, contentFingerprint } from '../infrastructure/conditionalWebdav.ts';
import type { SyncResult } from './syncContracts.ts';

const V3_RESOURCE = 'records-v3.json';
const LEGACY_RESOURCE = 'records.json';

export interface SyncTargetProbe { kind: 'empty' | 'v3' | 'legacy'; recordCount: number; revision: number | null; }

export type LegacyImportDatabase = Pick<typeof import('../../../shared/lib/database.ts'),
  'commitSyncResult' | 'getSettingAsync' | 'getSyncSnapshot'>;

export interface LegacyImportDependencies {
  transport: WebDavTransport;
  database: LegacyImportDatabase;
  now: () => Date;
}

const defaultDependencies: LegacyImportDependencies = {
  transport: webdavTransport,
  database: { commitSyncResult, getSettingAsync, getSyncSnapshot },
  now: () => new Date(),
};

async function probeWithDependencies(creds: WebDAVCreds, deps: LegacyImportDependencies): Promise<SyncTargetProbe> {
  const proxy = await deps.database.getSettingAsync('network_proxy');
  const v3 = await deps.transport.request('GET', creds, proxy, V3_RESOURCE);
  if (v3.status === 200) {
    const payload = parseSyncPayloadV3(v3.body);
    return { kind: 'v3', recordCount: payload.records.length, revision: payload.revision };
  }
  if (v3.status !== 404) throw new Error(`HTTP Error: ${v3.status}`);
  const legacy = await deps.transport.request('GET', creds, proxy, LEGACY_RESOURCE);
  if (legacy.status === 200) {
    const payload = legacyPayload(legacy.body);
    return { kind: 'legacy', recordCount: payload.records.length, revision: null };
  }
  if (legacy.status === 404) return { kind: 'empty', recordCount: 0, revision: null };
  throw new Error(`HTTP Error: ${legacy.status}`);
}

export function createLegacyImportService(dependencies: Partial<LegacyImportDependencies> = {}) {
  const deps = { ...defaultDependencies, ...dependencies };
  return {
    probeSyncTarget: (creds: WebDAVCreds) => probeWithDependencies(creds, deps),
    loadFromWebDAV: (creds: WebDAVCreds) => loadWithDependencies(creds, deps),
    importLegacyChangesToConflictCenter: (creds: WebDAVCreds) => importWithDependencies(creds, deps),
    getSyncConflicts: async () => (await deps.database.getSyncSnapshot()).conflicts,
    clearResolvedSyncConflicts: async (_records: WatchRecord[]) => {
      void _records;
      return (await deps.database.getSyncSnapshot()).conflicts;
    },
  };
}

async function loadWithDependencies(creds: WebDAVCreds, deps: LegacyImportDependencies): Promise<{ ok: boolean; data?: WatchRecord[]; error?: string }> {
  const proxy = await deps.database.getSettingAsync('network_proxy');
  try {
    const v3 = await deps.transport.request('GET', creds, proxy, V3_RESOURCE);
    if (v3.status === 200) return { ok: true, data: parseSyncPayloadV3(v3.body).records };
    if (v3.status !== 404) return { ok: false, error: `HTTP Error: ${v3.status}` };
    const legacy = await deps.transport.request('GET', creds, proxy, LEGACY_RESOURCE);
    if (legacy.status === 200) return { ok: true, data: legacyPayload(legacy.body).records };
    return { ok: false, error: legacy.status === 404 ? '云端暂无数据' : `HTTP Error: ${legacy.status}` };
  } catch (error) { return { ok: false, error: String(error) }; }
}

async function importWithDependencies(creds: WebDAVCreds, deps: LegacyImportDependencies): Promise<SyncResult> {
  const proxy = await deps.database.getSettingAsync('network_proxy');
  try {
    const snapshot = await deps.database.getSyncSnapshot();
    const v3Response = await deps.transport.request('GET', creds, proxy, V3_RESOURCE);
    if (v3Response.status !== 200) throw new Error(`HTTP Error: ${v3Response.status}`);
    const v3Etag = (await conditionalValidatorForResource(v3Response, creds, proxy, V3_RESOURCE, deps.transport)).etag;
    const v3 = parseSyncPayloadV3(v3Response.body);
    const legacyResponse = await deps.transport.request('GET', creds, proxy, LEGACY_RESOURCE);
    if (legacyResponse.status !== 200) throw new Error(`HTTP Error: ${legacyResponse.status}`);
    const legacy = legacyPayload(legacyResponse.body);
    const fingerprint = legacyResponse.etag || await contentFingerprint(legacyResponse.body);
    const localRecords = new Map(snapshot.records.map(record => [record.id, record]));
    const localTombstones = new Set(snapshot.tombstones.map(item => item.id));
    const baseRecords = new Map((snapshot.baseline?.records ?? []).map(record => [record.id, record]));
    const v3Records = new Map(v3.records.map(record => [record.id, record]));
    const legacyTombstones = new Set(legacy.tombstones.map(item => item.id));
    const imported: SyncConflictV3[] = [];
    const detectedAt = deps.now().toISOString();
    for (const legacyRecord of legacy.records) {
      const currentRemote = v3Records.get(legacyRecord.id);
      if (currentRemote && syncValuesEqual(currentRemote, legacyRecord)) continue;
      const local = localRecords.get(legacyRecord.id) ?? null;
      imported.push({
        id: legacyRecord.id,
        kind: local?.isLocked ? 'locked' : 'edit-edit',
        fields: ['legacy-import'],
        base: baseRecords.get(legacyRecord.id) ?? null,
        local,
        remote: legacyRecord,
        localDeleted: localTombstones.has(legacyRecord.id),
        remoteDeleted: false,
        detectedAt,
      });
    }
    for (const id of legacyTombstones) {
      const local = localRecords.get(id) ?? null;
      if (!local && !v3Records.has(id)) continue;
      imported.push({
        id,
        kind: local?.isLocked ? 'locked' : 'delete-edit',
        fields: [],
        base: baseRecords.get(id) ?? null,
        local,
        remote: null,
        localDeleted: localTombstones.has(id),
        remoteDeleted: true,
        detectedAt,
      });
    }
    const byId = new Map(snapshot.conflicts.map(conflict => [conflict.id, conflict]));
    for (const conflict of imported) byId.set(conflict.id, conflict);
    const conflicts = [...byId.values()];
    await deps.database.commitSyncResult({
      targetId: snapshot.targetId,
      targetEpoch: snapshot.targetEpoch,
      expectedGeneration: snapshot.recordsGeneration,
      records: snapshot.records,
      tombstones: snapshot.tombstones,
      episodeCompletions: snapshot.episodeCompletions,
      collections: snapshot.collections,
      collectionMembers: snapshot.collectionMembers,
      collectionTombstones: snapshot.collectionTombstones,
      collectionMemberTombstones: snapshot.collectionMemberTombstones,
      baseline: v3,
      conflicts,
      remoteEtag: v3Etag,
      lastCommit: { revision: v3.revision, commitId: v3.commitId, committedAt: v3.committedAt },
      v2SourceFingerprint: fingerprint,
      acknowledgeOutbox: false,
    });
    return { ok: true, records: snapshot.records, conflicts, conflictCount: imported.length };
  } catch (error) { return syncError(error); }
}

export async function probeSyncTarget(creds: WebDAVCreds, dependencies?: Partial<LegacyImportDependencies>) {
  return probeWithDependencies(creds, { ...defaultDependencies, ...dependencies });
}

export async function loadFromWebDAV(creds: WebDAVCreds, dependencies?: Partial<LegacyImportDependencies>) {
  return loadWithDependencies(creds, { ...defaultDependencies, ...dependencies });
}

export async function importLegacyChangesToConflictCenter(creds: WebDAVCreds, dependencies?: Partial<LegacyImportDependencies>) {
  return importWithDependencies(creds, { ...defaultDependencies, ...dependencies });
}
