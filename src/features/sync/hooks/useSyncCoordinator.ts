import { useCallback, useEffect, useRef, useState } from 'react';
import {
  getSyncRuntimeState,
  recordSyncFailure,
  setAutoSyncPaused,
  type SyncRuntimeState,
} from '../../../shared/lib/database';
import {
  hasCreds,
  syncFailureMessage,
  syncToWebDAV,
  type SyncResult,
} from '../../../shared/lib/webdav';
import {
  classifySyncFailure,
  focusPullDue,
  isDue,
  nextRetryAt,
  periodicPullDue,
} from '../../../shared/lib/syncScheduling';
import { publicFailureMessage, reportOperationFailure } from '../../../shared/lib/feedback';

type AutomaticTrigger = 'startup' | 'local-write' | 'focus' | 'online' | 'periodic' | 'retry' | 'resume';

function safeFailureCode(error?: string): string {
  const value = error ?? '';
  const known = [
    'remote_busy', 'stale_local_snapshot', 'conditional_write_unsupported',
    'conditional_validator_rejected',
    'unsupported_remote_schema', 'legacy_remote_changed',
    'episode_sync_upgrade_required', 'episode_completion_conflict',
  ].find(code => value.includes(code));
  if (known) return known;
  const http = value.match(/HTTP Error:\s*(\d{3})/);
  return http ? `http_${http[1]}` : 'network_or_sync_error';
}

export function useSyncCoordinator(
  syncInterval: number,
  pullIntervalMinutes: number,
  reloadRecords: () => Promise<unknown>,
  onBackgroundError?: (message: string) => void,
) {
  const [syncRuntime, setSyncRuntime] = useState<SyncRuntimeState | null>(null);
  const syncInFlightRef = useRef<Promise<SyncResult> | null>(null);
  const wakeTimerRef = useRef<ReturnType<typeof setTimeout> | null>(null);
  const wakeDueAtRef = useRef<number | null>(null);
  const debounceTimerRef = useRef<ReturnType<typeof setTimeout> | null>(null);
  const periodicTimerRef = useRef<ReturnType<typeof setInterval> | null>(null);
  const startedRef = useRef(false);
  const rerunRequestedRef = useRef(false);
  const intervalRef = useRef(syncInterval);
  const pullIntervalRef = useRef(pullIntervalMinutes);
  const runtimeRef = useRef<SyncRuntimeState | null>(null);
  const lastNotifiedErrorRef = useRef<string | null>(null);

  const updateRuntime = useCallback((runtime: SyncRuntimeState) => {
    runtimeRef.current = runtime;
    setSyncRuntime(runtime);
    return runtime;
  }, []);

  const refreshSyncRuntime = useCallback(async () => updateRuntime(await getSyncRuntimeState()), [updateRuntime]);

  useEffect(() => { intervalRef.current = syncInterval; }, [syncInterval]);
  useEffect(() => { pullIntervalRef.current = pullIntervalMinutes; }, [pullIntervalMinutes]);

  const notifyBackgroundFailure = useCallback((error?: string) => {
    const code = safeFailureCode(error);
    if (lastNotifiedErrorRef.current === code) return;
    lastNotifiedErrorRef.current = code;
    onBackgroundError?.(syncFailureMessage(error) ?? publicFailureMessage('自动同步'));
  }, [onBackgroundError]);

  const queueAutomaticRef = useRef<(trigger: AutomaticTrigger, delayMs: number, reset?: boolean) => void>(() => undefined);

  const executeSync = useCallback(async (manual = false): Promise<SyncResult> => {
    if (syncInFlightRef.current) {
      rerunRequestedRef.current = true;
      return syncInFlightRef.current;
    }
    const task = syncToWebDAV();
    syncInFlightRef.current = task;
    try {
      const result = await task;
      if (result.ok) {
        lastNotifiedErrorRef.current = null;
        await reloadRecords();
        const runtime = await refreshSyncRuntime();
        if (result.conflictCount && !manual) {
          onBackgroundError?.(`云端核对完成，有 ${result.conflictCount} 项冲突需要在设置中选择。`);
        }
        if (runtime.outbox.pending) rerunRequestedRef.current = true;
      } else {
        const disposition = classifySyncFailure(result.error);
        if (disposition === 'stale-local') {
          queueAutomaticRef.current('retry', 250);
        } else {
          const current = runtimeRef.current ?? await getSyncRuntimeState();
          const nextAttemptAt = disposition === 'retry'
            ? nextRetryAt(current.scheduler.consecutiveFailures + 1, Date.now(), Math.random() * 0.4 - 0.2)
            : null;
          const runtime = await recordSyncFailure(safeFailureCode(result.error), nextAttemptAt, current.targetId, current.targetEpoch);
          updateRuntime(runtime);
          if (!manual) notifyBackgroundFailure(result.error);
          if (nextAttemptAt) {
            queueAutomaticRef.current('retry', Math.max(0, Date.parse(nextAttemptAt) - Date.now()));
          }
        }
      }
      return result;
    } catch (error) {
      reportOperationFailure('Sync.Coordinator', error);
      const message = String(error);
      const current = runtimeRef.current ?? await getSyncRuntimeState();
      const nextAttemptAt = nextRetryAt(current.scheduler.consecutiveFailures + 1, Date.now());
      updateRuntime(await recordSyncFailure(safeFailureCode(message), nextAttemptAt, current.targetId, current.targetEpoch));
      if (!manual) notifyBackgroundFailure(message);
      queueAutomaticRef.current('retry', Math.max(0, Date.parse(nextAttemptAt) - Date.now()));
      return { ok: false, error: message };
    } finally {
      if (syncInFlightRef.current === task) syncInFlightRef.current = null;
      if (rerunRequestedRef.current) {
        rerunRequestedRef.current = false;
        queueAutomaticRef.current('retry', 0);
      }
    }
  }, [notifyBackgroundFailure, onBackgroundError, refreshSyncRuntime, reloadRecords, updateRuntime]);

  const runAutomatic = useCallback(async (trigger: AutomaticTrigger) => {
    try {
      const runtime = await refreshSyncRuntime();
      if (runtime.scheduler.paused || !await hasCreds()) return;
      const retryFixedConditionalWriteOnStartup = trigger === 'startup'
        && ['conditional_write_unsupported', 'conditional_validator_rejected']
          .includes(runtime.scheduler.lastErrorCode ?? '');
      if (runtime.scheduler.lastErrorCode && !runtime.scheduler.nextAttemptAt
        && trigger !== 'resume' && !retryFixedConditionalWriteOnStartup) return;
      if (trigger !== 'online' && trigger !== 'resume' && !isDue(runtime.scheduler.nextAttemptAt, Date.now())) {
        queueAutomaticRef.current(
          'retry',
          Math.max(0, Date.parse(runtime.scheduler.nextAttemptAt as string) - Date.now()),
        );
        return;
      }
      await executeSync(false);
    } catch (error) {
      reportOperationFailure('Sync.AutomaticCoordinator', error);
      notifyBackgroundFailure(String(error));
    }
  }, [executeSync, notifyBackgroundFailure, refreshSyncRuntime]);

  const queueAutomatic = useCallback((trigger: AutomaticTrigger, delayMs: number, reset = false) => {
    if (runtimeRef.current?.scheduler.paused) return;
    const dueAt = Date.now() + Math.max(0, delayMs);
    if (reset && wakeTimerRef.current) {
      clearTimeout(wakeTimerRef.current);
      wakeTimerRef.current = null;
      wakeDueAtRef.current = null;
    }
    if (wakeTimerRef.current) {
      if ((wakeDueAtRef.current ?? dueAt) <= dueAt) return;
      clearTimeout(wakeTimerRef.current);
    }
    wakeDueAtRef.current = dueAt;
    wakeTimerRef.current = setTimeout(() => {
      wakeTimerRef.current = null;
      wakeDueAtRef.current = null;
      void runAutomatic(trigger);
    }, Math.max(0, delayMs));
  }, [runAutomatic]);
  useEffect(() => {
    queueAutomaticRef.current = queueAutomatic;
  }, [queueAutomatic]);

  const scheduleLocalWrite = useCallback(async () => {
    try { await refreshSyncRuntime(); } catch (error) { reportOperationFailure('Sync.RefreshOutbox', error); }
    if (debounceTimerRef.current) clearTimeout(debounceTimerRef.current);
    debounceTimerRef.current = setTimeout(() => {
      debounceTimerRef.current = null;
      queueAutomatic('local-write', 0, true);
    }, intervalRef.current * 1000);
  }, [queueAutomatic, refreshSyncRuntime]);

  const checkFocusPull = useCallback(async () => {
    if (!startedRef.current) return;
    try {
      const runtime = await refreshSyncRuntime();
      if (focusPullDue(runtime.scheduler.lastRemoteCheckAt, Date.now())) queueAutomatic('focus', 0);
    } catch (error) { reportOperationFailure('Sync.FocusState', error); }
  }, [queueAutomatic, refreshSyncRuntime]);

  const checkPeriodicPull = useCallback(async () => {
    if (!startedRef.current) return;
    try {
      const runtime = await refreshSyncRuntime();
      if (periodicPullDue(runtime.scheduler.lastRemoteCheckAt, pullIntervalRef.current, Date.now())) {
        queueAutomatic('periodic', 0);
      }
    } catch (error) { reportOperationFailure('Sync.PeriodicState', error); }
  }, [queueAutomatic, refreshSyncRuntime]);

  const startCoordinator = useCallback(async () => {
    if (startedRef.current) return;
    startedRef.current = true;
    const runtime = await refreshSyncRuntime();
    if (!runtime.scheduler.paused && await hasCreds()) {
      queueAutomatic('startup', runtime.outbox.pending ? 0 : 3000);
    }
    periodicTimerRef.current = setInterval(() => void checkPeriodicPull(), 30_000);
  }, [checkPeriodicPull, queueAutomatic, refreshSyncRuntime]);

  useEffect(() => {
    const onFocus = () => void checkFocusPull();
    const onVisibility = () => { if (document.visibilityState === 'visible') void checkFocusPull(); };
    const onOnline = () => queueAutomatic('online', 0, true);
    window.addEventListener('focus', onFocus);
    window.addEventListener('online', onOnline);
    document.addEventListener('visibilitychange', onVisibility);
    return () => {
      window.removeEventListener('focus', onFocus);
      window.removeEventListener('online', onOnline);
      document.removeEventListener('visibilitychange', onVisibility);
      if (wakeTimerRef.current) clearTimeout(wakeTimerRef.current);
      if (debounceTimerRef.current) clearTimeout(debounceTimerRef.current);
      if (periodicTimerRef.current) clearInterval(periodicTimerRef.current);
    };
  }, [checkFocusPull, queueAutomatic]);

  const syncNow = useCallback(async () => {
    if (wakeTimerRef.current) { clearTimeout(wakeTimerRef.current); wakeTimerRef.current = null; }
    if (debounceTimerRef.current) { clearTimeout(debounceTimerRef.current); debounceTimerRef.current = null; }
    return executeSync(true);
  }, [executeSync]);

  const toggleSyncPause = useCallback(async () => {
    const current = runtimeRef.current ?? await getSyncRuntimeState();
    const runtime = updateRuntime(await setAutoSyncPaused(!current.scheduler.paused, current.targetId, current.targetEpoch));
    if (runtime.scheduler.paused) {
      if (wakeTimerRef.current) { clearTimeout(wakeTimerRef.current); wakeTimerRef.current = null; }
      if (debounceTimerRef.current) { clearTimeout(debounceTimerRef.current); debounceTimerRef.current = null; }
    } else {
      queueAutomatic('resume', 0, true);
    }
  }, [queueAutomatic, updateRuntime]);

  const notifySyncConfigurationChanged = useCallback(async () => {
    const runtime = await refreshSyncRuntime();
    if (!runtime.scheduler.paused && await hasCreds()) queueAutomatic('resume', 0, true);
  }, [queueAutomatic, refreshSyncRuntime]);

  return {
    syncRuntime,
    isSyncPaused: syncRuntime?.scheduler.paused ?? false,
    startCoordinator,
    scheduleLocalWrite,
    syncNow,
    toggleSyncPause,
    notifySyncConfigurationChanged,
  };
}
