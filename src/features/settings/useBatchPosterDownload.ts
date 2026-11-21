import { useCallback, useEffect, useRef, useState } from 'react';
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

export function useBatchPosterDownload(records: WatchRecord[]) {
  const [progress, setProgress] = useState<BatchPosterProgress>({
    status: 'idle',
    total: 0,
    missingTotal: 0,
    current: 0,
    downloaded: 0,
    cached: 0,
    failed: 0,
    currentTitle: '',
  });

  const [counts, setCounts] = useState<PosterCounts | null>(null);
  const cancelledRef = useRef(false);

  // 提取所有唯一海报条目
  const getUniquePosters = useCallback(() => {
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
    return uniquePosters;
  }, [records]);

  // 刷新片库本地海报完整度统计
  const refreshCounts = useCallback(async () => {
    const uniqueMap = getUniquePosters();
    const uniquePaths = Array.from(uniqueMap.keys());
    if (uniquePaths.length === 0) {
      setCounts({ total: 0, missing: 0, cached: 0 });
      return;
    }
    try {
      const missing = await getMissingPostersAsync(uniquePaths, 'w342');
      const missingCount = missing.length;
      const cachedCount = uniquePaths.length - missingCount;
      setCounts({ total: uniquePaths.length, missing: missingCount, cached: cachedCount });
    } catch {
      setCounts({ total: uniquePaths.length, missing: uniquePaths.length, cached: 0 });
    }
  }, [getUniquePosters]);

  useEffect(() => {
    // Checking crosses the Rust IPC boundary; counts are updated after awaited read.
    // eslint-disable-next-line react-hooks/set-state-in-effect
    void refreshCounts();
  }, [refreshCounts]);

  const startDownload = useCallback(async () => {
    const uniqueMap = getUniquePosters();
    const allItems = Array.from(uniqueMap.values());
    if (allItems.length === 0) {
      setProgress({
        status: 'completed',
        total: 0,
        missingTotal: 0,
        current: 0,
        downloaded: 0,
        cached: 0,
        failed: 0,
        currentTitle: '',
      });
      setCounts({ total: 0, missing: 0, cached: 0 });
      return;
    }

    cancelledRef.current = false;

    // 核心优化：直接查询本地已有海报，毫秒级跳过已存在的封面，仅补充缺失海报
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
      // 本地已有的海报直接跳过，全部已就绪
      setProgress({
        status: 'completed',
        total: allItems.length,
        missingTotal: 0,
        current: allItems.length,
        downloaded: 0,
        cached: cachedCount,
        failed: 0,
        currentTitle: '',
      });
      setCounts({ total: allItems.length, missing: 0, cached: allItems.length });
      return;
    }

    let current = 0;
    let downloaded = 0;
    let failed = 0;

    setProgress({
      status: 'running',
      total: allItems.length,
      missingTotal: missingItems.length,
      current: 0,
      downloaded: 0,
      cached: cachedCount,
      failed: 0,
      currentTitle: missingItems[0]?.title || '',
    });

    for (const item of missingItems) {
      if (cancelledRef.current) {
        setProgress(prev => ({ ...prev, status: 'cancelled' }));
        void refreshCounts();
        return;
      }

      setProgress(prev => ({
        ...prev,
        current,
        currentTitle: item.title,
      }));

      try {
        const res = await downloadPosterAsync(item.posterPath, 'w342');
        if (res.status === 'downloaded' || res.status === 'cache_hit') {
          downloaded++;
        }
      } catch {
        failed++;
      }

      current++;
      setProgress({
        status: 'running',
        total: allItems.length,
        missingTotal: missingItems.length,
        current,
        downloaded,
        cached: cachedCount,
        failed,
        currentTitle: item.title,
      });
    }

    setProgress({
      status: 'completed',
      total: allItems.length,
      missingTotal: missingItems.length,
      current: allItems.length,
      downloaded,
      cached: cachedCount,
      failed,
      currentTitle: '',
    });

    void refreshCounts();
  }, [getUniquePosters, refreshCounts]);

  const cancelDownload = useCallback(() => {
    cancelledRef.current = true;
    setProgress(prev => ({ ...prev, status: 'cancelled' }));
  }, []);

  const reset = useCallback(() => {
    cancelledRef.current = false;
    setProgress({
      status: 'idle',
      total: 0,
      missingTotal: 0,
      current: 0,
      downloaded: 0,
      cached: 0,
      failed: 0,
      currentTitle: '',
    });
    void refreshCounts();
  }, [refreshCounts]);

  return {
    progress,
    counts,
    refreshCounts,
    startDownload,
    cancelDownload,
    reset,
  };
}
