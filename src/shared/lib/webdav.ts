import { invoke } from '@tauri-apps/api/core';
import { getSettingAsync, getSyncRuntimeState } from './database.ts';
/** Compatibility facade for the legacy WebDAV module path. */
import type { WatchRecord } from '../types';
import {
  clearCreds,
  getCreds,
  hasCreds,
  normalizeSyncTargetUrl,
  saveCreds,
} from '../../features/sync/infrastructure/syncCredentials.ts';
import type { WebDAVCreds } from '../../features/sync/infrastructure/webdavTransport.ts';
import {
  createLegacyImportService,
  importLegacyChangesToConflictCenter as importLegacyWithCreds,
  loadFromWebDAV as loadFromWebDAVWithCreds,
  probeSyncTarget as probeSyncTargetWithCreds,
  type SyncTargetProbe,
} from '../../features/sync/services/legacyImportService.ts';
import { syncToWebDAV as syncToWebDAVWithCreds } from '../../features/sync/services/syncService.ts';
import type { SyncResult } from '../../features/sync/services/syncContracts.ts';
import type { SyncConflictV3 } from './syncMerge';

export type { WebDAVCreds, SyncResult, SyncTargetProbe };
export type SyncConflict = SyncConflictV3;
export { normalizeSyncTargetUrl, saveCreds, getCreds, clearCreds, hasCreds };
export { syncFailureMessage } from '../../features/sync/domain/syncErrors.ts';

export async function probeSyncTarget(creds: WebDAVCreds): Promise<SyncTargetProbe> {
  return probeSyncTargetWithCreds(creds);
}

/** Keeps the old ignored-records argument while the service reads its snapshot from Rust. */
export async function syncToWebDAV(_ignoredRecords?: WatchRecord[], automatic = false): Promise<SyncResult> {
  const creds = await getCreds();
  if (!creds) return { ok: false, error: '未配置凭据' };
  if (creds.targetId && creds.targetEpoch !== undefined) {
    try {
      const expectedAttemptAt = automatic ? (await getSyncRuntimeState()).scheduler.lastAttemptAt : null;
      const status = await invoke<string>('s2_sync_cycle', { input: { targetId: creds.targetId, targetEpoch: creds.targetEpoch, automatic, expectedAttemptAt }, proxy: await getSettingAsync('network_proxy') });
      if (status !== 'legacyS1Required') {
        return { ok: status === 'success', s2Managed: true, skipped: status === 'automaticSkipped', error: status === 'success' ? undefined : `s2_${status}` };
      }
    } catch (error) {
      return { ok: false, s2Managed: true, error: String(error) };
    }
  }
  return syncToWebDAVWithCreds(creds, _ignoredRecords, {
    confirm: message => window.confirm(message),
  });
}

export async function loadFromWebDAV(): Promise<{ ok: boolean; data?: WatchRecord[]; error?: string }> {
  const creds = await getCreds();
  if (!creds) return { ok: false, error: '未配置凭据' };
  return loadFromWebDAVWithCreds(creds);
}

export async function importLegacyChangesToConflictCenter(): Promise<SyncResult> {
  const creds = await getCreds();
  if (!creds) return { ok: false, error: '未配置凭据' };
  return importLegacyWithCreds(creds);
}

export async function getSyncConflicts(): Promise<SyncConflictV3[]> {
  return createLegacyImportService().getSyncConflicts();
}

export async function clearResolvedSyncConflicts(_records: WatchRecord[]): Promise<SyncConflictV3[]> {
  return createLegacyImportService().clearResolvedSyncConflicts(_records);
}
