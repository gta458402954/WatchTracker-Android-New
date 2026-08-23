import type { WatchRecord } from '../../../shared/types';
import type { SyncConflictV3 } from '../../../shared/lib/syncMerge';

export interface SyncResult {
  ok: boolean;
  error?: string;
  records?: WatchRecord[];
  conflictCount?: number;
  conflicts?: SyncConflictV3[];
  staleLocal?: boolean;
  legacyImported?: boolean;
}
