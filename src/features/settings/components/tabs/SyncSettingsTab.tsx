import type { SyncConflict } from '../../../../shared/lib/webdav';
import type { SyncRuntimeState, SyncTargetDescriptor, SyncTargetRegistry } from '../../../../shared/lib/database';
import type { WebDavTargetDisplay } from '../../../../shared/lib/webdavDisplay';
import { formatWebDavTargetUrl } from '../../../../shared/lib/webdavDisplay';

interface SyncSettingsTabProps {
  username: string; password: string; webdavUrl: string; saved: boolean; editingTarget: boolean;
  targetRegistry: SyncTargetRegistry | null; activeTarget?: SyncTargetDescriptor; activeTargetDisplay: WebDavTargetDisplay; automaticSyncPaused: boolean;
  syncStatus: string; importStatus: string; syncConflicts: SyncConflict[];
  localInterval: number; localPullInterval: number; syncRuntime: SyncRuntimeState | null;
  onUsernameChange: (value: string) => void; onPasswordChange: (value: string) => void; onWebdavUrlChange: (value: string) => void;
  onEditingTargetChange: (value: boolean) => void; onSave: () => void; onClear: () => void; onSync: () => void; onImport: () => void;
  onImportLegacyChanges: () => void; onResolveConflict: (conflict: SyncConflict, resolution: 'local' | 'remote' | 'keep' | 'delete') => void;
  onLocalIntervalChange: (value: number) => void; onSaveInterval: () => void; onLocalPullIntervalChange: (value: number) => void; onSavePullInterval: () => void;
}

const SYNC_FIELD_LABELS: Record<string, string> = {
  originalName: '原名', chineseName: '中文名', progress: '进度', totalEpisodes: '总集数', status: '状态', platform: '平台', rating: '个人评分', startDate: '开始日期', endDate: '完成日期', notes: '备注', movieProgress: '电影进度', movieDuration: '电影时长', releaseYear: '年份', posterPath: '海报', imdbId: 'IMDb 编号', isLocked: '锁定状态', genres: '题材', originCountry: '国家/地区', imdbRating: 'IMDb 评分', tmdbStatus: 'TMDB 状态', interestLevel: '兴趣等级', episodeRuntime: '单集时长', mediaType: '内容类型', contentTags: '内容标签', record: '整条记录', 'legacy-import': '旧版数据差异',
};

export default function SyncSettingsTab({username, password, webdavUrl, saved, editingTarget, targetRegistry, activeTarget, activeTargetDisplay, automaticSyncPaused, syncStatus, importStatus, syncConflicts, localInterval, localPullInterval, syncRuntime, onUsernameChange, onPasswordChange, onWebdavUrlChange, onEditingTargetChange, onSave, onClear, onSync, onImport, onImportLegacyChanges, onResolveConflict, onLocalIntervalChange, onSaveInterval, onLocalPullIntervalChange, onSavePullInterval }: SyncSettingsTabProps) {
  return <div className="space-y-6 animate-fade-in animate-duration-200">
              <div>
                <h3 className="text-2xl font-black text-gray-900">☁️ 云端同步</h3>
                <p className="text-xs text-gray-400 mt-1">通过 HTTPS 与坚果云等 WebDAV 服务同步影视记录</p>
              </div>

              {/* WebDAV Settings */}
              <div className="bg-white rounded-3xl p-6 shadow-sm border border-gray-100 space-y-4">
                <div className="flex items-center justify-between border-b border-gray-50 pb-4">
                  <div className="flex items-center gap-2.5">
                    <span className="text-2xl">☁️</span>
                    <div>
                      <h4 className="font-bold text-gray-800">WebDAV 同步</h4>
                      <p className="text-[11px] text-gray-400">用于备份或同步影视记录数据</p>
                    </div>
                  </div>
                  {saved && (
                    <span className="text-xs px-2.5 py-0.5 bg-green-50 text-green-700 border border-green-200 rounded-full font-semibold">
                      受 Windows 保护
                    </span>
                  )}
                </div>

                {targetRegistry && targetRegistry.targets.length > 0 && (
                  <div className="rounded-2xl border border-gray-100 bg-gray-50 p-3 space-y-2">
                    <p className="text-[11px] font-bold text-gray-500">已保存目标（共用本地影视库，远端状态相互隔离）</p>
                    {targetRegistry.targets.map(target => {
                      const display = formatWebDavTargetUrl(target.normalizedUrl);
                      return (
                        <div key={target.id} className="flex min-w-0 items-start justify-between gap-3 rounded-xl bg-white/70 px-3 py-2 text-xs text-gray-600">
                          <div className="min-w-0 flex-1">
                            <p className="truncate font-semibold text-gray-700" title={target.username}>{target.username}</p>
                            <p className="break-words text-gray-500" title={display.safeUrl}>{display.summary}</p>
                          </div>
                          <span className={`shrink-0 rounded-full px-2 py-0.5 ${target.id === targetRegistry.activeTargetId ? 'bg-green-100 text-green-700' : 'bg-gray-200 text-gray-500'}`}>
                            {target.id === targetRegistry.activeTargetId ? '当前' : target.id.slice(0, 8)}
                          </span>
                        </div>
                      );
                    })}
                  </div>
                )}

                {!saved || editingTarget ? (
                  <div className="space-y-3">
                    <input
                      type="text"
                      placeholder="WebDAV 服务器地址"
                      value={webdavUrl}
                      onChange={(e) => onWebdavUrlChange(e.target.value)}
                      className="w-full px-4 py-2.5 text-sm border border-gray-200 rounded-xl focus:outline-none focus:ring-2 focus:ring-indigo-500 focus:border-transparent transition-all"
                    />
                    <div className="grid grid-cols-1 md:grid-cols-2 gap-3">
                      <input
                        type="text"
                        placeholder="用户名"
                        value={username}
                        onChange={(e) => onUsernameChange(e.target.value)}
                        className="px-4 py-2.5 text-sm border border-gray-200 rounded-xl focus:outline-none focus:ring-2 focus:ring-indigo-500 focus:border-transparent transition-all"
                      />
                      <input
                        type="password"
                        placeholder="WebDAV 密码 / 应用密码"
                        value={password}
                        onChange={(e) => onPasswordChange(e.target.value)}
                        className="px-4 py-2.5 text-sm border border-gray-200 rounded-xl focus:outline-none focus:ring-2 focus:ring-indigo-500 focus:border-transparent transition-all"
                      />
                    </div>
                    <p className="text-[10px] text-gray-400">
                      💡 默认使用坚果云。若使用自定义 WebDAV 服务，请确保填入完整的文件夹 URL（例如：https://dav.example.com/dav/影视追踪/）。
                    </p>
                    <button
                      onClick={onSave}
                      className="py-2.5 px-6 rounded-xl bg-indigo-600 hover:bg-indigo-700 text-white text-sm font-semibold transition-colors disabled:opacity-50"
                      disabled={!username.trim() || !password.trim() || !webdavUrl.trim()}
                    >
                      {saved ? '只读检查并更新目标' : '只读检查并连接'}
                    </button>
                    {saved && (
                      <button onClick={() => { onEditingTargetChange(false); onPasswordChange(''); }} className="ml-3 py-2.5 px-4 rounded-xl border border-gray-200 text-sm font-semibold text-gray-600">
                        取消
                      </button>
                    )}
                  </div>
                ) : (
                  <div className="space-y-4">
                    <div className="min-w-0 space-y-4 rounded-2xl bg-gray-50 p-4">
                      <div className="flex min-w-0 items-start gap-3">
                        <svg className="mt-0.5 h-5 w-5 shrink-0 text-green-600" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                          <path strokeLinecap="round" strokeLinejoin="round" strokeWidth={2} d="M5 13l4 4L19 7" />
                        </svg>
                        <div className="min-w-0 flex-1">
                          <p className="text-sm font-bold text-gray-800">WebDAV 已连接</p>
                          <p className="mt-1 break-words text-xs text-gray-500" title={activeTargetDisplay.safeUrl}>
                            当前目标：{activeTarget?.username ?? username} · {activeTargetDisplay.summary}
                          </p>
                          <p className={`mt-1 text-xs font-medium ${automaticSyncPaused ? 'text-amber-600' : 'text-green-600'}`}>
                            自动同步：{automaticSyncPaused ? '已暂停' : '已开启'}
                          </p>
                        </div>
                      </div>
                      <div className="flex flex-wrap gap-2 border-t border-gray-200 pt-3">
                        <button onClick={() => onEditingTargetChange(true)} className="rounded-lg border border-indigo-200 bg-white px-3 py-2 text-xs font-bold text-indigo-600 transition-colors hover:bg-indigo-50">
                          切换或更新凭据
                        </button>
                        <button onClick={onClear} className="rounded-lg border border-red-200 bg-white px-3 py-2 text-xs font-bold text-red-500 transition-colors hover:bg-red-50">
                          断开连接
                        </button>
                      </div>
                      <p className="text-[11px] leading-relaxed text-gray-400">断开只移除当前连接，已保存的远端状态和未处理冲突仍会保留。</p>
                    </div>
                    <div className="flex gap-3">
                      <button
                        onClick={onSync}
                        className="flex-1 py-2.5 rounded-xl bg-indigo-600 hover:bg-indigo-700 text-white text-sm font-semibold transition-colors shadow-sm"
                      >
                        ☁️ 立即同步到云端
                      </button>
                      <button
                        onClick={onImport}
                        className="flex-1 py-2.5 rounded-xl bg-green-600 hover:bg-green-700 text-white text-sm font-semibold transition-colors shadow-sm"
                      >
                        📥 从云端导入覆盖
                      </button>
                    </div>
                  </div>
                )}
                {syncStatus && <p className="text-xs text-center text-indigo-600 font-medium mt-1">{syncStatus}</p>}
                {importStatus && <p className="text-xs text-center text-green-600 font-medium mt-1">{importStatus}</p>}
                <button
                  onClick={() => void onImportLegacyChanges()}
                  className="w-full rounded-xl border border-amber-200 bg-amber-50 py-2 text-xs font-bold text-amber-700 hover:bg-amber-100"
                >
                  检查并导入旧版 records.json 差异
                </button>
              </div>

              {/* Conflict history */}
              <div className="bg-white rounded-3xl p-6 shadow-sm border border-gray-100 space-y-4">
                <div className="flex items-center justify-between border-b border-gray-50 pb-4">
                  <div className="flex items-center gap-2.5">
                    <span className="text-2xl">⚖️</span>
                    <div>
                      <h4 className="font-bold text-gray-800">待处理同步冲突</h4>
                      <p className="text-[11px] text-gray-400">不同字段会自动合并；同字段或删除冲突需要明确选择</p>
                    </div>
                  </div>
                </div>
                {syncConflicts.length === 0 ? (
                  <p className="py-4 text-center text-sm text-gray-400">暂无同步冲突记录</p>
                ) : (
                  <div className="space-y-3 max-h-64 overflow-y-auto pr-1 custom-scrollbar">
                    {syncConflicts.map((conflict, index) => (
                      <div key={`${conflict.id}-${conflict.detectedAt}-${index}`} className="rounded-2xl border border-amber-100 bg-amber-50/50 p-4 space-y-3">
                        <div className="flex items-center gap-3">
                        <span className="text-xl">⚠️</span>
                        <div className="min-w-0 flex-1">
                          <p className="truncate text-sm font-bold text-gray-800">{conflict.local?.chineseName || conflict.remote?.chineseName || '未命名条目'}</p>
                          <p className="text-[11px] text-gray-500">
                            {conflict.kind === 'delete-edit' ? '删除与编辑发生冲突'
                              : conflict.kind === 'locked' ? '锁定条目与云端版本不同'
                                : `双方修改了相同字段：${conflict.fields.map(field => SYNC_FIELD_LABELS[field] || field).join('、')}`}
                            {' · '}{new Date(conflict.detectedAt).toLocaleString('zh-CN')}
                          </p>
                          <p className="mt-1 truncate text-[10px] text-gray-500">
                            本机：{conflict.local?.chineseName || (conflict.localDeleted ? '已删除' : '无记录')}
                            {' / '}云端：{conflict.remote?.chineseName || (conflict.remoteDeleted ? '已删除' : '无记录')}
                          </p>
                        </div>
                        </div>
                        <div className="flex justify-end gap-2">
                          {conflict.kind === 'delete-edit' ? <>
                            <button onClick={() => void onResolveConflict(conflict, 'keep')} className="rounded-xl bg-indigo-600 px-3 py-2 text-xs font-bold text-white hover:bg-indigo-700">保留条目</button>
                            <button onClick={() => void onResolveConflict(conflict, 'delete')} className="rounded-xl bg-red-500 px-3 py-2 text-xs font-bold text-white hover:bg-red-600">确认删除</button>
                          </> : <>
                            <button onClick={() => void onResolveConflict(conflict, 'local')} disabled={!conflict.local} className="rounded-xl bg-indigo-600 px-3 py-2 text-xs font-bold text-white hover:bg-indigo-700 disabled:bg-gray-300">采用本机</button>
                            <button onClick={() => void onResolveConflict(conflict, 'remote')} disabled={!conflict.remote} className="rounded-xl bg-emerald-600 px-3 py-2 text-xs font-bold text-white hover:bg-emerald-700 disabled:bg-gray-300">采用云端</button>
                          </>}
                        </div>
                      </div>
                    ))}
                  </div>
                )}
              </div>              {/* Auto Sync Settings */}
              <div className="bg-white rounded-3xl p-6 shadow-sm border border-gray-100 space-y-4">
                <div className="flex items-center gap-2.5 border-b border-gray-50 pb-4">
                  <span className="text-2xl">⏱️</span>
                  <div>
                    <h4 className="font-bold text-gray-800">自动同步防抖频率</h4>
                    <p className="text-[11px] text-gray-400">修改操作后，在后台自动上传至坚果云的防抖延迟时长</p>
                  </div>
                </div>
                <div className="space-y-4">
                  <div className="flex items-center justify-between text-sm select-none">
                    <span className="text-gray-500 font-medium">自动同步防抖间隔</span>
                    <span className="text-base font-black text-indigo-600">{localInterval} 秒</span>
                  </div>
                  <input
                    type="range" min="5" max="300" step="5"
                    value={localInterval}
                    onChange={(e) => onLocalIntervalChange(parseInt(e.target.value, 10))}
                    className="w-full h-2 bg-gray-100 rounded-lg appearance-none cursor-pointer accent-indigo-600"
                  />
                  <button
                    onClick={onSaveInterval}
                    className="py-2 px-5 rounded-xl bg-white border border-gray-200 text-xs font-semibold text-gray-600 hover:bg-gray-50 transition-colors shadow-sm"
                  >
                    💾 应用同步频率
                  </button>
                  <div className="border-t border-gray-100 pt-4 space-y-3">
                    <div className="flex items-center justify-between text-sm">
                      <span className="text-gray-500 font-medium">主动检查云端</span>
                      <select
                        value={localPullInterval}
                        onChange={event => onLocalPullIntervalChange(Number.parseInt(event.target.value, 10))}
                        className="rounded-xl border border-gray-200 bg-white px-3 py-2 text-sm text-gray-700"
                      >
                        <option value={0}>关闭周期检查</option>
                        <option value={5}>每 5 分钟</option>
                        <option value={15}>每 15 分钟</option>
                        <option value={30}>每 30 分钟</option>
                        <option value={60}>每 60 分钟</option>
                      </select>
                    </div>
                    <p className="text-[11px] leading-5 text-gray-400">
                      启动、重新聚焦和网络恢复仍会检查云端；暂停自动同步时所有自动检查都会停止。
                    </p>
                    <button
                      onClick={onSavePullInterval}
                      className="py-2 px-5 rounded-xl bg-white border border-gray-200 text-xs font-semibold text-gray-600 hover:bg-gray-50 transition-colors shadow-sm"
                    >
                      💾 应用主动拉取周期
                    </button>
                  </div>
                  {syncRuntime && (
                    <div data-testid="sync-runtime-status" className="rounded-2xl bg-gray-50 px-4 py-3 text-xs leading-5 text-gray-500">
                      <div>{syncRuntime.scheduler.paused ? '自动同步已暂停'
                        : syncRuntime.conflictCount > 0 ? `${syncRuntime.conflictCount} 项冲突等待处理`
                          : syncRuntime.publishPending ? '正在恢复未完成的云端发布'
                            : syncRuntime.stagedCount > 0 ? `${syncRuntime.stagedCount} 项本地版本等待上传`
                              : syncRuntime.outbox.pending ? '有本地修改等待同步'
                                : '本机与云端已核对'}</div>
                      {syncRuntime.scheduler.lastSuccessAt && <div>最近成功：{new Date(syncRuntime.scheduler.lastSuccessAt).toLocaleString('zh-CN')}</div>}
                      {syncRuntime.scheduler.nextAttemptAt && <div>下次重试：{new Date(syncRuntime.scheduler.nextAttemptAt).toLocaleString('zh-CN')}</div>}
                      {syncRuntime.scheduler.lastErrorCode && <div>当前状态：{syncRuntime.scheduler.lastErrorCode}</div>}
                    </div>
                  )}
                </div>
              </div>

  </div>;
}
