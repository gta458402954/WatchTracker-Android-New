import { useCallback, useEffect, useMemo, useState } from 'react';
import {
  getActiveSyncConnection,
  getSyncRuntimeState,
  getSyncSnapshot,
  getSyncTargets,
  resolveSyncConflict,
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

  const refresh = useCallback(async () => {
    const [nextConnection, nextRegistry, nextRuntime, snapshot] = await Promise.all([
      getActiveSyncConnection(), getSyncTargets(), getSyncRuntimeState(), getSyncSnapshot(),
    ]);
    setConnection(nextConnection);
    setRegistry(nextRegistry);
    setLoadedRuntime(nextRuntime);
    setConflicts(snapshot.conflicts);
    if (nextConnection) {
      setUrl(nextConnection.url);
      setUsername(nextConnection.username);
    }
  }, []);

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

    // Activation changes the target ID/epoch. Refresh the coordinator before
    // any sync so failure persistence is verified against the new context.
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

  return <section className="mobile-sync-settings" aria-labelledby="mobile-sync-title">
    <header className="mobile-page-heading">
      <div><h1 id="mobile-sync-title">同步设置</h1><p>WebDAV 密码由 Android Keystore 保护，不会回显到页面。</p></div>
      <span className={`mobile-sync-state mobile-sync-state-${stateLabel === '错误' ? 'error' : 'normal'}`}>{stateLabel}</span>
    </header>

    <div className="mobile-sync-card" data-testid="mobile-sync-runtime">
      <h2>当前状态</h2>
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

    <form className="mobile-sync-card mobile-sync-form" onSubmit={event => { event.preventDefault(); void runProbe(); }}>
      <h2>{connection ? '更新或切换目标' : '配置 WebDAV'}</h2>
      <label>WebDAV URL<input type="url" autoCapitalize="none" autoCorrect="off" value={url} onChange={event => editInput(setUrl)(event.target.value)} required /></label>
      <label>用户名<input type="text" autoCapitalize="none" autoCorrect="off" value={username} onChange={event => editInput(setUsername)(event.target.value)} required /></label>
      <label>密码<input type="password" value={password} onChange={event => editInput(setPassword)(event.target.value)} autoComplete="new-password" required /></label>
      <button type="submit" className="mobile-primary-button" disabled={busy !== null || !url.trim() || !username.trim() || !password}>{busy === 'probe' ? '检查中…' : '测试连接'}</button>
      {probe && <div className="mobile-sync-probe" role="region" aria-label="目标检查结果"><p>{probeLabel(probe)}</p><button type="button" className="mobile-primary-button" disabled={busy !== null} onClick={() => void activate()}>{busy === 'activate' ? '激活中…' : '确认激活并首次同步'}</button></div>}
      <p className="mobile-sync-note">激活前只进行只读检查。切换目标不会删除旧目标的待上传、基线、冲突或暂存状态。</p>
    </form>

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

    {registry && registry.targets.length > 1 && <p className="mobile-sync-target-count">已保留 {registry.targets.length} 个隔离同步目标的本地状态。</p>}
  </section>;
}
