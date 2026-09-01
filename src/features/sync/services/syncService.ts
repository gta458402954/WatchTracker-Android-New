import type { WatchRecord } from '../../../shared/types';
import {
  commitSyncResult,
  getSettingAsync,
  getSyncSnapshot,
  prepareSyncPublishIntent,
  recordSyncRemoteUnchanged,
  setSettingAsync,
  type SyncSnapshot,
} from '../../../shared/lib/database.ts';
import {
  emptySyncPayload,
  mergeEpisodeCompletions,
  mergeSyncStates,
  parseSyncPayloadV3,
  episodeCompletionStateEquivalent,
  syncSideEquivalent,
  syncValuesEqual,
  type SyncMergeSide,
  type SyncPayloadV3,
} from '../../../shared/lib/syncMerge.ts';
import { collectionStateEquivalent, emptyCollectionState, mergeCollectionStates, type CollectionSyncState } from '../../../shared/lib/collectionSync.ts';
import { entityTag, normalizedEntityTag, strongEtag } from '../domain/entityTags.ts';
import { buildSyncPayload, collectionSideOfPayload, legacyPayload, sideOfLegacy, sideOfPayload } from '../domain/syncPayload.ts';
import { syncError } from '../domain/syncErrors.ts';
import { assertEntityTag, conditionalValidatorForResource, contentFingerprint, probeDavEntityTagForResource, type ConditionalValidator } from '../infrastructure/conditionalWebdav.ts';
import type { WebDAVCreds, WebDavTransport } from '../infrastructure/webdavTransport.ts';
import { webdavTransport } from '../infrastructure/webdavTransport.ts';
import type { SyncResult } from './syncContracts.ts';

const V3_RESOURCE = 'records-v3.json';
const LEGACY_RESOURCE = 'records.json';
const RANGE_PROBE = 'bytes=0-0';
const MAX_PRECONDITION_RETRIES = 3;

export type SyncServiceDatabase = Pick<typeof import('../../../shared/lib/database.ts'),
  'commitSyncResult' | 'getSettingAsync' | 'getSyncSnapshot' | 'prepareSyncPublishIntent' | 'recordSyncRemoteUnchanged' | 'setSettingAsync'>;

export interface SyncServiceDependencies {
  transport: WebDavTransport;
  database: SyncServiceDatabase;
  now: () => Date;
  uuid: () => string;
  confirm: (message: string) => boolean;
}

const defaultDependencies: SyncServiceDependencies = {
  transport: webdavTransport,
  database: { commitSyncResult, getSettingAsync, getSyncSnapshot, prepareSyncPublishIntent, recordSyncRemoteUnchanged, setSettingAsync },
  now: () => new Date(),
  uuid: () => crypto.randomUUID(),
  confirm: () => false,
};

function successful(status: number) { return status >= 200 && status < 300; }

/**
 * A ranged response is metadata only. It is never parsed as a sync payload
 * and its validator is deliberately kept separate from PUT validation.
 */
function rangeProbeEtag(response: Awaited<ReturnType<WebDavTransport['request']>>): string | null {
  if (response.status !== 206 || response.rangeBodyLength !== 1) return null;
  const contentRange = response.contentRange?.trim() ?? '';
  const completeLength = /^bytes 0-0\/([0-9]+)$/.exec(contentRange)?.[1];
  if (!completeLength || !/[1-9]/.test(completeLength)) return null;
  return normalizedEntityTag(response.etag);
}

function confirmUpgrade(deps: SyncServiceDependencies, message: string): boolean {
  return deps.confirm(message);
}

export function conditionalPullEtag(snapshot: Pick<SyncSnapshot,
  'baseline' | 'remoteEtag' | 'outbox' | 'publishIntent' | 'staging'>): string | null {
  if (!snapshot.baseline || snapshot.outbox.pending || snapshot.publishIntent
    || snapshot.staging.entries.length > 0) return null;
  const stored = snapshot.remoteEtag;
  return entityTag(stored) ? stored : null;
}

async function checkLegacyRemote(
  deps: SyncServiceDependencies,
  creds: WebDAVCreds,
  proxy: string | null,
  previousFingerprint: string | null,
): Promise<string | null> {
  const legacyResponse = await deps.transport.request('GET', creds, proxy, LEGACY_RESOURCE);
  if (legacyResponse.status === 200) {
    const currentFingerprint = legacyResponse.etag || await contentFingerprint(legacyResponse.body);
    if (previousFingerprint && previousFingerprint !== currentFingerprint) {
      throw new Error('legacy_remote_changed');
    }
    return currentFingerprint;
  }
  if (legacyResponse.status === 404) return 'missing';
  return previousFingerprint;
}

async function finishRemoteUnchanged(
  snapshot: SyncSnapshot,
  deps: SyncServiceDependencies,
  creds: WebDAVCreds,
  proxy: string | null,
  storedConditionalEtag: string,
): Promise<SyncResult> {
  const legacyFingerprint = await checkLegacyRemote(
    deps, creds, proxy, snapshot.v2SourceFingerprint,
  );
  await deps.database.recordSyncRemoteUnchanged({
    targetId: snapshot.targetId,
    targetEpoch: snapshot.targetEpoch,
    expectedGeneration: snapshot.recordsGeneration,
    expectedRemoteEtag: storedConditionalEtag,
    v2SourceFingerprint: legacyFingerprint,
  });
  return {
    ok: true,
    records: snapshot.records,
    conflicts: snapshot.conflicts,
    conflictCount: snapshot.conflicts.length,
    legacyImported: false,
  };
}

async function syncWithDependencies(
  creds: WebDAVCreds,
  _ignoredRecords: WatchRecord[] | undefined,
  deps: SyncServiceDependencies,
): Promise<SyncResult> {
  void _ignoredRecords;
  const proxy = await deps.database.getSettingAsync('network_proxy');
  try {
    const folder = await deps.transport.request('MKCOL', creds, proxy, V3_RESOURCE);
    if (!successful(folder.status) && folder.status !== 405) throw new Error(`HTTP Error: ${folder.status}`);
    const snapshot = await deps.database.getSyncSnapshot();
    if (snapshot.targetId && (creds.targetId !== snapshot.targetId || creds.targetEpoch !== snapshot.targetEpoch)) {
      throw new Error('stale_sync_target');
    }
    const localSide: SyncMergeSide = { records: snapshot.records, tombstones: snapshot.tombstones };
    const localCollectionState: CollectionSyncState = {
      collections: snapshot.collections,
      collectionMembers: snapshot.collectionMembers,
      collectionTombstones: snapshot.collectionTombstones,
      collectionMemberTombstones: snapshot.collectionMemberTombstones,
    };
    const rejectedValidatorFingerprints: string[] = [];
    const storedConditionalEtag = conditionalPullEtag(snapshot);

    for (let attempt = 0; attempt < MAX_PRECONDITION_RETRIES; attempt++) {
      let useConditionalGetFallback = false;
      if (storedConditionalEtag) {
        const davEtag = await probeDavEntityTagForResource(
          creds, proxy, V3_RESOURCE, deps.transport,
        );
        if (davEtag === storedConditionalEtag) {
          console.info('[sync] clean preflight: PROPFIND same validator');
          return await finishRemoteUnchanged(snapshot, deps, creds, proxy, storedConditionalEtag);
        }
        if (davEtag) {
          console.info('[sync] clean preflight: PROPFIND changed');
        } else {
          console.info('[sync] clean preflight: PROPFIND unavailable');
          const rangeResponse = await deps.transport.request(
            'GET', creds, proxy, V3_RESOURCE, null, null, null, null, RANGE_PROBE,
          );
          const rangeEtag = rangeProbeEtag(rangeResponse);
          if (rangeEtag === storedConditionalEtag) {
            console.info('[sync] clean preflight: RANGE same validator');
            return await finishRemoteUnchanged(snapshot, deps, creds, proxy, storedConditionalEtag);
          }
          if (rangeEtag) {
            console.info('[sync] clean preflight: RANGE changed');
          } else {
            console.info('[sync] clean preflight: RANGE unavailable');
            useConditionalGetFallback = true;
          }
        }
      } else {
        console.info('[sync] clean preflight: normal full merge');
      }
      const v3Response = await deps.transport.request(
        'GET', creds, proxy, V3_RESOURCE, null, null,
        useConditionalGetFallback ? storedConditionalEtag : null, null,
      );
      let remotePayload: SyncPayloadV3;
      let baseSide: SyncMergeSide;
      let remoteSide: SyncMergeSide;
      let baseCollectionState: CollectionSyncState;
      let remoteCollectionState: CollectionSyncState;
      let validator: ConditionalValidator | null = null;
      let creating = false;
      let legacyImported = false;
      let legacyFingerprint = snapshot.v2SourceFingerprint;

      if (v3Response.status === 304) {
        if (!storedConditionalEtag || !useConditionalGetFallback) throw new Error('HTTP Error: 304');
        console.info('[sync] clean preflight: HTTP conditional fallback 304');
        return await finishRemoteUnchanged(snapshot, deps, creds, proxy, storedConditionalEtag);
      } else if (v3Response.status === 200) {
        try {
          validator = await conditionalValidatorForResource(v3Response, creds, proxy, V3_RESOURCE, deps.transport);
        } catch (error) {
          if (!String(error).includes('conditional_write_unsupported')) throw error;
          console.info('[sync] clean preflight: validator unavailable; full merge without upload permission');
          validator = null;
        }
        if (storedConditionalEtag && validator?.etag === storedConditionalEtag) {
          console.info('[sync] clean preflight: HTTP 200 same validator');
          return await finishRemoteUnchanged(snapshot, deps, creds, proxy, storedConditionalEtag);
        }
        remotePayload = parseSyncPayloadV3(v3Response.body);
        remoteSide = sideOfPayload(remotePayload);
        remoteCollectionState = collectionSideOfPayload(remotePayload);
        const recoverablePublishedIntent = snapshot.publishIntent?.commitId === remotePayload.commitId
          && snapshot.publishIntent.payloadFingerprint === await contentFingerprint(remotePayload);
        const sameDeviceBootstrap = !snapshot.baseline && remotePayload.writerId === snapshot.deviceId;
        baseSide = snapshot.baseline
          ? sideOfPayload(parseSyncPayloadV3(snapshot.baseline))
          : (recoverablePublishedIntent || sameDeviceBootstrap ? remoteSide : { records: [], tombstones: [] });
        baseCollectionState = snapshot.baseline
          ? collectionSideOfPayload(parseSyncPayloadV3(snapshot.baseline))
          : (recoverablePublishedIntent || sameDeviceBootstrap ? remoteCollectionState : emptyCollectionState());
        legacyFingerprint = await checkLegacyRemote(
          deps, creds, proxy, snapshot.v2SourceFingerprint,
        );
      } else if (v3Response.status === 404) {
        creating = true;
        const legacyResponse = await deps.transport.request('GET', creds, proxy, LEGACY_RESOURCE);
        const now = deps.now().toISOString();
        remotePayload = emptySyncPayload(snapshot.deviceId, now);
        if (legacyResponse.status === 200) {
          const legacy = legacyPayload(legacyResponse.body);
          remoteSide = sideOfLegacy(legacy);
          baseSide = snapshot.baseline ? sideOfPayload(parseSyncPayloadV3(snapshot.baseline)) : remoteSide;
          remoteCollectionState = emptyCollectionState();
          baseCollectionState = snapshot.baseline ? collectionSideOfPayload(parseSyncPayloadV3(snapshot.baseline)) : remoteCollectionState;
          legacyFingerprint = legacyResponse.etag || await contentFingerprint(legacyResponse.body);
          legacyImported = true;
        } else if (legacyResponse.status === 404) {
          remoteSide = { records: [], tombstones: [] };
          baseSide = snapshot.baseline ? sideOfPayload(parseSyncPayloadV3(snapshot.baseline)) : remoteSide;
          remoteCollectionState = emptyCollectionState();
          baseCollectionState = snapshot.baseline ? collectionSideOfPayload(parseSyncPayloadV3(snapshot.baseline)) : remoteCollectionState;
          legacyFingerprint = 'missing';
        } else {
          throw new Error(`HTTP Error: ${legacyResponse.status}`);
        }
      } else {
        throw new Error(`HTTP Error: ${v3Response.status}`);
      }

      const now = deps.now().toISOString();
      const activeConflicts = snapshot.conflicts.filter(conflict => {
        if (conflict.base || remotePayload.writerId !== snapshot.deviceId
          || !conflict.local || !conflict.remote
          || conflict.local.revActor !== snapshot.deviceId
          || (conflict.local.rev ?? 0) <= (conflict.remote.rev ?? 0)) return true;
        const currentRemote = remoteSide.records.find(record => record.id === conflict.id);
        return !currentRemote || !syncValuesEqual(currentRemote, conflict.remote);
      });
      const merged = mergeSyncStates(baseSide, localSide, remoteSide, snapshot.deviceId, now, activeConflicts);
      const mergedCollections = mergeCollectionStates(
        baseCollectionState, localCollectionState, remoteCollectionState,
        (kind, id, local, remote) => {
          const label = kind === 'collection'
            ? ((local as import('../../../shared/types').WatchCollection | null)?.name || (remote as import('../../../shared/types').WatchCollection | null)?.name || id)
            : id;
          return confirmUpgrade(deps, `收藏集数据“${label}”在本机和云端同时发生变化。\n\n确定：采用本机；取消：采用云端。`) ? 'local' : 'remote';
        },
      );
      const baselinePayload = snapshot.baseline ? parseSyncPayloadV3(snapshot.baseline) : null;
      const mergedCompletions = mergeEpisodeCompletions(
        baselinePayload?.episodeCompletions ?? [], snapshot.episodeCompletions, remotePayload.episodeCompletions ?? [],
        (localCompletion, remoteCompletion) => {
          const record = snapshot.records.find(item => item.id === localCompletion.recordId)
            ?? remotePayload.records.find(item => item.id === localCompletion.recordId);
          const title = record?.chineseName || record?.originalName || localCompletion.recordId;
          return confirmUpgrade(deps,
            `「${title}」第 ${localCompletion.episodeNumber} 集在本机和云端记录了不同的完成时间。\n\n`
            + `本机：${localCompletion.completedAt ?? '时间未知'}\n`
            + `云端：${remoteCompletion.completedAt ?? '时间未知'}\n\n`
            + '确定：采用本机；取消：采用云端。') ? 'local' : 'remote';
        },
      );
      const localRecordIds = new Set(merged.local.records.map(record => record.id));
      const remoteRecordIds = new Set(merged.remote.records.map(record => record.id));
      const localCollections = { ...mergedCollections, collectionMembers: mergedCollections.collectionMembers.filter(item => localRecordIds.has(item.recordId)) };
      const remoteCollections = { ...mergedCollections, collectionMembers: mergedCollections.collectionMembers.filter(item => remoteRecordIds.has(item.recordId)) };
      const localCompletions = mergedCompletions.filter(item => localRecordIds.has(item.recordId));
      const remoteCompletions = mergedCompletions.filter(item => remoteRecordIds.has(item.recordId));
      const remoteRecordsChanged = !syncSideEquivalent(merged.remote, remoteSide);
      const remoteEpisodesChanged = !episodeCompletionStateEquivalent(remoteCompletions, remotePayload.episodeCompletions ?? []);
      const remoteCollectionsChanged = !collectionStateEquivalent(remoteCollections, remoteCollectionState);
      const remoteChanged = remoteRecordsChanged || remoteEpisodesChanged || remoteCollectionsChanged;
      console.info('[sync] remote change classification', {
        records: remoteRecordsChanged, episodes: remoteEpisodesChanged, collections: remoteCollectionsChanged,
      });
      let confirmedPayload = remotePayload;
      let confirmedEtag = validator?.etag ?? null;

      if (creating || remoteChanged) {
        if (!creating && !validator) throw new Error('conditional_write_unsupported');
        const nextPayload = buildSyncPayload(remotePayload, merged.remote, remoteCompletions, remoteCollections, snapshot.deviceId, now, deps.uuid());
        if (remotePayload.schemaVersion === 3 && nextPayload.schemaVersion === 4
          && await deps.database.getSettingAsync('sync_v4_upgrade_confirmed') !== '1') {
          if (!confirmUpgrade(deps, '逐集历史需要把云端同步格式升级到 V4。旧版程序将安全停止同步，所有设备都需要更新到支持 V4 的版本。是否继续？')) throw new Error('episode_sync_upgrade_required');
          await deps.database.setSettingAsync('sync_v4_upgrade_confirmed', '1');
        }
        if (remotePayload.schemaVersion < 5 && nextPayload.schemaVersion === 5
          && await deps.database.getSettingAsync('sync_v5_upgrade_confirmed') !== '1') {
          if (!confirmUpgrade(deps, '收藏集需要把云端同步格式升级到 V5。旧版程序将安全停止同步，所有设备都需要更新到支持 V5 的版本。是否继续？')) throw new Error('collections_sync_upgrade_required');
          await deps.database.setSettingAsync('sync_v5_upgrade_confirmed', '1');
        }
        if (remotePayload.schemaVersion < 6 && nextPayload.schemaVersion === 6
          && await deps.database.getSettingAsync('sync_v6_upgrade_confirmed') !== '1') {
          if (!confirmUpgrade(deps, '系列类型和年代排序需要把云端同步格式升级到 V6。旧版程序将安全停止同步，所有设备都需要更新到支持 V6 的版本。是否继续？')) throw new Error('series_sync_upgrade_required');
          await deps.database.setSettingAsync('sync_v6_upgrade_confirmed', '1');
        }
        await deps.database.prepareSyncPublishIntent({
          targetId: snapshot.targetId, targetEpoch: snapshot.targetEpoch, commitId: nextPayload.commitId,
          previousCommitId: remotePayload.commitId || null, expectedGeneration: snapshot.recordsGeneration,
          payloadFingerprint: await contentFingerprint(nextPayload),
        });
        const put = await deps.transport.request(
          'PUT', creds, proxy, V3_RESOURCE, JSON.stringify(nextPayload),
          !creating && validator?.header === 'if-match' ? validator.etag : null,
          creating ? '*' : null,
          !creating && validator?.header === 'dav-if' ? validator.etag : null,
        );
        if (put.status === 412) {
          rejectedValidatorFingerprints.push(await contentFingerprint({ mode: creating ? 'if-none-match' : validator?.header, etag: creating ? '*' : validator?.etag }));
          continue;
        }
        if (!successful(put.status)) throw new Error(`HTTP Error: ${put.status}`);
        confirmedPayload = nextPayload;
        confirmedEtag = put.etag;
        if (!strongEtag(confirmedEtag)) {
          const verification = await deps.transport.request('GET', creds, proxy, V3_RESOURCE);
          if (verification.status !== 200) throw new Error('conditional_write_unsupported');
          const verified = parseSyncPayloadV3(verification.body);
          if (verified.commitId !== nextPayload.commitId) continue;
          confirmedPayload = verified;
          confirmedEtag = (await conditionalValidatorForResource(verification, creds, proxy, V3_RESOURCE, deps.transport)).etag;
        }
        assertEntityTag(confirmedEtag);
      }

      await deps.database.commitSyncResult({
        targetId: snapshot.targetId, targetEpoch: snapshot.targetEpoch, expectedGeneration: snapshot.recordsGeneration,
        records: merged.local.records, tombstones: merged.local.tombstones, episodeCompletions: localCompletions,
        collections: localCollections.collections, collectionMembers: localCollections.collectionMembers,
        collectionTombstones: localCollections.collectionTombstones, collectionMemberTombstones: localCollections.collectionMemberTombstones,
        baseline: confirmedPayload, conflicts: merged.conflicts, remoteEtag: confirmedEtag,
        lastCommit: { revision: confirmedPayload.revision, commitId: confirmedPayload.commitId, committedAt: confirmedPayload.committedAt },
        v2SourceFingerprint: legacyFingerprint, acknowledgeOutbox: true,
      });
      return { ok: true, records: merged.local.records, conflicts: merged.conflicts, conflictCount: merged.conflicts.length, legacyImported };
    }
    if (rejectedValidatorFingerprints.length === MAX_PRECONDITION_RETRIES && new Set(rejectedValidatorFingerprints).size === 1) {
      return { ok: false, error: 'conditional_validator_rejected' };
    }
    return { ok: false, error: 'remote_busy' };
  } catch (error) {
    return syncError(error);
  }
}

export function createSyncService(dependencies: Partial<SyncServiceDependencies> = {}) {
  const deps = { ...defaultDependencies, ...dependencies };
  return {
    syncToWebDAV: (creds: WebDAVCreds, ignoredRecords?: WatchRecord[]) => syncWithDependencies(creds, ignoredRecords, deps),
  };
}

export async function syncToWebDAV(
  creds: WebDAVCreds,
  ignoredRecords?: WatchRecord[],
  dependencies?: Partial<SyncServiceDependencies>,
) {
  return createSyncService(dependencies).syncToWebDAV(creds, ignoredRecords);
}
