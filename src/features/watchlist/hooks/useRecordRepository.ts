import { useCallback, useEffect, useRef, useState } from 'react';
import { UpdateWatchRecord, WatchRecord } from '../../../shared/types';
import {
  deleteRecord as dbDeleteRecord,
  enableEpisodeTracking as dbEnableEpisodeTracking,
  getAllRecordsAsync,
  insertRecord,
  replaceAllRecords,
  setNextEpisode as dbSetNextEpisode,
  updateRecord as dbUpdateRecord,
} from '../../../shared/lib/database';

export type LocalWriteHandler = () => void | Promise<void>;

/** The Rust database remains the source of truth for records. */
export function useRecordRepository(onLocalWrite: LocalWriteHandler) {
  const [records, setRecords] = useState<WatchRecord[]>([]);
  const recordsRef = useRef<WatchRecord[]>([]);
  const onLocalWriteRef = useRef(onLocalWrite);

  useEffect(() => {
    onLocalWriteRef.current = onLocalWrite;
  }, [onLocalWrite]);

  const notifyLocalWrite = useCallback(() => {
    void onLocalWriteRef.current();
  }, []);

  const reloadRecords = useCallback(async () => {
    const allRecords = await getAllRecordsAsync();
    recordsRef.current = allRecords;
    setRecords(allRecords);
    return allRecords;
  }, []);

  const loadRecords = useCallback(() => reloadRecords(), [reloadRecords]);

  const addRecord = useCallback(async (record: Omit<WatchRecord, 'id' | 'createdAt'>) => {
    const now = new Date().toISOString();
    const persisted = await insertRecord({ ...record, id: crypto.randomUUID(), createdAt: now, updatedAt: now });
    const updated = [persisted, ...recordsRef.current];
    recordsRef.current = updated;
    setRecords(updated);
    notifyLocalWrite();
    return persisted;
  }, [notifyLocalWrite]);

  const updateRecord = useCallback(async (id: string, updates: UpdateWatchRecord) => {
    const persisted = await dbUpdateRecord(id, updates);
    const updated = recordsRef.current.map(record => record.id === id ? persisted : record);
    recordsRef.current = updated;
    setRecords(updated);
    notifyLocalWrite();
    return persisted;
  }, [notifyLocalWrite]);

  const deleteRecord = useCallback(async (id: string) => {
    await dbDeleteRecord(id);
    const updated = recordsRef.current.filter(record => record.id !== id);
    recordsRef.current = updated;
    setRecords(updated);
    notifyLocalWrite();
  }, [notifyLocalWrite]);

  const changeNextEpisode = useCallback(async (record: WatchRecord, nextEpisode: number | null) => {
    const expectedRev = record.rev ?? 0;
    const tracking = record.episodeTrackingEnabled
      ? await dbSetNextEpisode(record.id, nextEpisode, expectedRev)
      : await dbEnableEpisodeTracking(record.id, nextEpisode as number, expectedRev);
    const updated = recordsRef.current.map(item => item.id === record.id ? tracking.record : item);
    recordsRef.current = updated;
    setRecords(updated);
    notifyLocalWrite();
    return tracking;
  }, [notifyLocalWrite]);

  const replaceRecords = useCallback(async (newRecords: WatchRecord[]) => {
    await replaceAllRecords(newRecords, 'import');
    const persisted = await reloadRecords();
    notifyLocalWrite();
    return persisted;
  }, [notifyLocalWrite, reloadRecords]);

  return {
    records,
    loadRecords,
    reloadRecords,
    addRecord,
    updateRecord,
    deleteRecord,
    changeNextEpisode,
    replaceRecords,
  };
}
