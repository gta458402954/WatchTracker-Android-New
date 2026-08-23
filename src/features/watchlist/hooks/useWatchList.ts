import { useCallback, useEffect, useRef } from 'react';
import { useRecordRepository } from './useRecordRepository';
import { useSyncCoordinator } from '../../sync/hooks/useSyncCoordinator';

export function useWatchList(
  syncInterval = 30,
  pullIntervalMinutes = 15,
  onBackgroundError?: (message: string) => void,
) {
  // The ref bridge keeps the repository independent from sync concerns while
  // allowing both hooks to be composed without a circular callback dependency.
  const scheduleLocalWriteRef = useRef<() => void | Promise<void>>(() => undefined);
  const repository = useRecordRepository(() => scheduleLocalWriteRef.current());
  const coordinator = useSyncCoordinator(
    syncInterval,
    pullIntervalMinutes,
    repository.reloadRecords,
    onBackgroundError,
  );
  const { reloadRecords } = repository;
  const { scheduleLocalWrite, startCoordinator } = coordinator;
  useEffect(() => {
    scheduleLocalWriteRef.current = scheduleLocalWrite;
  }, [scheduleLocalWrite]);

  const loadRecords = useCallback(async () => {
    const loaded = await reloadRecords();
    await startCoordinator();
    return loaded;
  }, [reloadRecords, startCoordinator]);

  const reloadAndSchedule = useCallback(async () => {
    const loaded = await reloadRecords();
    await scheduleLocalWrite();
    return loaded;
  }, [reloadRecords, scheduleLocalWrite]);

  return {
    records: repository.records,
    loadRecords,
    addRecord: repository.addRecord,
    updateRecord: repository.updateRecord,
    deleteRecord: repository.deleteRecord,
    changeNextEpisode: repository.changeNextEpisode,
    replaceRecords: repository.replaceRecords,
    syncNow: coordinator.syncNow,
    syncRuntime: coordinator.syncRuntime,
    isSyncPaused: coordinator.isSyncPaused,
    toggleSyncPause: coordinator.toggleSyncPause,
    notifySyncConfigurationChanged: coordinator.notifySyncConfigurationChanged,
    reloadAndSchedule,
  };
}
