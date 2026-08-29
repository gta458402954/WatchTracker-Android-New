import type { LocalExportSnapshot } from '../../shared/lib/database.ts';

export interface LocalBackupV4 extends LocalExportSnapshot {
  formatVersion: 4;
  exportedAt: string;
}

/** Desktop-compatible whitelist; sync state and credentials cannot flow in. */
export function buildLocalBackupV4(
  snapshot: LocalExportSnapshot,
  now = new Date(),
): LocalBackupV4 {
  return {
    formatVersion: 4,
    exportedAt: now.toISOString(),
    records: snapshot.records,
    episodeCompletions: snapshot.episodeCompletions,
    collections: snapshot.collections,
    collectionMembers: snapshot.collectionMembers,
  };
}

export function serializeLocalBackupV4(payload: LocalBackupV4): string {
  return JSON.stringify(payload, null, 2);
}

export function localBackupFileName(now = new Date()): string {
  const pad = (value: number) => String(value).padStart(2, '0');
  return `WatchTracker-backup-${now.getFullYear()}-${pad(now.getMonth() + 1)}-${pad(now.getDate())}-${pad(now.getHours())}${pad(now.getMinutes())}${pad(now.getSeconds())}.json`;
}
