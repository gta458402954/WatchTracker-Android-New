import type { UpdateWatchRecord } from '../../../shared/types';
import type { BatchMetadataField, TmdbIdentityPlan, TmdbMatch } from '../../../shared/lib/batchMetadata';

export interface BatchCandidate { match: TmdbMatch; label: string; }
export interface BatchPlanRow {
  recordId: string; recordName: string; status: 'ready' | 'choice' | 'skipped' | 'failed'; updates: UpdateWatchRecord;
  fields: BatchMetadataField[]; noDataFields?: BatchMetadataField[]; candidates?: BatchCandidate[];
  remoteIdentity?: string; identityPlan?: TmdbIdentityPlan; identityConflict?: string; reason?: string;
}
export interface BatchApplyResult { plan: BatchPlanRow; status: 'updated' | 'skipped' | 'failed'; reason?: string; }
