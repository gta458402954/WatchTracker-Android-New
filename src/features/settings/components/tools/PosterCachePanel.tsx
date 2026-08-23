import type { PosterCacheStats } from '../../../../shared/lib/database';
interface PosterCachePanelProps { posterCache: PosterCacheStats | null; posterCacheStatus: string; posterCacheBusy: boolean; onRefresh: () => void; onClean: (mode: 'unreferenced' | 'all') => void; }
function formatBytes(bytes: number): string { if (bytes < 1024 * 1024) return `${Math.max(1, Math.round(bytes / 1024))} KB`; return `${(bytes / (1024 * 1024)).toFixed(1)} MB`; }
export default function PosterCachePanel({ posterCache, posterCacheStatus, posterCacheBusy, onRefresh, onClean }: PosterCachePanelProps) {
  return <div aria-label="海报缓存" className="bg-white rounded-3xl p-6 shadow-sm border border-gray-100 space-y-4">
                <div className="flex items-center justify-between gap-3 border-b border-gray-50 pb-4">
                  <div className="flex items-center gap-2.5">
                    <span className="text-2xl">🖼️</span>
                    <div>
                      <h4 className="font-bold text-gray-800">海报缓存</h4>
                      <p className="text-[11px] text-gray-400">缓存可安全重建；自动清理永不删除仍被条目引用的海报</p>
                    </div>
                  </div>
                  <button
                    onClick={() => void onRefresh()}
                    disabled={posterCacheBusy}
                    className="rounded-lg border border-gray-200 px-3 py-1.5 text-[11px] font-bold text-gray-500 hover:bg-gray-50 disabled:opacity-50"
                  >
                    刷新
                  </button>
                </div>
                {posterCache && (
                  <div className="grid grid-cols-2 gap-2 text-[11px] text-gray-600">
                    <span>总容量：{formatBytes(posterCache.totalBytes)}</span>
                    <span>建议上限：{formatBytes(posterCache.capacityBytes)}</span>
                    <span>条目引用：{posterCache.referencedCount} 张</span>
                    <span>未引用：{posterCache.orphanCount} 张</span>
                    <span>有效缓存：{posterCache.validCount} 张</span>
                    <span>无效/临时：{posterCache.invalidCount + posterCache.temporaryCount} 个</span>
                  </div>
                )}
                {posterCache?.capacityExceeded && (
                  <p className="rounded-xl border border-amber-200 bg-amber-50 px-3 py-2 text-xs text-amber-700">
                    缓存超过建议容量。程序只会自动回收未引用文件，不会删除条目仍在使用的海报。
                  </p>
                )}
                <div className="flex gap-3">
                  <button
                    onClick={() => void onClean('unreferenced')}
                    disabled={posterCacheBusy}
                    className="flex-1 rounded-xl border border-gray-200 py-2.5 text-xs font-bold text-gray-600 hover:bg-gray-50 disabled:opacity-50"
                  >
                    清理未引用缓存
                  </button>
                  <button
                    onClick={() => void onClean('all')}
                    disabled={posterCacheBusy}
                    className="flex-1 rounded-xl border border-red-100 py-2.5 text-xs font-bold text-red-500 hover:bg-red-50 disabled:opacity-50"
                  >
                    清空全部海报
                  </button>
                </div>
                {posterCacheStatus && <p className="text-center text-xs font-medium text-indigo-600">{posterCacheStatus}</p>}
</div>;
}
