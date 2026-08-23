import { BATCH_METADATA_FIELD_LABELS, type BatchMetadataNoDataState, type TmdbMatch } from '../../../../shared/lib/batchMetadata';
import type { BatchApplyResult, BatchPlanRow } from '../../hooks/batchMetadataModel';
interface BatchMetadataPanelProps { tmdbSaved: boolean; batchNoDataState: BatchMetadataNoDataState; batchPhase: 'idle' | 'planning' | 'preview' | 'applying' | 'done'; batchStatus: string; batchProgress: number; batchTotal: number; batchPlan: BatchPlanRow[]; batchResults: BatchApplyResult[]; batchChoosingId: string | null; batchSyncing: boolean; onClearNoData: () => void; onPrepare: () => void; onCancel: () => void; onApply: (plans?: BatchPlanRow[]) => void; onSelectCandidate: (row: BatchPlanRow, match: TmdbMatch) => void; onReset: () => void; }
export default function BatchMetadataPanel({tmdbSaved, batchNoDataState, batchPhase, batchStatus, batchProgress, batchTotal, batchPlan, batchResults, batchChoosingId, batchSyncing, onClearNoData, onPrepare, onCancel, onApply, onSelectCandidate, onReset }: BatchMetadataPanelProps) {
  return <div className="bg-white rounded-3xl p-6 shadow-sm border border-gray-100 space-y-4">
                <div className="flex items-center gap-2.5 border-b border-gray-50 pb-4">
                  <span className="text-2xl">✨</span>
                  <div>
                    <h4 className="font-bold text-gray-800">一键补全缺失元数据</h4>
                    <p className="text-[11px] text-gray-400">先分析并预览；确认后只写仍然缺失的字段，不覆盖已有数据</p>
                  </div>
                </div>
                <div className="rounded-2xl border border-emerald-100 bg-emerald-50/60 p-4 text-xs leading-5 text-emerald-800">
                  检查 TMDB 可提供的名称、年份、海报、平台、分类、国家、评分、状态、片长/集数及稳定 TMDB 身份。电影补电影 ID，明确分季记录补父剧 ID、季号和季 ID；只填空值且绝不覆盖已有内容，身份冲突会单独阻止。TMDB 已确认没有的数据会按“条目 + IMDb 编号 + 字段”记住，下次不再重复查询；IMDb 编号变化后会重新检查。
                  {Object.keys(batchNoDataState.records).length > 0 && ` 当前已记住 ${Object.keys(batchNoDataState.records).length} 个条目的无数据状态。`}
                </div>
                {Object.keys(batchNoDataState.records).length > 0 && (batchPhase === 'idle' || batchPhase === 'done') && (
                  <button
                    onClick={() => void onClearNoData()}
                    className="w-full rounded-xl border border-gray-200 py-2 text-xs font-bold text-gray-500 hover:bg-gray-50"
                  >
                    清除 TMDB 无数据记忆并允许重新检查
                  </button>
                )}
                {(batchPhase === 'idle' || batchPhase === 'done') && (
                  <button
                    onClick={onPrepare}
                    disabled={!tmdbSaved}
                    className="w-full py-3 rounded-xl bg-amber-500 hover:bg-amber-600 disabled:bg-gray-200 text-white text-sm font-bold transition-colors flex items-center justify-center gap-2 shadow-sm"
                    title={!tmdbSaved ? "请先在【基础配置】中设置 TMDB 密钥" : ""}
                  >
                    🔎 分析并预览缺失字段
                  </button>
                )}
                {batchSyncing && batchTotal > 0 && (
                  <div className="mt-3 space-y-2">
                    <div className="w-full bg-gray-100 rounded-full h-2">
                      <div className="bg-amber-500 h-2 rounded-full transition-all duration-300" style={{ width: `${(batchProgress / batchTotal) * 100}%` }}></div>
                    </div>
                    <p className="text-[10px] text-center text-gray-400">进度：{batchProgress} / {batchTotal}</p>
                    <button
                      onClick={onCancel}
                      className="w-full rounded-xl border border-gray-200 py-2 text-xs font-bold text-gray-600 hover:bg-gray-50"
                    >
                      安全停止
                    </button>
                  </div>
                )}
                {batchStatus && <p className="text-xs text-center text-amber-600 font-bold mt-1">{batchStatus}</p>}

                {batchPhase === 'preview' && (
                  <div aria-label="元数据补全预览" className="space-y-3 rounded-2xl border border-gray-200 bg-gray-50 p-4">
                    <div className="flex items-center justify-between gap-3">
                      <h5 className="text-sm font-black text-gray-800">写入预览</h5>
                      <span className="text-[10px] text-gray-500">确认时会再次检查字段是否仍缺失</span>
                    </div>
                    <div className="max-h-64 space-y-2 overflow-y-auto pr-1">
                      {batchPlan.map(row => (
                        <div key={row.recordId} className="rounded-xl border border-gray-100 bg-white px-3 py-2">
                          <div className="flex items-start justify-between gap-3">
                            <p className="min-w-0 truncate text-xs font-bold text-gray-800">{row.recordName}</p>
                            <span className={`shrink-0 text-[10px] font-bold ${row.status === 'ready' ? 'text-emerald-600' : row.status === 'choice' ? 'text-amber-600' : row.status === 'failed' ? 'text-red-500' : 'text-gray-400'}`}>
                              {row.status === 'ready' ? '可更新' : row.status === 'choice' ? '待选择' : row.status === 'failed' ? '失败' : '跳过'}
                            </span>
                          </div>
                          <p className="mt-1 text-[10px] text-gray-500">
                            {row.fields.length
                              ? row.fields.map(field => BATCH_METADATA_FIELD_LABELS[field]).join('、')
                              : row.reason}
                          </p>
                          {row.noDataFields && row.noDataFields.length > 0 && (
                            <p className="mt-1 text-[10px] text-amber-600">TMDB 无数据：{row.noDataFields.map(field => BATCH_METADATA_FIELD_LABELS[field]).join('、')}（下次不再查询）</p>
                          )}
                          {row.identityConflict && <p className="mt-1 text-[10px] font-semibold text-red-500">身份冲突：{row.identityConflict}（普通缺失字段仍可补充）</p>}
                          {row.status === 'choice' && row.candidates && (
                            <div className="mt-2 space-y-1.5">
                              {row.candidates.map(candidate => (
                                <button
                                  key={`${candidate.match.type}:${candidate.match.id}`}
                                  onClick={() => void onSelectCandidate(row, candidate.match)}
                                  disabled={batchChoosingId === row.recordId}
                                  className="block w-full rounded-lg border border-amber-200 bg-amber-50 px-2.5 py-2 text-left text-[10px] font-semibold text-amber-900 hover:bg-amber-100 disabled:opacity-50"
                                >
                                  {batchChoosingId === row.recordId ? '正在读取所选条目…' : candidate.label}
                                </button>
                              ))}
                            </div>
                          )}
                          {row.remoteIdentity && <p className="mt-1 font-mono text-[9px] text-gray-300">{row.remoteIdentity}</p>}
                        </div>
                      ))}
                    </div>
                    <div className="flex gap-2">
                      <button
                        onClick={() => void onApply()}
                        disabled={!batchPlan.some(row => row.status === 'ready')}
                        className="flex-1 rounded-xl bg-emerald-600 py-2.5 text-xs font-bold text-white hover:bg-emerald-700 disabled:bg-gray-300"
                      >
                        确认写入 {batchPlan.filter(row => row.status === 'ready').length} 条
                      </button>
                      <button onClick={onReset} className="rounded-xl border border-gray-200 px-4 py-2.5 text-xs font-bold text-gray-600 hover:bg-white">
                        取消
                      </button>
                    </div>
                    {batchPlan.some(row => row.status === 'failed') && (
                      <button
                        onClick={() => void onPrepare()}
                        className="w-full rounded-xl border border-red-200 bg-white py-2 text-xs font-bold text-red-600 hover:bg-red-50"
                      >
                        重新分析全部候选
                      </button>
                    )}
                  </div>
                )}

                {batchPhase === 'done' && batchResults.length > 0 && (
                  <div aria-label="元数据补全结果" className="space-y-3 rounded-2xl border border-gray-200 bg-gray-50 p-4">
                    <h5 className="text-sm font-black text-gray-800">逐条结果</h5>
                    <div className="max-h-56 space-y-2 overflow-y-auto pr-1">
                      {batchResults.map(result => (
                        <div key={result.plan.recordId} className="flex items-start justify-between gap-3 rounded-xl bg-white px-3 py-2 text-xs">
                          <div className="min-w-0">
                            <p className="truncate font-bold text-gray-800">{result.plan.recordName}</p>
                            <p className="mt-1 text-[10px] text-gray-500">
                              {result.status === 'updated'
                                ? `${result.plan.fields.map(field => BATCH_METADATA_FIELD_LABELS[field]).join('、')}${result.reason ? `；${result.reason}` : ''}`
                                : result.reason}
                            </p>
                          </div>
                          <span className={`shrink-0 text-[10px] font-bold ${result.status === 'updated' ? 'text-emerald-600' : result.status === 'failed' ? 'text-red-500' : 'text-gray-400'}`}>
                            {result.status === 'updated' ? '已更新' : result.status === 'failed' ? '失败' : '已跳过'}
                          </span>
                        </div>
                      ))}
                    </div>
                    {batchResults.some(result => result.status === 'failed') && (
                      <button
                        onClick={() => void onApply(batchResults.filter(result => result.status === 'failed').map(result => result.plan))}
                        className="w-full rounded-xl border border-red-200 bg-white py-2 text-xs font-bold text-red-600 hover:bg-red-50"
                      >
                        重试失败项
                      </button>
                    )}
                    <button onClick={onReset} className="w-full rounded-xl border border-gray-200 bg-white py-2 text-xs font-bold text-gray-600 hover:bg-gray-50">
                      清除结果
                    </button>
                  </div>
                )}

  </div>;
}
