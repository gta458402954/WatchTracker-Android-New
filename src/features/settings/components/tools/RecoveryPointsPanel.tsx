import type { RecoveryPoint, RecoveryPointList } from '../../../../shared/lib/database';
interface RecoveryPointsPanelProps { recoveryPoints: RecoveryPointList | null; recoveryStatus: string; recoveryBusyId: string | null; onRefresh: () => void; onRestore: (point: RecoveryPoint) => void; onToggleRetention: (point: RecoveryPoint) => void; onDelete: (point: RecoveryPoint) => void; onOpenBackupDirectory: () => void; }
const RECOVERY_REASON_LABELS: Record<RecoveryPoint['reason'], string> = { import: '全量导入前', sync: '同步落盘前', 'batch-metadata': '批量补全前', migration: '数据库迁移前', 'target-migration': '同步目标迁移前', 'episode-history-migration': '逐集历史迁移前', 'collections-migration': '收藏集迁移前', 'series-identity-migration': '系列身份迁移前', 'series-completion': '补充系列条目前', 'pre-restore': '恢复操作前' };
function formatBytes(bytes: number): string { if (bytes < 1024 * 1024) return `${Math.max(1, Math.round(bytes / 1024))} KB`; return `${(bytes / (1024 * 1024)).toFixed(1)} MB`; }
export default function RecoveryPointsPanel({ recoveryPoints, recoveryStatus, recoveryBusyId, onRefresh, onRestore, onToggleRetention, onDelete, onOpenBackupDirectory }: RecoveryPointsPanelProps) {
  return <div aria-label="自动恢复点" className="bg-white rounded-3xl p-6 shadow-sm border border-gray-100 space-y-4">
                <div className="flex items-center justify-between gap-3 border-b border-gray-50 pb-4">
                  <div className="flex items-center gap-2.5">
                    <span className="text-2xl">🛟</span>
                    <div>
                      <h4 className="font-bold text-gray-800">高风险操作自动恢复点</h4>
                      <p className="text-[11px] text-gray-400">导入、同步全量落盘、两条以上批量补全及恢复前自动保存完整 SQLite 状态</p>
                    </div>
                  </div>
                  <button
                    onClick={() => void onRefresh()}
                    className="rounded-lg border border-gray-200 px-3 py-1.5 text-[11px] font-bold text-gray-500 hover:bg-gray-50"
                  >
                    刷新
                  </button>
                </div>

                {recoveryPoints && (
                  <div className="flex items-center justify-between text-[11px] text-gray-500">
                    <span>{recoveryPoints.points.length} 个恢复点 · {formatBytes(recoveryPoints.totalBytes)}</span>
                    <span>自动保留最近 10 个 · 上限 {formatBytes(recoveryPoints.capacityBytes)}</span>
                  </div>
                )}
                <div className="rounded-2xl border border-amber-200 bg-amber-50 px-4 py-3 text-[11px] leading-5 text-amber-800">
                  🔐 新凭据仅保存在当前 Windows 用户的凭据管理器中。迁移前创建的恢复点、手工数据库副本或旧便携目录仍可能含旧格式凭据；程序不会自动删除这些文件，必要时请轮换 WebDAV 密码和 TMDB Key。
                </div>
                {recoveryPoints?.capacityExceeded && (
                  <p className="rounded-xl border border-amber-200 bg-amber-50 px-3 py-2 text-xs text-amber-700">
                    手工保留的恢复点使总容量超过上限；程序不会自动删除手工保留项，请按需清理。
                  </p>
                )}

                <div className="max-h-72 space-y-2 overflow-y-auto pr-1">
                  {recoveryPoints?.points.map(point => (
                    <div
                      key={point.id}
                      aria-label={`恢复点 ${RECOVERY_REASON_LABELS[point.reason]}`}
                      className="rounded-2xl border border-gray-100 bg-gray-50 p-3"
                    >
                      <div className="flex items-start justify-between gap-3">
                        <div className="min-w-0">
                          <p className="text-xs font-bold text-gray-800">
                            {new Date(point.createdAt).toLocaleString('zh-CN')} · {RECOVERY_REASON_LABELS[point.reason]}
                          </p>
                          <p className="mt-1 text-[10px] text-gray-500">
                            V{point.databaseVersion} · {point.recordCount} 条 · {formatBytes(point.sizeBytes)} · {point.integrityOk ? '校验正常' : '校验失败'}
                            {point.retained && ' · 已手工保留'}
                          </p>
                        </div>
                        <span className={`shrink-0 text-[10px] font-bold ${point.integrityOk ? 'text-emerald-600' : 'text-red-500'}`}>
                          {!point.integrityOk ? '不可用' : point.databaseVersion === 18 ? '可恢复' : '仅供迁移回退'}
                        </span>
                      </div>
                      <div className="mt-3 flex flex-wrap gap-2">
                        <button
                          onClick={() => void onRestore(point)}
                          disabled={!point.integrityOk || point.databaseVersion !== 18 || recoveryBusyId !== null}
                          className="rounded-lg bg-indigo-600 px-3 py-1.5 text-[10px] font-bold text-white hover:bg-indigo-700 disabled:bg-gray-300"
                        >
                          恢复
                        </button>
                        <button
                          onClick={() => void onToggleRetention(point)}
                          disabled={recoveryBusyId !== null}
                          className="rounded-lg border border-gray-200 bg-white px-3 py-1.5 text-[10px] font-bold text-gray-600 hover:bg-gray-50 disabled:opacity-50"
                        >
                          {point.retained ? '取消保留' : '手工保留'}
                        </button>
                        <button
                          onClick={() => void onDelete(point)}
                          disabled={recoveryBusyId !== null}
                          className="rounded-lg border border-red-100 bg-white px-3 py-1.5 text-[10px] font-bold text-red-500 hover:bg-red-50 disabled:opacity-50"
                        >
                          删除
                        </button>
                      </div>
                    </div>
                  ))}
                  {recoveryPoints && recoveryPoints.points.length === 0 && (
                    <p className="py-5 text-center text-xs text-gray-400">尚无自动恢复点；首次高风险操作前会自动创建。</p>
                  )}
                </div>

                <button
                  onClick={() => void onOpenBackupDirectory()}
                  className="w-full rounded-xl border border-gray-200 py-2 text-xs font-bold text-gray-600 hover:bg-gray-50"
                >
                  打开 backups 目录
                </button>
                {recoveryStatus && <p className="text-center text-xs font-medium text-indigo-600">{recoveryStatus}</p>}
</div>;
}
