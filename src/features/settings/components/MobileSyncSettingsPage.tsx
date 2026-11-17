import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import {
  getActiveSyncConnection,
  getSyncRuntimeState,
  getSyncSnapshot,
  getSyncTargets,
  getLocalExportSnapshot,
  previewLocalImport,
  commitLocalImport,
  resolveSyncConflict,
  getTmdbCredentialStatus,
  saveTmdbCredential,
  clearTmdbCredential,
  getSettingAsync,
  setSettingAsync,
  vacuumDbAsync,
  cleanPosterCache,
  type ActiveSyncConnection,
  type SyncRuntimeState,
  type SyncTargetRegistry,
} from '../../../shared/lib/database.ts';
import { formatWebDavTargetUrl } from '../../../shared/lib/webdavDisplay.ts';
import {
  clearCreds,
  normalizeSyncTargetUrl,
  probeSyncTarget,
  saveCreds,
  syncFailureMessage,
  type SyncConflict,
  type SyncResult,
  type SyncTargetProbe,
} from '../../../shared/lib/webdav.ts';
import type { NoticeTone } from '../../../shared/lib/feedback.ts';
import {
  buildLocalBackupV4,
  localBackupFileName,
  serializeLocalBackupV4,
} from '../../backup/localBackupV4.ts';
import { exportJsonDocument } from '../../../platform/documentExport.ts';
import { discardImportedDocument, selectJsonDocument } from '../../../platform/documentImport.ts';
import {
  formatImportBytes,
  localImportErrorCode,
  localImportErrorMessage,
  type LocalImportPreview,
} from '../../backup/localImport.ts';

const DEFAULT_URL = 'https://dav.jianguoyun.com/dav/%E5%BD%B1%E8%A7%86%E8%BF%BD%E8%B8%AA/';

interface Props {
  runtime: SyncRuntimeState | null;
  isSyncing: boolean;
  onSyncNow: () => Promise<SyncResult>;
  onTogglePause: () => Promise<void>;
  onConfigurationChanged: () => Promise<void>;
  onSyncWorkQueued: () => Promise<void>;
  onReloadRecords: () => Promise<unknown>;
  notify: (tone: NoticeTone, message: string) => void;
}

function credentialLabel(connection: ActiveSyncConnection | null): string {
  if (!connection) return '未配置';
  if (connection.credentialAvailable) return '已配置';
  if (connection.credentialState === 'reentry-required') return '需要重新输入';
  if (connection.credentialState === 'unavailable') return '存储不可用';
  return '凭据缺失';
}

function probeLabel(probe: SyncTargetProbe): string {
  if (probe.kind === 'empty') return '目标中暂无同步文件。激活后会安全创建并上传本机数据。';
  if (probe.kind === 'legacy') return `发现旧版目标，共 ${probe.recordCount} 条记录。激活后将先安全导入并合并。`;
  return `发现 records-v3.json：${probe.recordCount} 条记录，修订 ${probe.revision ?? '未知'}。激活后将先拉取、合并，再按需上传。`;
}

function runtimeLabel(runtime: SyncRuntimeState | null, connection: ActiveSyncConnection | null, isSyncing: boolean): string {
  if (isSyncing) return '正在同步';
  if (!connection) return '未配置';
  if (!connection.credentialAvailable) return credentialLabel(connection);
  if (runtime?.scheduler.paused) return '已暂停';
  if (runtime?.scheduler.lastErrorCode) return '错误';
  if (runtime?.outbox.pending || runtime?.stagedCount || runtime?.publishPending) return '等待上传';
  if (runtime?.scheduler.lastSuccessAt) return '同步正常';
  return '已连接';
}

function conflictTitle(conflict: SyncConflict): string {
  return conflict.local?.chineseName || conflict.remote?.chineseName
    || conflict.local?.originalName || conflict.remote?.originalName || '未命名条目';
}

export default function MobileSyncSettingsPage({
  runtime, isSyncing, onSyncNow, onTogglePause, onConfigurationChanged,
  onSyncWorkQueued, onReloadRecords, notify,
}: Props) {
  const [connection, setConnection] = useState<ActiveSyncConnection | null>(null);
  const [registry, setRegistry] = useState<SyncTargetRegistry | null>(null);
  const [loadedRuntime, setLoadedRuntime] = useState<SyncRuntimeState | null>(null);
  const [conflicts, setConflicts] = useState<SyncConflict[]>([]);
  const [url, setUrl] = useState(DEFAULT_URL);
  const [username, setUsername] = useState('');
  const [password, setPassword] = useState('');
  const [probe, setProbe] = useState<SyncTargetProbe | null>(null);
  const [status, setStatus] = useState('');
  const [busy, setBusy] = useState<'probe' | 'activate' | 'sync' | 'clear' | 'pause' | 'resolve' | null>(null);
  const [showConfigForm, setShowConfigForm] = useState(false);

  // Maintenance state
  const [vacuumBusy, setVacuumBusy] = useState(false);
  const [vacuumStatus, setVacuumStatus] = useState('');
  const [cleanCacheBusy, setCleanCacheBusy] = useState(false);
  const [cleanCacheStatus, setCleanCacheStatus] = useState('');

  // Backup & Import state
  const [exportBusy, setExportBusy] = useState(false);
  const [exportStatus, setExportStatus] = useState('');
  const [importBusy, setImportBusy] = useState<'select' | 'preview' | 'commit' | null>(null);
  const [importStatus, setImportStatus] = useState('');
  const [importError, setImportError] = useState('');
  const [importPreview, setImportPreview] = useState<LocalImportPreview | null>(null);

  // TMDB & Proxy state
  const [tmdbConfigured, setTmdbConfigured] = useState(false);
  const [tmdbInput, setTmdbInput] = useState('');
  const [tmdbSaving, setTmdbSaving] = useState(false);
  const [proxyInput, setProxyInput] = useState('');
  const [proxySaving, setProxySaving] = useState(false);
  const pendingImportToken = useRef<string | null>(null);
  const importCommitInFlight = useRef(false);

  useEffect(() => () => {
    const token = pendingImportToken.current;
    if (token && !importCommitInFlight.current) void discardImportedDocument(token).catch(() => undefined);
  }, []);

  const refresh = useCallback(async () => {
    const [nextConnection, nextRegistry, nextRuntime, snapshot, tmdbStatus, savedProxy] = await Promise.all([
      getActiveSyncConnection(),
      getSyncTargets(),
      getSyncRuntimeState(),
      getSyncSnapshot(),
      getTmdbCredentialStatus().catch(() => ({ available: false, state: 'missing' as const })),
      getSettingAsync('network_proxy').catch(() => null),
    ]);
    setConnection(nextConnection);
    setRegistry(nextRegistry);
    setLoadedRuntime(nextRuntime);
    setConflicts(snapshot.conflicts);
    setTmdbConfigured(tmdbStatus.available);
    if (savedProxy !== null) {
      setProxyInput(savedProxy);
    }
    if (nextConnection) {
      setUrl(nextConnection.url);
      setUsername(nextConnection.username);
    }
  }, []);

  const saveTmdbKey = async () => {
    if (!tmdbInput.trim()) return;
    setTmdbSaving(true);
    try {
      const res = await saveTmdbCredential(tmdbInput.trim());
      setTmdbConfigured(res.available);
      setTmdbInput('');
      notify('success', 'TMDB API 密钥已保存并受 Android Keystore 保护。');
    } catch {
      notify('error', '保存 TMDB 密钥失败。');
    } finally {
      setTmdbSaving(false);
    }
  };

  const clearTmdbKey = async () => {
    if (!window.confirm('确定清除已保存的 TMDB 密钥吗？清除后将无法在线检索影片信息。')) return;
    try {
      await clearTmdbCredential();
      setTmdbConfigured(false);
      notify('info', '已清除 TMDB 密钥。');
    } catch {
      notify('error', '清除 TMDB 密钥失败。');
    }
  };

  const saveProxy = async () => {
    setProxySaving(true);
    try {
      await setSettingAsync('network_proxy', proxyInput.trim());
      notify('success', proxyInput.trim() ? '代理配置已保存。' : '已清除代理设置。');
    } catch {
      notify('error', '保存代理设置失败。');
    } finally {
      setProxySaving(false);
    }
  };

  const handleVacuum = async () => {
    if (vacuumBusy) return;
    setVacuumBusy(true);
    setVacuumStatus('正在压缩数据库…');
    try {
      await vacuumDbAsync();
      setVacuumStatus('✓ 数据库已完成压缩与碎片整理');
      notify('success', '数据库压缩完成');
    } catch (error) {
      console.error('[Vacuum]', error);
      setVacuumStatus('数据库压缩失败');
      notify('error', '数据库压缩失败');
    } finally {
      setVacuumBusy(false);
      setTimeout(() => setVacuumStatus(''), 4000);
    }
  };

  const handleCleanCache = async () => {
    if (cleanCacheBusy) return;
    if (!window.confirm('清理无引用的多余海报缓存？正常影视海报不受影响。')) return;
    setCleanCacheBusy(true);
    setCleanCacheStatus('正在清理海报缓存…');
    try {
      const res = await cleanPosterCache('unreferenced');
      const mb = (res.totalBytes / 1024 / 1024).toFixed(1);
      setCleanCacheStatus(`✓ 缓存清理完成，保留 ${res.referencedCount} 个有效封面 (${mb} MB)`);
      notify('success', `缓存清理完成，保留 ${res.referencedCount} 个有效封面`);
    } catch (error) {
      console.error('[CleanCache]', error);
      setCleanCacheStatus('清理海报缓存失败');
      notify('error', '清理海报缓存失败');
    } finally {
      setCleanCacheBusy(false);
      setTimeout(() => setCleanCacheStatus(''), 4000);
    }
  };

  useEffect(() => {
    let active = true;
    const timer = window.setTimeout(() => void refresh().catch(() => {
      if (active) setStatus('同步状态读取失败；本地片库仍可使用。');
    }), 0);
    return () => { active = false; window.clearTimeout(timer); };
  }, [refresh]);

  useEffect(() => {
    if (runtime?.conflictCount === undefined) return;
    const timer = window.setTimeout(() => void refresh(), 0);
    return () => window.clearTimeout(timer);
  }, [refresh, runtime?.conflictCount]);

  const liveRuntime = runtime ?? loadedRuntime;
  const display = useMemo(() => formatWebDavTargetUrl(connection?.url ?? url), [connection?.url, url]);
  const stateLabel = runtimeLabel(liveRuntime, connection, isSyncing || busy === 'sync');
  const editInput = (setter: (value: string) => void) => (value: string) => { setter(value); setProbe(null); setStatus(''); };

  const runProbe = async () => {
    if (!url.trim() || !username.trim() || !password) return;
    setBusy('probe'); setStatus('正在只读检查目标…'); setProbe(null);
    try {
      const normalizedUrl = normalizeSyncTargetUrl(url);
      const result = await probeSyncTarget({ url: normalizedUrl, username: username.trim(), password });
      setUrl(normalizedUrl); setProbe(result); setStatus(probeLabel(result));
    } catch {
      setStatus('目标检查失败。请检查地址、网络和凭据；当前连接未改变。');
      notify('error', 'WebDAV 目标只读检查失败。');
    } finally { setBusy(null); }
  };

  const activate = async () => {
    if (!probe || !password) return;
    setBusy('activate'); setStatus('正在安全保存凭据并激活目标…');
    try {
      await saveCreds({ url, username: username.trim(), password });
    } catch {
      setPassword('');
      setStatus('安全保存或激活失败，旧目标保持不变。');
      notify('error', 'WebDAV 目标激活失败。');
      setBusy(null);
      return;
    }

    setPassword(''); setProbe(null);
    setStatus('目标已激活，正在执行首次拉取与合并…');
    try {
      await onConfigurationChanged();
      const result = await onSyncNow();
      await refresh();
      if (result.ok) {
        setShowConfigForm(false);
        setStatus(result.conflictCount ? `首次同步完成，有 ${result.conflictCount} 项冲突需要处理。` : '目标已激活并完成首次同步。');
        notify(result.conflictCount ? 'warning' : 'success', result.conflictCount ? '首次同步完成，但有冲突需要处理。' : 'WebDAV 已安全连接并完成首次同步。');
      } else {
        const detail = syncFailureMessage(result.error) ?? '请稍后重试；本地片库不受影响。';
        setStatus(`目标已保存，但首次同步失败；${detail}`);
        notify('warning', '目标已保存，但首次同步失败。');
      }
    } catch {
      await refresh().catch(() => undefined);
      setStatus('目标已保存，但首次同步失败；失败状态可能无法刷新，本地片库仍可使用。');
      notify('warning', '目标已保存，但首次同步失败。');
    } finally { setBusy(null); }
  };

  const syncNow = async () => {
    setBusy('sync'); setStatus('正在同步…');
    try {
      const result = await onSyncNow();
      await refresh();
      if (result.ok) {
        setStatus(result.conflictCount ? `同步完成，有 ${result.conflictCount} 项冲突需要处理。` : '同步成功。');
      } else {
        setStatus(syncFailureMessage(result.error) ?? '同步失败；本地修改已保留并等待重试。');
      }
    } catch {
      setStatus('同步失败；本地修改已保留并等待重试。');
    } finally { setBusy(null); }
  };

  const clear = async () => {
    if (!window.confirm('清除当前 WebDAV 凭据并停止自动同步？本地片库和各目标的待处理状态不会删除。')) return;
    setBusy('clear');
    try {
      await clearCreds();
      setConnection(null); setPassword(''); setProbe(null);
      await refresh(); await onConfigurationChanged();
      setStatus('凭据已清除；本地数据未删除。');
      notify('success', 'WebDAV 凭据已清除。');
    } catch {
      setStatus('凭据清除失败；未假定安全存储已经删除。');
      notify('error', 'WebDAV 凭据清除失败。');
    } finally { setBusy(null); }
  };

  const togglePause = async () => {
    setBusy('pause');
    try { await onTogglePause(); await refresh(); setStatus(liveRuntime?.scheduler.paused ? '自动同步已恢复。' : '自动同步已暂停。'); }
    catch { setStatus('无法更新自动同步状态。'); }
    finally { setBusy(null); }
  };

  const resolve = async (conflict: SyncConflict, resolution: 'local' | 'remote' | 'keep' | 'delete') => {
    if (!window.confirm(`确认对“${conflictTitle(conflict)}”应用此选择？结果会进入待同步队列。`)) return;
    setBusy('resolve');
    try {
      const snapshot = await getSyncSnapshot();
      await resolveSyncConflict(conflict.id, resolution, snapshot.targetId, snapshot.targetEpoch);
      await onReloadRecords(); await onSyncWorkQueued(); await refresh();
      setStatus('冲突已解决，选择结果等待同步。');
    } catch { setStatus('冲突解决失败，原冲突仍保留。'); }
    finally { setBusy(null); }
  };

  const exportLocalData = async () => {
    if (exportBusy) return;
    setExportBusy(true);
    setExportStatus('正在准备导出…');
    try {
      const now = new Date();
      const snapshot = await getLocalExportSnapshot();
      const json = serializeLocalBackupV4(buildLocalBackupV4(snapshot, now));
      const fileName = localBackupFileName(now);
      setExportStatus('请选择保存位置…');
      const result = await exportJsonDocument(fileName, json);
      if (result.status === 'cancelled') {
        setExportStatus('已取消导出。');
        return;
      }
      setExportStatus(`已导出 ${result.fileName}`);
      notify('success', '数据已导出。');
    } catch (error) {
      console.error('[MobileSettings.Export]', error instanceof Error ? error.message : 'unknown_error');
      setExportStatus('数据导出失败。');
      notify('error', '数据导出失败。');
    } finally {
      setExportBusy(false);
    }
  };

  const clearPendingImport = async () => {
    const token = pendingImportToken.current;
    pendingImportToken.current = null;
    setImportPreview(null);
    if (token) await discardImportedDocument(token).catch(() => undefined);
  };

  const chooseImport = async () => {
    if (importBusy) return;
    await clearPendingImport();
    setImportError(''); setImportStatus(''); setImportBusy('select');
    try {
      const selection = await selectJsonDocument();
      if (selection.status === 'cancelled') {
        setImportStatus('已取消导入。');
        return;
      }
      pendingImportToken.current = selection.token;
      setImportBusy('preview');
      setImportStatus('正在安全验证备份…');
      const preview = await previewLocalImport(selection.token, selection.fileName);
      setImportPreview(preview);
      setImportStatus('');
    } catch (error) {
      const token = pendingImportToken.current;
      pendingImportToken.current = null;
      if (token) await discardImportedDocument(token).catch(() => undefined);
      setImportPreview(null);
      setImportStatus('');
      setImportError(localImportErrorMessage(error));
      notify('error', '无法导入此备份。');
    } finally {
      setImportBusy(null);
    }
  };

  const cancelImport = async () => {
    if (importBusy) return;
    setImportBusy('preview');
    await clearPendingImport();
    setImportError(''); setImportStatus('已取消导入。'); setImportBusy(null);
  };

  const confirmImport = async () => {
    const preview = importPreview;
    if (!preview || importBusy) return;
    setImportBusy('commit'); setImportError(''); setImportStatus('正在导入…');
    importCommitInFlight.current = true;
    try {
      const result = await commitLocalImport(preview);
      pendingImportToken.current = null;
      setImportPreview(null);
      await onReloadRecords();
      await onSyncWorkQueued();
      setImportStatus(`数据已导入，共 ${result.recordCount} 条记录。已创建导入前恢复点。`);
      notify('success', '数据已导入。');
    } catch (error) {
      const code = localImportErrorCode(error);
      const message = localImportErrorMessage(error);
      setImportError(message);
      setImportStatus('');
      if (code === 'import_preview_stale' || code === 'import_stage_changed') {
        notify('warning', message);
        const token = pendingImportToken.current;
        if (token) {
          try {
            const refreshed = await previewLocalImport(token, preview.fileName);
            setImportPreview(refreshed);
          } catch (previewError) {
            await clearPendingImport();
            setImportError(localImportErrorMessage(previewError));
          }
        }
      } else {
        await clearPendingImport();
        notify('error', '无法导入此备份。');
      }
    } finally {
      importCommitInFlight.current = false;
      setImportBusy(null);
    }
  };

  return (
    <section className="mobile-sync-settings" aria-labelledby="mobile-sync-title">
      <header className="mobile-page-heading">
        <div>
          <h1 id="mobile-sync-title">系统设置</h1>
          <p>同步、备份与存储维护</p>
        </div>
        <span className={`mobile-sync-state mobile-sync-state-${stateLabel === '错误' ? 'error' : 'normal'}`}>
          {stateLabel}
        </span>
      </header>

      {/* Group 1: WebDAV Cloud Sync */}
      <div className="mobile-settings-section" data-testid="mobile-sync-runtime">
        <div className="mobile-settings-header">
          <h2>☁️ WebDAV 云端同步</h2>
          <span className={`mobile-settings-tag ${connection ? 'green' : 'gray'}`}>
            {connection ? '已配置' : '未连接'}
          </span>
        </div>
        <p className="mobile-settings-desc">
          与桌面端双向轻量 V3 增量同步。密钥由 Android Keystore 硬件级隔离存储。
        </p>

        {connection ? (
          <>
            <div className="mobile-sync-facts">
              <div>
                <dt>云端账号</dt>
                <dd title={connection.username}>{connection.username}</dd>
              </div>
              <div>
                <dt>待上传变动</dt>
                <dd>{liveRuntime?.outbox.pending ? '有待上传' : '无'}</dd>
              </div>
              <div>
                <dt>待解决冲突</dt>
                <dd>{liveRuntime?.conflictCount ?? conflicts.length}</dd>
              </div>
              <div>
                <dt>最近同步</dt>
                <dd>{liveRuntime?.scheduler.lastRemoteCheckAt ? new Date(liveRuntime.scheduler.lastRemoteCheckAt).toLocaleTimeString('zh-CN', { hour: '2-digit', minute: '2-digit' }) : '未检查'}</dd>
              </div>
            </div>

            <div className="mobile-settings-row">
              <div className="mobile-settings-row-info">
                <span className="mobile-settings-row-title">同步路径</span>
                <span className="mobile-settings-row-sub" title={display.safeUrl}>{display.summary}</span>
              </div>
            </div>

            {liveRuntime?.scheduler.lastErrorCode && (
              <p className="mobile-sync-error" role="alert">
                同步异常：{liveRuntime.scheduler.lastErrorCode}。本地修改已完整保留。
              </p>
            )}

            {status && <p className="mobile-sync-message" role="status">{status}</p>}

            <div className="mobile-sync-actions">
              <button
                type="button"
                className="mobile-primary-button"
                style={{ flex: 1 }}
                disabled={busy !== null || isSyncing}
                onClick={() => void syncNow()}
              >
                {busy === 'sync' || isSyncing ? '同步中…' : '立即同步'}
              </button>
              <button
                type="button"
                className="mobile-quiet-button"
                disabled={busy !== null}
                onClick={() => void togglePause()}
              >
                {liveRuntime?.scheduler.paused ? '恢复自动同步' : '暂停自动同步'}
              </button>
              <button
                type="button"
                className="mobile-quiet-button"
                onClick={() => setShowConfigForm(prev => !prev)}
              >
                {showConfigForm ? '收起配置' : '修改连接'}
              </button>
              <button
                type="button"
                className="mobile-danger-button"
                disabled={busy !== null}
                onClick={() => void clear()}
              >
                断开
              </button>
            </div>
          </>
        ) : (
          <>
            <p className="mobile-sync-note" style={{ margin: '4px 0 8px' }}>
              支持坚果云及标准 WebDAV。配置完成后可在多设备间无缝流转观影记录。
            </p>
            <button
              type="button"
              className="mobile-primary-button"
              style={{ width: '100%' }}
              onClick={() => setShowConfigForm(true)}
            >
              配置 WebDAV 连接
            </button>
          </>
        )}

        {/* Expandable WebDAV configuration form */}
        {(showConfigForm || !connection) && (
          <form
            className="mobile-sync-form"
            style={{ marginTop: '8px', paddingTop: '12px', borderTop: '1px dashed #e2e8f0' }}
            onSubmit={event => { event.preventDefault(); void runProbe(); }}
          >
            <div style={{ display: 'flex', justifyContent: 'space-between', alignItems: 'center' }}>
              <span style={{ fontSize: '13px', fontWeight: 800, color: '#334155' }}>
                {connection ? '修改 WebDAV 目标' : '填写 WebDAV 服务器'}
              </span>
              <button
                type="button"
                className="mobile-settings-toggle-btn"
                onClick={() => setUrl(DEFAULT_URL)}
              >
                填入坚果云预设
              </button>
            </div>

            <div className="mobile-settings-input-group" style={{ marginTop: '8px' }}>
              <label>WebDAV URL</label>
              <input
                type="url"
                autoCapitalize="none"
                autoCorrect="off"
                value={url}
                onChange={event => editInput(setUrl)(event.target.value)}
                placeholder="https://dav.jianguoyun.com/dav/..."
                required
              />
            </div>

            <div className="mobile-settings-input-group" style={{ marginTop: '8px' }}>
              <label>账号 / 用户名</label>
              <input
                type="text"
                autoCapitalize="none"
                autoCorrect="off"
                value={username}
                onChange={event => editInput(setUsername)(event.target.value)}
                placeholder="邮箱或用户名"
                required
              />
            </div>

            <div className="mobile-settings-input-group" style={{ marginTop: '8px' }}>
              <label>密码 / 应用授权密码</label>
              <input
                type="password"
                value={password}
                onChange={event => editInput(setPassword)(event.target.value)}
                autoComplete="new-password"
                placeholder="输入密码"
                required
              />
            </div>

            <button
              type="submit"
              className="mobile-primary-button"
              style={{ width: '100%', marginTop: '12px' }}
              disabled={busy !== null || !url.trim() || !username.trim() || !password}
            >
              {busy === 'probe' ? '连接检测中…' : '测试连接并验证'}
            </button>

            {probe && (
              <div className="mobile-sync-probe" role="region" aria-label="目标检查结果">
                <p>{probeLabel(probe)}</p>
                <button
                  type="button"
                  className="mobile-primary-button"
                  disabled={busy !== null}
                  onClick={() => void activate()}
                >
                  {busy === 'activate' ? '激活中…' : '确认激活并开始同步'}
                </button>
              </div>
            )}
          </form>
        )}
      </div>

      {/* Sync Conflicts Section (only shown when conflicts exist or for testing) */}
      <div className="mobile-settings-section" data-testid="mobile-conflict-center">
        <div className="mobile-settings-header">
          <h2>⚖️ 同步冲突处理</h2>
          <span className={`mobile-settings-tag ${conflicts.length > 0 ? 'gray' : 'green'}`}>
            {conflicts.length > 0 ? `${conflicts.length} 条冲突` : '无冲突'}
          </span>
        </div>
        {conflicts.length === 0 ? (
          <p className="mobile-settings-desc">当前片库数据一致，无双向并发修改冲突。</p>
        ) : (
          conflicts.map(conflict => (
            <article key={`${conflict.id}-${conflict.detectedAt}`} className="mobile-sync-conflict">
              <h3>{conflictTitle(conflict)}</h3>
              <p>
                {conflict.kind === 'delete-edit'
                  ? '远端已删除但本地修改'
                  : conflict.kind === 'locked'
                    ? '锁定记录与远端不同'
                    : `相同字段同时被修改：${conflict.fields.join('、') || '基本信息'}`}
              </p>
              <div className="mobile-sync-actions">
                {conflict.kind === 'delete-edit' ? (
                  <>
                    <button type="button" disabled={busy !== null} onClick={() => void resolve(conflict, 'keep')}>
                      保留本地条目
                    </button>
                    <button type="button" disabled={busy !== null} onClick={() => void resolve(conflict, 'delete')}>
                      确认删除
                    </button>
                  </>
                ) : (
                  <>
                    <button type="button" disabled={busy !== null || !conflict.local} onClick={() => void resolve(conflict, 'local')}>
                      采用本机
                    </button>
                    <button type="button" disabled={busy !== null || !conflict.remote} onClick={() => void resolve(conflict, 'remote')}>
                      采用云端
                    </button>
                  </>
                )}
              </div>
            </article>
          ))
        )}
      </div>

      {/* Group 2: TMDB & Proxy */}
      <div className="mobile-settings-section" data-testid="mobile-tmdb-settings">
        <div className="mobile-settings-header">
          <h2>🎬 TMDB 元数据与网络代理</h2>
          <span className={`mobile-settings-tag ${tmdbConfigured ? 'green' : 'gray'}`}>
            {tmdbConfigured ? '✓ Keystore 已保护' : '未配置'}
          </span>
        </div>
        <p className="mobile-settings-desc">
          用于影视封面抓取、演职员、分季与评分填充。API Key 受硬件 Keystore 加密存储。
        </p>

        {/* TMDB API Key */}
        <div className="mobile-settings-row">
          <div className="mobile-settings-row-info">
            <span className="mobile-settings-row-title">TMDB API Key (V3)</span>
            <span className="mobile-settings-row-sub">
              {tmdbConfigured ? '已受硬件加密保护' : '未配置，无法在线检索影视元数据'}
            </span>
          </div>
          {tmdbConfigured ? (
            <button
              type="button"
              className="mobile-quiet-button mobile-danger"
              style={{ minHeight: '38px', padding: '0 10px' }}
              onClick={() => void clearTmdbKey()}
            >
              清除重新配置
            </button>
          ) : null}
        </div>

        {!tmdbConfigured && (
          <form
            onSubmit={e => { e.preventDefault(); void saveTmdbKey(); }}
            style={{ display: 'flex', gap: '8px', alignItems: 'center' }}
          >
            <input
              type="password"
              placeholder="输入 TMDB API Key"
              value={tmdbInput}
              onChange={e => setTmdbInput(e.target.value)}
              style={{ flex: 1, minHeight: '42px', padding: '0 10px', borderRadius: '10px', border: '1px solid #dbe3ef', outline: 'none' }}
              required
            />
            <button
              type="submit"
              className="mobile-primary-button"
              style={{ minHeight: '42px', padding: '0 14px' }}
              disabled={!tmdbInput.trim() || tmdbSaving}
            >
              {tmdbSaving ? '…' : '保存'}
            </button>
          </form>
        )}

        {/* Proxy */}
        <div style={{ marginTop: '6px' }}>
          <form onSubmit={e => { e.preventDefault(); void saveProxy(); }}>
            <div style={{ display: 'flex', flexDirection: 'column', gap: '4px' }}>
              <span style={{ fontSize: '12px', fontWeight: 800, color: '#475569' }}>
                网络代理 (国内加速可选)
              </span>
              <div style={{ display: 'flex', gap: '8px' }}>
                <input
                  type="text"
                  placeholder="如 http://127.0.0.1:7890，无代理留空"
                  value={proxyInput}
                  onChange={e => setProxyInput(e.target.value)}
                  style={{ flex: 1, minHeight: '42px', padding: '0 10px', borderRadius: '10px', border: '1px solid #dbe3ef', outline: 'none', fontSize: '13px' }}
                />
                <button
                  type="submit"
                  className="mobile-quiet-button"
                  style={{ minHeight: '42px', padding: '0 12px' }}
                  disabled={proxySaving}
                >
                  {proxySaving ? '…' : '保存'}
                </button>
              </div>
            </div>
          </form>
        </div>
      </div>

      {/* Group 3: Backup & Restore */}
      <div className="mobile-settings-section" data-testid="mobile-backup-export">
        <div className="mobile-settings-header">
          <h2>💾 数据备份与恢复</h2>
        </div>
        <p className="mobile-settings-desc">
          导出全量片库、逐集历史和自定义收藏为标准 JSON 备份，不含任何敏感密钥。
        </p>

        <div className="mobile-settings-dual-btns">
          <button
            type="button"
            className="mobile-settings-btn primary"
            disabled={exportBusy}
            onClick={() => void exportLocalData()}
          >
            <span>📤</span>
            <span>{exportBusy ? '准备导出…' : '导出备份 JSON'}</span>
          </button>
          <button
            type="button"
            className="mobile-settings-btn secondary"
            disabled={importBusy !== null}
            onClick={() => void chooseImport()}
          >
            <span>📥</span>
            <span>{importBusy === 'select' || importBusy === 'preview' ? '读取中…' : '导入备份文件'}</span>
          </button>
        </div>

        {exportStatus && (
          <p className="mobile-sync-message" role="status" data-testid="mobile-export-status">
            {exportStatus}
          </p>
        )}
        {importStatus && (
          <p className="mobile-sync-message" role="status" data-testid="mobile-import-status">
            {importStatus}
          </p>
        )}
        {importError && (
          <p className="mobile-sync-error" role="alert" data-testid="mobile-import-error">
            {importError}
          </p>
        )}

        {importPreview && (
          <section className="mobile-import-preview" role="region" aria-label="导入预览" data-testid="mobile-import-preview">
            <h3>导入备份预览</h3>
            <dl className="mobile-sync-facts">
              <div><dt>文件</dt><dd>{importPreview.fileName}</dd></div>
              <div><dt>大小</dt><dd>{formatImportBytes(importPreview.sizeBytes)}</dd></div>
              <div><dt>格式版本</dt><dd>V{importPreview.formatVersion}</dd></div>
              <div><dt>导出时间</dt><dd>{new Date(importPreview.exportedAt).toLocaleDateString('zh-CN')}</dd></div>
              <div><dt>影视记录</dt><dd>{importPreview.counts.records}</dd></div>
              <div><dt>逐集历史</dt><dd>{importPreview.counts.episodeCompletions}</dd></div>
            </dl>
            <h4>预计影响</h4>
            <dl className="mobile-import-diff">
              <div><dt>新增</dt><dd>{importPreview.records.added}</dd></div>
              <div><dt>更新</dt><dd>{importPreview.records.updated}</dd></div>
              <div><dt>删除</dt><dd>{importPreview.records.removed}</dd></div>
              <div><dt>导入后总计</dt><dd>{importPreview.records.finalCount}</dd></div>
            </dl>
            <p className="mobile-sync-note">
              导入前会自动生成安全还原点。已锁定的本地记录将受到保护，不会被覆盖。
            </p>
            <div className="mobile-sync-actions">
              <button
                type="button"
                className="mobile-quiet-button"
                disabled={importBusy !== null}
                onClick={() => void cancelImport()}
              >
                取消导入
              </button>
              <button
                type="button"
                className="mobile-danger-button"
                disabled={importBusy !== null}
                onClick={() => void confirmImport()}
              >
                {importBusy === 'commit' ? '正在写入…' : '确认导入并生效'}
              </button>
            </div>
          </section>
        )}
      </div>

      {/* Group 4: Storage & Maintenance (P2 Feature) */}
      <div className="mobile-settings-section">
        <div className="mobile-settings-header">
          <h2>🛠️ 存储维护与优化</h2>
        </div>
        <p className="mobile-settings-desc">
          维护本地 SQLite 数据库与封面缓存，优化启动速度并释放手机存储空间。
        </p>

        {/* SQLite VACUUM */}
        <div className="mobile-settings-row">
          <div className="mobile-settings-row-info">
            <span className="mobile-settings-row-title">整理并压缩数据库 (VACUUM)</span>
            <span className="mobile-settings-row-sub">整理磁盘碎片，回收已删除记录的空间</span>
          </div>
          <button
            type="button"
            className="mobile-settings-btn secondary"
            style={{ minHeight: '38px', padding: '0 12px', fontSize: '12px' }}
            disabled={vacuumBusy}
            onClick={() => void handleVacuum()}
          >
            {vacuumBusy ? '压缩中…' : '立即压缩'}
          </button>
        </div>
        {vacuumStatus && <p className="mobile-sync-message">{vacuumStatus}</p>}

        {/* Poster Cache Cleaner */}
        <div className="mobile-settings-row">
          <div className="mobile-settings-row-info">
            <span className="mobile-settings-row-title">清理孤立海报缓存</span>
            <span className="mobile-settings-row-sub">清除已删除影片的无用封面文件</span>
          </div>
          <button
            type="button"
            className="mobile-settings-btn secondary"
            style={{ minHeight: '38px', padding: '0 12px', fontSize: '12px' }}
            disabled={cleanCacheBusy}
            onClick={() => void handleCleanCache()}
          >
            {cleanCacheBusy ? '清理中…' : '清理缓存'}
          </button>
        </div>
        {cleanCacheStatus && <p className="mobile-sync-message">{cleanCacheStatus}</p>}
      </div>

      {/* Group 5: About WatchTracker */}
      <div className="mobile-settings-section" style={{ background: '#f8fafc', border: '1px solid #f1f5f9' }}>
        <div className="mobile-settings-header">
          <h2 style={{ fontSize: '14px', color: '#64748b' }}>ℹ️ 关于 WatchTracker</h2>
          <span style={{ fontSize: '11px', color: '#94a3b8' }}>v1.10.2 (Android)</span>
        </div>
        <p style={{ margin: 0, fontSize: '11px', color: '#94a3b8', lineHeight: 1.6 }}>
          纯本地优先架构 · Android Keystore 硬件隔离保护 · 轻量 V3 双向同步协议
        </p>
      </div>

      {registry && registry.targets.length > 1 && (
        <p className="mobile-sync-target-count">
          已保留 {registry.targets.length} 个隔离同步目标的本地状态。
        </p>
      )}
    </section>
  );
}
