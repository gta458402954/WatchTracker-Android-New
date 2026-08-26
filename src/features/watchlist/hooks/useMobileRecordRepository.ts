import { useCallback, useEffect, useRef } from 'react';
import type { UpdateWatchRecord, WatchRecord } from '../../../shared/types';
import type { EpisodeTracking } from '../../../shared/lib/database.ts';
import { mobileEpisodeWriteReason, type MobileEpisodeWriteReason } from '../mobileEpisodeTracking.ts';
import { useRecordRepository, type LocalWriteHandler } from './useRecordRepository';

export type MobileWriteReason = 'missing' | 'stale' | 'locked' | 'error';
export type MobileWriteResult = { ok: true; record: WatchRecord } | { ok: false; reason: MobileWriteReason; error?: unknown };
export type MobileEpisodeWriteResult = { ok: true; tracking: EpisodeTracking } | { ok: false; reason: MobileEpisodeWriteReason; error?: unknown };

/** Mobile-only write façade. The underlying Rust CRUD remains unchanged and is not CAS.
 * We reload and compare the revision immediately before each write to reduce stale UI writes. */
export function useMobileRecordRepository(onLocalWrite: LocalWriteHandler) {
  const repository = useRecordRepository(onLocalWrite);
  const reloadRecords = repository.reloadRecords;
  const recordsRef = useRef(repository.records);
  useEffect(() => { recordsRef.current = repository.records; }, [repository.records]);
  const reloadLatest = useCallback(async (id: string) => {
    const latest = await reloadRecords();
    recordsRef.current = latest;
    return latest.find(record => record.id === id) ?? null;
  }, [reloadRecords]);

  const updateMobileRecord = useCallback(async (id: string, updates: UpdateWatchRecord, expectedRev: number, mode: 'edit' | 'status' | 'lock' | 'unlock' = 'edit'): Promise<MobileWriteResult> => {
    try {
      const latest = await reloadLatest(id);
      if (!latest) return { ok: false, reason: 'missing' };
      if ((latest.rev ?? 0) !== expectedRev) return { ok: false, reason: 'stale' };
      if (latest.isLocked && mode !== 'unlock') return { ok: false, reason: 'locked' };
      const record = await repository.updateRecord(id, updates);
      return { ok: true, record };
    } catch (error) {
      return { ok: false, reason: 'error', error };
    }
  }, [reloadLatest, repository]);

  const deleteMobileRecord = useCallback(async (id: string, expectedRev: number): Promise<{ ok: true } | { ok: false; reason: MobileWriteReason; error?: unknown }> => {
    try {
      const latest = await reloadLatest(id);
      if (!latest) return { ok: false, reason: 'missing' };
      if ((latest.rev ?? 0) !== expectedRev) return { ok: false, reason: 'stale' };
      if (latest.isLocked) return { ok: false, reason: 'locked' };
      await repository.deleteRecord(id);
      return { ok: true };
    } catch (error) {
      return { ok: false, reason: 'error', error };
    }
  }, [reloadLatest, repository]);

  /** Episode commands are real Rust CAS transactions. Their persisted record and
   * history are returned together, so no optimistic episode state is created. */
  const changeMobileEpisode = useCallback(async (record: WatchRecord, nextEpisode: number | null): Promise<MobileEpisodeWriteResult> => {
    try {
      const tracking = await repository.changeNextEpisode(record, nextEpisode);
      return { ok: true, tracking };
    } catch (error) {
      const reason = mobileEpisodeWriteReason(error);
      try {
        await reloadRecords();
      } catch {
        // Preserve the command failure classification. Initialization and the
        // next explicit reload still surface a database read failure safely.
      }
      return { ok: false, reason, error };
    }
  }, [reloadRecords, repository]);

  return { ...repository, updateMobileRecord, deleteMobileRecord, changeMobileEpisode, reloadLatest };
}
