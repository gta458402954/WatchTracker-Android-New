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
import { useBatchMetadata } from '../hooks/useBatchMetadata.ts';
import { BATCH_METADATA_FIELD_LABELS, type BatchMetadataNoDataState } from '../../../shared/lib/batchMetadata.ts';
import type { UpdateWatchRecord, WatchRecord } from '../../../shared/types';

const DEFAULT_URL = 'https://dav.jianguoyun.com/dav/%E5%BD%B1%E8%A7%86%E8%BF%BD%E8%B8%AA/';
const DEFAULT_BATCH_NO_DATA: BatchMetadataNoDataState = { version: 1, records: {} };
const EMPTY_RECORDS: WatchRecord[] = [];

interface Props {
  runtime: SyncRuntimeState | null;
  isSyncing: boolean;
  onSyncNow: () => Promise<SyncResult>;
  onTogglePause: () => Promise<void>;
  onConfigurationChanged: () => Promise<void>;
  onSyncWorkQueued: () => Promise<void>;
  onReloadRecords: () => Promise<unknown>;
  notify: (tone: NoticeTone, message: string) => void;
  records?: WatchRecord[];
  onUpdateRecord?: (id: string, updates: UpdateWatchRecord) => Promise<void>;
}

function credentialLabel(connection: ActiveSyncConnection | null): string {
  if (!connection) return '未配置';
  if (connection.credentialAvailable) return '已配置';
  if (connection.credentialState === 'reentry-required') return '需要重新输入凭据';
  if (connection.credentialState === 'unavailable') return '系统凭据存储不可用';
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
  if (runtime?.scheduler.lastSuccessAt) return '同步成功';
  return '已配置';
}

function conflictTitle(conflict: SyncConflict): string {
  return conflict.local?.chineseName || conflict.remote?.chineseName
    || conflict.local?.originalName || conflict.remote?.originalName || '未命名条目';
}

export default function MobileSyncSettingsPage({
  runtime, isSyncing, onSyncNow, onTogglePause, onConfigurationChanged,
  onSyncWorkQueued, onReloadRecords, notify, records, onUpdateRecord,
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
  const [exportBusy, setExportBusy] = useState(false);
  const [exportStatus, setExportStatus] = useState('');
  const [importBusy, setImportBusy] = useState<'select' | 'preview' | 'commit' | null>(null);
  const [importStatus, setImportStatus] = useState('');
  const [importError, setImportError] = useState('');
  const [importPreview, setImportPreview] = useState<LocalImportPreview | null>(null);
  const [tmdbConfigured, setTmdbConfigured] = useState(false);
  const [tmdbInput, setTmdbInput] = useState('');
  const [tmdbSaving, setTmdbSaving] = useState(false);
  const [proxyInput, setProxyInput] = useState('');
  const [proxySaving, setProxySaving] = useState(false);
  const [vacuumBusy, setVacuumBusy] = useState(false);
  const [vacuumStatus, setVacuumStatus] = useState('');
  const [cleanCacheBusy, setCleanCacheBusy] = useState(false);
  const [cleanCacheStatus, setCleanCacheStatus] = useState('');
  const pendingImportToken = useRef<string | null>(null);
  const importCommitInFlight = useRef(false);

  const batch = useBatchMetadata({
    records: records ?? EMPTY_RECORDS,
    tmdbSaved: tmdbConfigured,
    initialBatchNoDataState: DEFAULT_BATCH_NO_DATA,
    onUpdateRecord: onUpdateRecord ?? (async () => undefined),
    onDatabaseRestored: async () => {
      await onReloadRecords();
      return records ?? EMPTY_RECORDS;
    },
    onNotify: notify,
    showFailure: (_scope, action, error, setStatus) => {
      const msg = error instanceof Error ? error.message : String(error);
      notify('error', `${action}失败: ${msg}`);
      if (setStatus) setStatus(`${action}失败`);
    },
    showSuccess: msg => notify('success', msg),
  });

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

  const togglePause = async () => {
    setBusy('pause');
    try {
      await onTogglePause();
      await refresh();
      notify('info', liveRuntime?.scheduler.paused ? '自动同步已恢复。' : '自动同步已暂停。');
    } catch {
      notify('error', '切换自动同步状态失败。');
    } finally { setBusy(null); }
  };

  const clear = async () => {
    if (!window.confirm('确定清除已保存的凭据吗？清除后会自动断开云端同步，本地片库完整保留。')) return;
    setBusy('clear'); setStatus('正在清除凭据…');
    try {
      await clearCreds();
      await onConfigurationChanged();
      await refresh();
      setPassword(''); setProbe(null);
      setStatus('凭据已清除；本地数据未删除。');
      notify('info', 'WebDAV 凭据已清除，本地数据完好保留。');
    } catch {
      setStatus('清除凭据失败；当前凭据可能仍有效。');
      notify('error', '清除 WebDAV 凭据失败。');
    } finally { setBusy(null); }
  };

  const resolve = async (conflict: SyncConflict, resolution: 'local' | 'remote' | 'keep' | 'delete') => {
    if (!window.confirm(`确认对“${conflictTitle(conflict)}”应用此选择？结果会进入待同步队列。`)) return;
    setBusy('resolve');
    try {
      const snapshot = await getSyncSnapshot();
      await resolveSyncConflict(conflict.id, resolution, snapshot.targetId, snapshot.targetEpoch);
      await onReloadRecords();
      await onSyncWorkQueued();
      await refresh();
      setStatus('冲突已解决，选择结果等待同步。');
    } catch {
      setStatus('冲突解决失败，原冲突仍保留。');
    } finally {
      setBusy(null);
    }
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
    setImportError('');
    setImportStatus('');
    setImportBusy('select');
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
    setImportError('');
    setImportStatus('已取消导入。');
    setImportBusy(null);
  };

  const confirmImport = async () => {
    const preview = importPreview;
    if (!preview || importBusy) return;
    setImportBusy('commit');
    setImportError('');
    setImportStatus('正在导入…');
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
      const message = localImportErrorMessage(code);
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
          <h1 id="mobile-sync-title">设置</h1>
          <p>WebDAV 密码由 Android Keystore 保护，不会回显到页面。</p>
        </div>
        <span className={`mobile-sync-state mobile-sync-state-${stateLabel === '错误' ? 'error' : 'normal'}`}>
          {stateLabel}
        </span>
      </header>

      {/* Backup & Restore */}
      <div className="mobile-sync-card" data-testid="mobile-backup-export">
        <h2>备份与恢复</h2>
        <p className="mobile-sync-note">导出片库、逐集历史和收藏数据为 JSON 文件。</p>
        <p className="mobile-sync-note">不会包含 WebDAV 密码、TMDB API Key 或同步运行状态。</p>
        <button
          type="button"
          className="mobile-primary-button"
          disabled={exportBusy}
          onClick={() => void exportLocalData()}
        >
          {exportBusy ? '正在准备导出…' : '导出数据'}
        </button>
        <button
          type="button"
          className="mobile-quiet-button mobile-import-button"
          disabled={importBusy !== null}
          onClick={() => void chooseImport()}
        >
          {importBusy === 'select' || importBusy === 'preview' ? '正在读取备份…' : '导入数据'}
        </button>
        {exportStatus && <p className="mobile-sync-message" role="status" data-testid="mobile-export-status">{exportStatus}</p>}
        {importStatus && <p className="mobile-sync-message" role="status" data-testid="mobile-import-status">{importStatus}</p>}
        {importError && <p className="mobile-sync-error" role="alert" data-testid="mobile-import-error">{importError}</p>}
        {importPreview && (
          <section className="mobile-import-preview" role="region" aria-label="导入预览" data-testid="mobile-import-preview">
            <h3>导入预览</h3>
            <dl className="mobile-sync-facts">
              <div><dt>文件</dt><dd>{importPreview.fileName}</dd></div>
              <div><dt>大小</dt><dd>{formatImportBytes(importPreview.sizeBytes)}</dd></div>
              <div><dt>格式</dt><dd>V{importPreview.formatVersion}</dd></div>
              <div><dt>导出时间</dt><dd>{new Date(importPreview.exportedAt).toLocaleString('zh-CN')}</dd></div>
              <div><dt>记录</dt><dd>{importPreview.counts.records}</dd></div>
              <div><dt>逐集历史</dt><dd>{importPreview.counts.episodeCompletions}</dd></div>
              <div><dt>收藏集</dt><dd>{importPreview.counts.collections}</dd></div>
              <div><dt>收藏成员</dt><dd>{importPreview.counts.collectionMembers}</dd></div>
            </dl>
            <h4>预计影响</h4>
            <dl className="mobile-import-diff">
              <div><dt>新增</dt><dd>{importPreview.records.added}</dd></div>
              <div><dt>更新</dt><dd>{importPreview.records.updated}</dd></div>
              <div><dt>删除</dt><dd>{importPreview.records.removed}</dd></div>
              <div><dt>不变</dt><dd>{importPreview.records.unchanged}</dd></div>
              <div><dt>锁定保护</dt><dd>{importPreview.records.lockedPreserved}</dd></div>
              <div><dt>导入后记录</dt><dd>{importPreview.records.finalCount}</dd></div>
            </dl>
            <p className="mobile-sync-note">
              未锁定的当前数据可能被替换或删除。锁定记录及其逐集历史会受到保护；操作前会创建恢复点。
            </p>
            <div className="mobile-sync-actions">
              <button type="button" className="mobile-quiet-button" disabled={importBusy !== null} onClick={() => void cancelImport()}>
                取消导入
              </button>
              <button type="button" className="mobile-danger-button" disabled={importBusy !== null} onClick={() => void confirmImport()}>
                {importBusy === 'commit' ? '正在导入…' : '确认导入并替换'}
              </button>
            </div>
          </section>
        )}
      </div>

      {/* TMDB & Network Proxy */}
      <div className="mobile-sync-card" data-testid="mobile-tmdb-settings">
        <h2>TMDB 元数据与网络代理</h2>
        <p className="mobile-sync-note">用于影视海报拉取、年份及详情自动填充。API 密钥由 Android Keystore 硬件级加密保护。</p>
        <dl className="mobile-sync-facts">
          <div><dt>TMDB 密钥</dt><dd>{tmdbConfigured ? '✓ 已受 Keystore 保护' : '未配置'}</dd></div>
          <div><dt>网络代理</dt><dd>{proxyInput.trim() || '直连 (无代理)'}</dd></div>
        </dl>
        {!tmdbConfigured ? (
          <form onSubmit={e => { e.preventDefault(); void saveTmdbKey(); }} style={{ marginTop: '12px' }}>
            <label>
              TMDB API KEY (V3)
              <input type="password" placeholder="输入 TMDB API KEY" value={tmdbInput} onChange={e => setTmdbInput(e.target.value)} required />
            </label>
            <button type="submit" className="mobile-primary-button" disabled={!tmdbInput.trim() || tmdbSaving} style={{ marginTop: '8px' }}>
              {tmdbSaving ? '保存中…' : '保存密钥'}
            </button>
          </form>
        ) : (
          <div className="mobile-sync-actions" style={{ marginTop: '10px' }}>
            <button type="button" className="mobile-quiet-button mobile-danger" onClick={() => void clearTmdbKey()}>
              清除密钥并重新输入
            </button>
          </div>
        )}
        <form onSubmit={e => { e.preventDefault(); void saveProxy(); }} style={{ marginTop: '16px' }}>
          <label>
            网络代理 (可选)
            <input type="text" placeholder="如 http://127.0.0.1:7890，无代理留空" value={proxyInput} onChange={e => setProxyInput(e.target.value)} />
          </label>
          <button type="submit" className="mobile-quiet-button" disabled={proxySaving} style={{ marginTop: '8px' }}>
            {proxySaving ? '保存中…' : '保存代理设置'}
          </button>
        </form>
      </div>

      {/* Batch Metadata Tool */}
      <div className="mobile-sync-card" data-testid="mobile-batch-metadata">
        <h2>批量元数据补全</h2>
        <p className="mobile-sync-note">
          基于已录入的 IMDb 编号，通过 TMDB 智能分析并补全缺失的年份、评分、海报、国家、片长与分季信息。绝不覆盖已有数据。
        </p>

        {(batch.batchPhase === 'idle' || batch.batchPhase === 'done') && (
          <div style={{ display: 'flex', flexDirection: 'column', gap: '8px', marginTop: '10px' }}>
            <button
              type="button"
              className="mobile-primary-button"
              disabled={!tmdbConfigured}
              onClick={() => void batch.handlePrepare()}
            >
              分析并预览缺失字段
            </button>
            {Object.keys(batch.batchNoDataState.records).length > 0 && (
              <div style={{ display: 'flex', alignItems: 'center', justifyContent: 'space-between', padding: '6px 10px', background: '#f8fafc', borderRadius: '10px', fontSize: '12px', color: '#64748b' }}>
                <span>已记住 {Object.keys(batch.batchNoDataState.records).length} 个无数据条目</span>
                <button
                  type="button"
                  className="mobile-quiet-button"
                  style={{ minHeight: '28px', padding: '0 8px', fontSize: '11px' }}
                  onClick={() => void batch.handleClearNoData()}
                >
                  清除记忆
                </button>
              </div>
            )}
          </div>
        )}

        {batch.batchSyncing && batch.batchTotal > 0 && (
          <div style={{ marginTop: '10px', display: 'flex', flexDirection: 'column', gap: '6px' }}>
            <div className="mobile-stats-progress-track" style={{ height: '8px' }}>
              <div
                className="mobile-stats-progress-seg completed"
                style={{ width: `${(batch.batchProgress / batch.batchTotal) * 100}%`, background: '#f59e0b' }}
              />
            </div>
            <div style={{ display: 'flex', justifyContent: 'space-between', alignItems: 'center', fontSize: '11px', color: '#64748b' }}>
              <span>进度：{batch.batchProgress} / {batch.batchTotal}</span>
              <button
                type="button"
                className="mobile-quiet-button mobile-danger"
                style={{ minHeight: '28px', padding: '0 8px', fontSize: '11px' }}
                onClick={batch.handleCancel}
              >
                停止
              </button>
            </div>
          </div>
        )}

        {batch.batchStatus && (
          <p className="mobile-sync-message" style={{ color: '#d97706', fontWeight: 700, marginTop: '8px' }}>
            {batch.batchStatus}
          </p>
        )}

        {batch.batchPhase === 'preview' && (
          <div className="mobile-batch-preview-box" style={{ marginTop: '10px', display: 'flex', flexDirection: 'column', gap: '8px' }}>
            <div style={{ display: 'flex', justifyContent: 'space-between', alignItems: 'center' }}>
              <span style={{ fontSize: '13px', fontWeight: 800, color: '#1e293b' }}>
                写入预览 ({batch.batchPlan.filter(r => r.status === 'ready').length} 可更新)
              </span>
              <small style={{ color: '#94a3b8' }}>只补缺失字段</small>
            </div>

            <div style={{ maxHeight: '240px', overflowY: 'auto', display: 'flex', flexDirection: 'column', gap: '6px', paddingRight: '4px' }}>
              {batch.batchPlan.map(row => (
                <div key={row.recordId} style={{ padding: '8px 10px', borderRadius: '10px', background: '#f8fafc', border: '1px solid #e2e8f0', fontSize: '12px' }}>
                  <div style={{ display: 'flex', justifyContent: 'space-between', alignItems: 'center' }}>
                    <strong style={{ color: '#0f172a', overflow: 'hidden', textOverflow: 'ellipsis', whiteSpace: 'nowrap' }}>{row.recordName}</strong>
                    <span style={{
                      fontSize: '10px', fontWeight: 800, padding: '2px 6px', borderRadius: '6px',
                      background: row.status === 'ready' ? '#d1fae5' : row.status === 'choice' ? '#fef3c7' : '#f1f5f9',
                      color: row.status === 'ready' ? '#065f46' : row.status === 'choice' ? '#92400e' : '#64748b'
                    }}>
                      {row.status === 'ready' ? '可更新' : row.status === 'choice' ? '待选择' : row.status === 'failed' ? '失败' : '跳过'}
                    </span>
                  </div>
                  <div style={{ fontSize: '11px', color: '#64748b', marginTop: '3px' }}>
                    {row.fields.length ? `补全：${row.fields.map(f => BATCH_METADATA_FIELD_LABELS[f]).join('、')}` : row.reason}
                  </div>
                  {row.status === 'choice' && row.candidates && (
                    <div style={{ marginTop: '6px', display: 'flex', flexDirection: 'column', gap: '4px' }}>
                      {row.candidates.map(candidate => (
                        <button
                          key={`${candidate.match.type}:${candidate.match.id}`}
                          type="button"
                          className="mobile-quiet-button"
                          style={{ fontSize: '10px', minHeight: '30px', textAlign: 'left', padding: '4px 8px' }}
                          disabled={batch.batchChoosingId === row.recordId}
                          onClick={() => void batch.handleSelectCandidate(row, candidate.match)}
                        >
                          {batch.batchChoosingId === row.recordId ? '读取中…' : candidate.label}
                        </button>
                      ))}
                    </div>
                  )}
                </div>
              ))}
            </div>

            <div style={{ display: 'flex', gap: '8px', marginTop: '6px' }}>
              <button
                type="button"
                className="mobile-primary-button"
                style={{ flex: 2, minHeight: '40px' }}
                disabled={!batch.batchPlan.some(r => r.status === 'ready')}
                onClick={() => void batch.handleApply()}
              >
                确认写入 ({batch.batchPlan.filter(r => r.status === 'ready').length})
              </button>
              <button
                type="button"
                className="mobile-quiet-button"
                style={{ flex: 1, minHeight: '40px' }}
                onClick={batch.resetBatch}
              >
                取消
              </button>
            </div>
          </div>
        )}

        {batch.batchPhase === 'done' && batch.batchResults.length > 0 && (
          <div style={{ marginTop: '10px', display: 'flex', flexDirection: 'column', gap: '8px' }}>
            <div style={{ maxHeight: '180px', overflowY: 'auto', display: 'flex', flexDirection: 'column', gap: '4px' }}>
              {batch.batchResults.map(res => (
                <div key={res.plan.recordId} style={{ display: 'flex', justifyContent: 'space-between', padding: '6px 8px', borderRadius: '8px', background: '#f8fafc', fontSize: '11px' }}>
                  <span style={{ overflow: 'hidden', textOverflow: 'ellipsis', whiteSpace: 'nowrap', maxWidth: '70%' }}>{res.plan.recordName}</span>
                  <span style={{ fontWeight: 800, color: res.status === 'updated' ? '#10b981' : res.status === 'failed' ? '#ef4444' : '#94a3b8' }}>
                    {res.status === 'updated' ? '已更新' : res.status === 'failed' ? '失败' : '跳过'}
                  </span>
                </div>
              ))}
            </div>
            <button
              type="button"
              className="mobile-quiet-button"
              style={{ minHeight: '36px', width: '100%' }}
              onClick={batch.resetBatch}
            >
              完成
            </button>
          </div>
        )}
      </div>

      {/* WebDAV Cloud Sync */}
      <div className="mobile-sync-card" data-testid="mobile-sync-runtime">
        <h2>WebDAV 同步</h2>
        <dl className="mobile-sync-facts">
          <div><dt>凭据</dt><dd>{credentialLabel(connection)}</dd></div>
          <div><dt>待上传</dt><dd>{liveRuntime?.outbox.pending ? '是' : '否'}</dd></div>
          <div><dt>冲突</dt><dd>{liveRuntime?.conflictCount ?? conflicts.length}</dd></div>
          <div><dt>最近检查</dt><dd>{liveRuntime?.scheduler.lastRemoteCheckAt ? new Date(liveRuntime.scheduler.lastRemoteCheckAt).toLocaleString('zh-CN') : '尚未检查'}</dd></div>
        </dl>
        {connection && <p className="mobile-sync-target" title={display.safeUrl}>{connection.username} · {display.summary}</p>}
        {liveRuntime?.scheduler.lastErrorCode && <p className="mobile-sync-error" role="alert">同步错误：{liveRuntime.scheduler.lastErrorCode}。本地修改已保留。</p>}
        {status && <p className="mobile-sync-message" role="status">{status}</p>}
        {connection && <div className="mobile-sync-actions">
          {connection.credentialAvailable && <>
            <button type="button" className="mobile-primary-button" disabled={busy !== null || isSyncing} onClick={() => void syncNow()}>{busy === 'sync' || isSyncing ? '同步中…' : '立即同步'}</button>
            <button type="button" className="mobile-quiet-button" disabled={busy !== null} onClick={() => void togglePause()}>{liveRuntime?.scheduler.paused ? '恢复自动同步' : '暂停自动同步'}</button>
          </>}
          <button type="button" className="mobile-danger-button" disabled={busy !== null} onClick={() => void clear()}>清除凭据</button>
        </div>}
      </div>

      {/* WebDAV Form */}
      <form className="mobile-sync-card mobile-sync-form" onSubmit={event => { event.preventDefault(); void runProbe(); }}>
        <h2>{connection ? '更新或切换目标' : '配置 WebDAV'}</h2>
        <label>WebDAV URL<input type="url" autoCapitalize="none" autoCorrect="off" value={url} onChange={event => editInput(setUrl)(event.target.value)} required /></label>
        <label>用户名<input type="text" autoCapitalize="none" autoCorrect="off" value={username} onChange={event => editInput(setUsername)(event.target.value)} required /></label>
        <label>密码<input type="password" value={password} onChange={event => editInput(setPassword)(event.target.value)} autoComplete="new-password" required /></label>
        <button type="submit" className="mobile-primary-button" disabled={busy !== null || !url.trim() || !username.trim() || !password}>{busy === 'probe' ? '检查中…' : '测试连接'}</button>
        {probe && <div className="mobile-sync-probe" role="region" aria-label="目标检查结果"><p>{probeLabel(probe)}</p><button type="button" className="mobile-primary-button" disabled={busy !== null} onClick={() => void activate()}>{busy === 'activate' ? '激活中…' : '确认激活并首次同步'}</button></div>}
        <p className="mobile-sync-note">激活前只进行只读检查。切换目标不会删除旧目标的待上传、基线、冲突或暂存状态。</p>
      </form>

      {/* Sync Conflicts */}
      <div className="mobile-sync-card" data-testid="mobile-conflict-center">
        <h2>同步冲突</h2>
        {conflicts.length === 0 ? <p className="mobile-sync-empty">暂无冲突</p> : conflicts.map(conflict => <article key={`${conflict.id}-${conflict.detectedAt}`} className="mobile-sync-conflict">
          <h3>{conflictTitle(conflict)}</h3>
          <p>{conflict.kind === 'delete-edit' ? '删除与编辑冲突' : conflict.kind === 'locked' ? '锁定记录与远端不同' : `相同字段同时修改：${conflict.fields.join('、') || '记录'}`}</p>
          <div className="mobile-sync-actions">
            {conflict.kind === 'delete-edit' ? <>
              <button type="button" disabled={busy !== null} onClick={() => void resolve(conflict, 'keep')}>保留条目</button>
              <button type="button" disabled={busy !== null} onClick={() => void resolve(conflict, 'delete')}>确认删除</button>
            </> : <>
              <button type="button" disabled={busy !== null || !conflict.local} onClick={() => void resolve(conflict, 'local')}>采用本机</button>
              <button type="button" disabled={busy !== null || !conflict.remote} onClick={() => void resolve(conflict, 'remote')}>采用云端</button>
            </>}
          </div>
        </article>)}
      </div>

      {/* Storage & Maintenance */}
      <div className="mobile-sync-card" data-testid="mobile-maintenance">
        <h2>存储维护与优化</h2>
        <p className="mobile-sync-note">维护本地 SQLite 数据库与封面缓存，优化启动速度并释放手机存储空间。</p>
        <div style={{ display: 'flex', flexDirection: 'column', gap: '10px', marginTop: '10px' }}>
          <div>
            <div style={{ display: 'flex', justifyContent: 'space-between', alignItems: 'center' }}>
              <strong>整理并压缩数据库 (VACUUM)</strong>
              <button type="button" className="mobile-quiet-button" disabled={vacuumBusy} onClick={() => void handleVacuum()}>{vacuumBusy ? '压缩中…' : '立即压缩'}</button>
            </div>
            <p className="mobile-sync-note" style={{ margin: '2px 0 0' }}>整理磁盘碎片，回收已删除记录的空间</p>
            {vacuumStatus && <p className="mobile-sync-message">{vacuumStatus}</p>}
          </div>
          <div>
            <div style={{ display: 'flex', justifyContent: 'space-between', alignItems: 'center' }}>
              <strong>清理孤立海报缓存</strong>
              <button type="button" className="mobile-quiet-button" disabled={cleanCacheBusy} onClick={() => void handleCleanCache()}>{cleanCacheBusy ? '清理中…' : '清理缓存'}</button>
            </div>
            <p className="mobile-sync-note" style={{ margin: '2px 0 0' }}>清除已删除影片的无用封面文件</p>
            {cleanCacheStatus && <p className="mobile-sync-message">{cleanCacheStatus}</p>}
          </div>
        </div>
      </div>

      {registry && registry.targets.length > 1 && (
        <p className="mobile-sync-target-count">已保留 {registry.targets.length} 个隔离同步目标的本地状态。</p>
      )}
    </section>
  );
}
