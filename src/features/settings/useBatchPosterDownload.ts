import { useCallback, useRef, useState } from 'react';
import type { WatchRecord } from '../../shared/types';
import { downloadPosterAsync } from '../../shared/lib/database';
import { displayTitlesOf } from '../../shared/lib/displayTitle';

export interface BatchPosterProgress {
  status: 'idle' | 'running' | 'completed' | 'cancelled';
  total: number;
  current: number;
  downloaded: number;
  cached: number;
  failed: number;
  currentTitle: string;
}

export function useBatchPosterDownload(records: WatchRecord[]) {
  const [progress, setProgress] = useState<BatchPosterProgress>({
    status: 'idle',
    total: 0,
    current: 0,
    downloaded: 0,
    cached: 0,
    failed: 0,
    currentTitle: '',
  });

  const cancelledRef = useRef(false);

  const startDownload = useCallback(async () => {
    // 收集所有带 posterPath 的唯一海报路径
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

    const items = Array.from(uniquePosters.values());
    if (items.length === 0) {
      setProgress({
        status: 'completed',
        total: 0,
        current: 0,
        downloaded: 0,
        cached: 0,
        failed: 0,
        currentTitle: '',
      });
      return;
    }

    cancelledRef.current = false;
    let current = 0;
    let downloaded = 0;
    let cached = 0;
    let failed = 0;

    setProgress({
      status: 'running',
      total: items.length,
      current: 0,
      downloaded: 0,
      cached: 0,
      failed: 0,
      currentTitle: items[0]?.title || '',
    });

    for (const item of items) {
      if (cancelledRef.current) {
        setProgress(prev => ({ ...prev, status: 'cancelled' }));
        return;
      }

      setProgress(prev => ({
        ...prev,
        current,
        currentTitle: item.title,
      }));

      try {
        const res = await downloadPosterAsync(item.posterPath, 'w342');
        if (res.status === 'downloaded') {
          downloaded++;
        } else {
          cached++;
        }
      } catch {
        failed++;
      }

      current++;
      setProgress({
        status: 'running',
        total: items.length,
        current,
        downloaded,
        cached,
        failed,
        currentTitle: item.title,
      });
    }

    setProgress({
      status: 'completed',
      total: items.length,
      current,
      downloaded,
      cached,
      failed,
      currentTitle: '',
    });
  }, [records]);

  const cancelDownload = useCallback(() => {
    cancelledRef.current = true;
    setProgress(prev => ({ ...prev, status: 'cancelled' }));
  }, []);

  const reset = useCallback(() => {
    cancelledRef.current = false;
    setProgress({
      status: 'idle',
      total: 0,
      current: 0,
      downloaded: 0,
      cached: 0,
      failed: 0,
      currentTitle: '',
    });
  }, []);

  return {
    progress,
    startDownload,
    cancelDownload,
    reset,
  };
}
