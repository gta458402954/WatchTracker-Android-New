import { useCallback, useEffect, useSyncExternalStore } from 'react';
import type { WatchRecord } from '../../shared/types';
import { downloadPosterAsync, getMissingPostersAsync } from '../../shared/lib/database';
import { displayTitlesOf } from '../../shared/lib/displayTitle';

export interface BatchPosterProgress {
  status: 'idle' | 'running' | 'completed' | 'cancelled';
  total: number;
  missingTotal: number;
  current: number;
  downloaded: number;
  cached: number;
  failed: number;
  currentTitle: string;
}

export interface PosterCounts {
  total: number;
  missing: number;
  cached: number;
}

// 模块级单例状态：组件卸载或切页时后台任务不中断，切回设置页可无缝恢复进度
let globalProgress: BatchPosterProgress = {
  status: 'idle',
  total: 0,
  missingTotal: 0,
  current: 0,
  downloaded: 0,
  cached: 0,
  failed: 0,
  currentTitle: '',
};

let globalCounts: PosterCounts | null = null;
let isCancelled = false;
let isRunning = false;
let activeDownloadPromise: Promise<void> | null = null;

const progressListeners = new Set<() => void>();
const countsListeners = new Set<() => void>();

function emitProgress() {
  for (const listener of progressListeners) listener();
}

function emitCounts() {
  for (const listener of countsListeners) listener();
}

export function subscribeProgress(listener: () => void) {
  progressListeners.add(listener);
  return () => {
    progressListeners.delete(listener);
  };
}

export function subscribeCounts(listener: () => void) {
  countsListeners.add(listener);
  return () => {
    countsListeners.delete(listener);
  };
}

export function getProgressSnapshot(): BatchPosterProgress {
  return globalProgress;
}

export function getCountsSnapshot(): PosterCounts | null {
  return globalCounts;
}

export function isPosterDownloadRunning(): boolean {
  return isRunning;
}

// 刷新片库本地海报完整度统计
export async function refreshPosterCounts(records: WatchRecord[]): Promise<PosterCounts> {
  const uniquePaths: string[] = [];
  for (const r of records) {
    if (r.posterPath && r.posterPath.trim() && !uniquePaths.includes(r.posterPath)) {
      uniquePaths.push(r.posterPath);
    }
  }
  if (uniquePaths.length === 0) {
    globalCounts = { total: 0, missing: 0, cached: 0 };
    emitCounts();
    return globalCounts;
  }
  try {
    const missing = await getMissingPostersAsync(uniquePaths, 'w342');
    const missingCount = missing.length;
    const cachedCount = uniquePaths.length - missingCount;
    globalCounts = { total: uniquePaths.length, missing: missingCount, cached: cachedCount };
  } catch {
    globalCounts = { total: uniquePaths.length, missing: uniquePaths.length, cached: 0 };
  }
  emitCounts();
  return globalCounts;
}

// 核心优化：直接查询本地已有海报，毫秒级跳过已存在的封面，仅补充缺失海报
export async function startBatchPosterDownload(
  records: WatchRecord[],
  onNotice?: (type: 'success' | 'warning' | 'error', msg: string) => void
): Promise<void> {
  if (isRunning) {
    return activeDownloadPromise ?? Promise.resolve();
  }

  const uniquePosters = new Map<string, { posterPath: string; title: string }>();
  for (const r of records) {
    if (r.posterPath && r.posterPath.trim()) {
      if (!uniquePosters.has(r.posterPath)) {
        const titles = displayTitlesOf(r);
        uniquePosters.set(r.posterPath, {
          posterPath: r.posterPath,
          title: titles.primary || titles.secondary || r.originalName || '未命名影片',
        });
      }
    }
  }

  const allItems = Array.from(uniquePosters.values());
  if (allItems.length === 0) {
    globalProgress = {
      status: 'completed',
      total: 0,
      missingTotal: 0,
      current: 0,
      downloaded: 0,
      cached: 0,
      failed: 0,
      currentTitle: '',
    };
    globalCounts = { total: 0, missing: 0, cached: 0 };
    emitProgress();
    emitCounts();
    return;
  }

  isCancelled = false;
  isRunning = true;

  const downloadTask = (async () => {
    let missingPaths: string[];
    try {
      missingPaths = await getMissingPostersAsync(allItems.map(it => it.posterPath), 'w342');
    } catch {
      missingPaths = allItems.map(it => it.posterPath);
    }

    const missingSet = new Set(missingPaths);
    const missingItems = allItems.filter(it => missingSet.has(it.posterPath));
    const cachedCount = allItems.length - missingItems.length;

    if (missingItems.length === 0) {
      globalProgress = {
        status: 'completed',
        total: allItems.length,
        missingTotal: 0,
        current: allItems.length,
        downloaded: 0,
        cached: cachedCount,
        failed: 0,
        currentTitle: '',
      };
      globalCounts = { total: allItems.length, missing: 0, cached: allItems.length };
      isRunning = false;
      activeDownloadPromise = null;
      emitProgress();
      emitCounts();
      return;
    }

    let current = 0;
    let downloaded = 0;
    let failed = 0;

    globalProgress = {
      status: 'running',
      total: allItems.length,
      missingTotal: missingItems.length,
      current: 0,
      downloaded: 0,
      cached: cachedCount,
      failed: 0,
      currentTitle: missingItems[0]?.title || '',
    };
    emitProgress();

    for (const item of missingItems) {
      if (isCancelled) {
        globalProgress = {
          ...globalProgress,
          status: 'cancelled',
        };
        isRunning = false;
        activeDownloadPromise = null;
        emitProgress();
        void refreshPosterCounts(records);
        return;
      }

      globalProgress = {
        ...globalProgress,
        current,
        currentTitle: item.title,
      };
      emitProgress();

      try {
        const res = await downloadPosterAsync(item.posterPath, 'w342');
        if (res.status === 'downloaded' || res.status === 'cache_hit') {
          downloaded++;
        }
      } catch {
        failed++;
      }

      current++;
      globalProgress = {
        ...globalProgress,
        current,
        downloaded,
        cached: cachedCount,
        failed,
        currentTitle: item.title,
      };
      emitProgress();
    }

    globalProgress = {
      status: 'completed',
      total: allItems.length,
      missingTotal: missingItems.length,
      current: allItems.length,
      downloaded,
      cached: cachedCount,
      failed,
      currentTitle: '',
    };
    isRunning = false;
    activeDownloadPromise = null;
    emitProgress();

    if (onNotice && downloaded > 0) {
      onNotice('success', `海报补充完成！已成功下载 ${downloaded} 张封面。`);
    }

    void refreshPosterCounts(records);
  })();

  activeDownloadPromise = downloadTask;
  return downloadTask;
}

export function cancelBatchPosterDownload(): void {
  if (isRunning) {
    isCancelled = true;
  }
}

export function resetBatchPosterDownload(records: WatchRecord[]): void {
  isCancelled = false;
  isRunning = false;
  activeDownloadPromise = null;
  globalProgress = {
    status: 'idle',
    total: 0,
    missingTotal: 0,
    current: 0,
    downloaded: 0,
    cached: 0,
    failed: 0,
    currentTitle: '',
  };
  emitProgress();
  void refreshPosterCounts(records);
}

export function useBatchPosterDownload(
  records: WatchRecord[],
  onNotice?: (type: 'success' | 'warning' | 'error', msg: string) => void
) {
  const progress = useSyncExternalStore(subscribeProgress, getProgressSnapshot);
  const counts = useSyncExternalStore(subscribeCounts, getCountsSnapshot);

  useEffect(() => {
    if (globalProgress.status === 'idle' && globalCounts === null) {
      void refreshPosterCounts(records);
    }
  }, [records]);

  const startDownload = useCallback(() => {
    return startBatchPosterDownload(records, onNotice);
  }, [records, onNotice]);

  const cancelDownload = useCallback(() => {
    cancelBatchPosterDownload();
  }, []);

  const reset = useCallback(() => {
    resetBatchPosterDownload(records);
  }, [records]);

  const refreshCounts = useCallback(() => {
    return refreshPosterCounts(records);
  }, [records]);

  return {
    progress,
    counts,
    refreshCounts,
    startDownload,
    cancelDownload,
    reset,
  };
}
