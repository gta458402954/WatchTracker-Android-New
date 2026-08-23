import { useCallback, useState } from 'react';
import { cleanPosterCache, getPosterCacheStats, type PosterCacheStats } from '../../../shared/lib/database';

interface Options {
  showFailure: (scope: string, action: string, error: unknown, setStatus?: (message: string) => void) => void;
  showSuccess: (message: string) => void;
}

export function usePosterCache({ showFailure, showSuccess }: Options) {
  const [posterCache, setPosterCache] = useState<PosterCacheStats | null>(null);
  const [posterCacheStatus, setPosterCacheStatus] = useState('');
  const [posterCacheBusy, setPosterCacheBusy] = useState(false);
  const refreshPosterCache = useCallback(async () => {
    try { setPosterCache(await getPosterCacheStats()); }
    catch (error) { showFailure('Settings.PosterCacheStats', '读取海报缓存', error, setPosterCacheStatus); }
  }, [showFailure]);
  const handleCleanPosterCache = useCallback(async (mode: 'unreferenced' | 'all') => {
    const prompt = mode === 'all'
      ? '确定清空全部海报缓存吗？条目数据不会改变，海报将在需要时重新下载。'
      : '确定清理临时、无效和未被任何条目引用的海报缓存吗？';
    if (!confirm(prompt)) return;
    setPosterCacheBusy(true);
    try {
      const next = await cleanPosterCache(mode);
      setPosterCache(next);
      const message = mode === 'all' ? '全部海报缓存已清空，条目数据未改变。' : '未引用海报缓存已清理。';
      setPosterCacheStatus(`✅ ${message}`); showSuccess(message);
    } catch (error) { showFailure('Settings.CleanPosterCache', '清理海报缓存', error, setPosterCacheStatus); }
    finally { setPosterCacheBusy(false); }
  }, [showFailure, showSuccess]);
  return { posterCache, posterCacheStatus, posterCacheBusy, refreshPosterCache, handleCleanPosterCache };
}
