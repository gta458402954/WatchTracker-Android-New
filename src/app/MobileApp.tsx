import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import type { Status, WatchRecord } from '../shared/types';
import type { RecordFormValues } from '../features/watchlist/record-form/recordFormModel';
import { useMobileRecordRepository } from '../features/watchlist/hooks/useMobileRecordRepository';
import MobileLibraryPage from '../features/watchlist/components/MobileLibraryPage';
import { displayTitlesOf } from '../shared/lib/displayTitle';
import NotificationRegion, { useNotifications } from '../shared/components/NotificationRegion';
import ErrorBoundary from '../shared/components/ErrorBoundary';
import { publicFailureMessage } from '../shared/lib/feedback';
import { useMobileNavigation, type AndroidBackAction, type MobileRoute } from '../platform/navigation';
import type { EpisodeTracking } from '../shared/lib/database.ts';
import type { MobileEpisodeAction } from '../features/watchlist/mobileEpisodeTracking.ts';
import { useSyncCoordinator } from '../features/sync/hooks/useSyncCoordinator.ts';
import MobileSyncSettingsPage from '../features/settings/components/MobileSyncSettingsPage.tsx';

declare global { interface Window { __WATCHTRACKER_ANDROID_BACK__?: () => AndroidBackAction; } }
type InitializationState = 'loading' | 'ready' | 'error';
const tabs: Array<{ route: Exclude<MobileRoute, 'form' | 'detail'>; label: string; icon: string }> = [
  { route: 'library', label: '片库', icon: '▦' }, { route: 'discover', label: '发现', icon: '⌕' }, { route: 'collections', label: '收藏', icon: '♡' }, { route: 'stats', label: '统计', icon: '◔' }, { route: 'settings', label: '设置', icon: '⚙' },
];
function localDateString(date = new Date()): string { const pad = (value: number) => String(value).padStart(2, '0'); return `${date.getFullYear()}-${pad(date.getMonth() + 1)}-${pad(date.getDate())}`; }
function PlaceholderPage({ route }: { route: Exclude<MobileRoute, 'library' | 'form' | 'detail'> }) { const tab = tabs.find(item => item.route === route)!; return <section className="mobile-placeholder" aria-labelledby="mobile-placeholder-title"><span className="mobile-placeholder-icon" aria-hidden="true">{tab.icon}</span><h1 id="mobile-placeholder-title">{tab.label}</h1><p>此功能正在开发中，当前不会修改本地数据。</p></section>; }

export default function MobileApp() {
  const navigation = useMobileNavigation();
  const { notices, notify, dismiss } = useNotifications();
  const scheduleLocalWriteRef = useRef<() => void | Promise<void>>(() => undefined);
  const repository = useMobileRecordRepository(() => scheduleLocalWriteRef.current());
  const { records, reloadRecords, addRecord, updateMobileRecord, deleteMobileRecord, changeMobileEpisode } = repository;
  const onBackgroundSyncError = useCallback((message: string) => notify('warning', message), [notify]);
  const coordinator = useSyncCoordinator(30, 15, reloadRecords, onBackgroundSyncError);
  const { scheduleLocalWrite, startCoordinator, syncNow, syncRuntime, isSyncing, toggleSyncPause, notifySyncConfigurationChanged } = coordinator;
  useEffect(() => { scheduleLocalWriteRef.current = scheduleLocalWrite; }, [scheduleLocalWrite]);
  const [initialization, setInitialization] = useState<InitializationState>('loading');
  const initializationPromise = useRef<Promise<void> | null>(null);
  const loadRecords = useCallback(() => {
    if (initializationPromise.current) return initializationPromise.current;
    const pending = (async () => { try { await new Promise(resolve => window.setTimeout(resolve, 1500)); await reloadRecords(); setInitialization('ready'); void startCoordinator().catch(() => onBackgroundSyncError(publicFailureMessage('启动同步'))); } catch (error) { console.error('[MobileApp.Initialize]', error); setInitialization('error'); } finally { initializationPromise.current = null; } })();
    initializationPromise.current = pending;
    return pending;
  }, [onBackgroundSyncError, reloadRecords, startCoordinator]);
  useEffect(() => { // Loading crosses the Rust IPC boundary; readiness is updated after await.
    void loadRecords();
  }, [loadRecords]);
  useEffect(() => { window.__WATCHTRACKER_ANDROID_BACK__ = navigation.handleBack; return () => { delete window.__WATCHTRACKER_ANDROID_BACK__; }; }, [navigation.handleBack]);

  const openForm = useCallback((mode: 'new' | 'edit', record?: WatchRecord) => { if (mode === 'edit' && record?.isLocked) { notify('warning', '已锁定记录不能编辑。'); return; } navigation.navigateForm(mode, record?.id); }, [navigation, notify]);
  const onBack = useCallback(() => navigation.back(), [navigation]);
  const saveNew = useCallback(async (data: RecordFormValues) => { const next = { ...data }; if (next.status === '在看' && !next.startDate) next.startDate = localDateString(); if (next.status === '已看' && !next.endDate) next.endDate = localDateString(); try { await addRecord(next); notify('success', '记录已添加。'); return true; } catch (error) { console.error('[MobileApp.MobileAdd]', error); notify('error', publicFailureMessage('添加记录')); return false; } }, [addRecord, notify]);
  const saveUpdate = useCallback(async (record: WatchRecord, data: RecordFormValues) => { const next = { ...data }; if (next.status === '在看' && !next.startDate) next.startDate = localDateString(); if (next.status === '已看' && !next.endDate) next.endDate = localDateString(); const result = await updateMobileRecord(record.id, next, record.rev ?? 0, 'edit'); if (result.ok) { notify('success', '记录已更新。'); return true; } notify(result.reason === 'error' ? 'error' : 'warning', result.reason === 'stale' ? '记录已被其他操作更新，请重新打开。' : result.reason === 'locked' ? '已锁定记录不能编辑。' : result.reason === 'missing' ? '记录已不存在。' : publicFailureMessage('更新记录')); return false; }, [notify, updateMobileRecord]);
  const remove = useCallback(async (record: WatchRecord) => { if (!window.confirm(`确定删除“${displayTitlesOf(record).primary || record.originalName}”吗？`)) return false; const result = await deleteMobileRecord(record.id, record.rev ?? 0); if (result.ok) { notify('success', '记录已删除。'); navigation.navigate('library'); return true; } notify(result.reason === 'error' ? 'error' : 'warning', result.reason === 'stale' ? '记录已被其他操作更新，删除已取消。' : result.reason === 'locked' ? '已锁定记录不能删除。' : '记录已不存在。'); return false; }, [deleteMobileRecord, navigation, notify]);
  const lock = useCallback(async (record: WatchRecord, locked: boolean) => { const result = await updateMobileRecord(record.id, { isLocked: locked }, record.rev ?? 0, locked ? 'lock' : 'unlock'); if (result.ok) { notify('success', locked ? '记录已锁定。' : '记录已解锁。'); return true; } notify(result.reason === 'stale' ? 'warning' : 'error', result.reason === 'stale' ? '记录已被其他操作更新，请重新打开。' : publicFailureMessage(locked ? '锁定记录' : '解锁记录')); return false; }, [notify, updateMobileRecord]);
  const status = useCallback(async (record: WatchRecord, nextStatus: Status) => { const updates: Partial<WatchRecord> = { status: nextStatus }; if (nextStatus === '在看' && !record.startDate) updates.startDate = localDateString(); if (nextStatus === '已看' && !record.endDate) updates.endDate = localDateString(); const result = await updateMobileRecord(record.id, updates, record.rev ?? 0, 'status'); if (result.ok) { notify('success', `状态已更新为“${nextStatus}”。`); return true; } notify(result.reason === 'stale' ? 'warning' : 'error', result.reason === 'stale' ? '记录已被其他操作更新，请重新打开。' : result.reason === 'locked' ? '已锁定记录不能修改。' : publicFailureMessage('更新状态')); return false; }, [notify, updateMobileRecord]);
  const episode = useCallback(async (record: WatchRecord, nextEpisode: number | null, action: MobileEpisodeAction): Promise<EpisodeTracking | null> => {
    const result = await changeMobileEpisode(record, nextEpisode);
    if (!result.ok) {
      const messages = {
        stale: '逐集进度已被其他操作更新，已重新载入，请确认后重试。',
        locked: '记录已锁定，逐集进度保持不变。',
        missing: '记录已不存在，已重新载入片库。',
        domain: '逐集进度未修改，请确认总集数和当前状态后重试。',
        error: '逐集进度更新失败，原有界面状态已保留。',
      } as const;
      notify(result.reason === 'error' ? 'error' : 'warning', messages[result.reason]);
      return null;
    }
    const current = record.nextEpisode;
    const persistedNext = result.tracking.record.nextEpisode;
    const message = action === 'enable'
      ? `逐集跟踪已启用，下一集为第 ${persistedNext} 集。`
      : action === 'complete'
        ? persistedNext === null ? `第 ${current} 集已完成，这条记录已完结。` : `第 ${current} 集已完成，下一集为第 ${persistedNext} 集。`
        : action === 'retreat' ? `下一集已回退至第 ${persistedNext} 集，原有历史已保留。`
          : action === 'finish' ? '已原子完成剩余集数并标记完结。'
            : action === 'resume' ? `已从第 ${persistedNext} 集继续追更。`
              : `下一集已跳至第 ${persistedNext} 集。`;
    notify('success', message);
    return result.tracking;
  }, [changeMobileEpisode, notify]);
  const page = useMemo(() => { if (initialization === 'loading') return <div className="mobile-loading" role="status">正在读取本地片库…</div>; if (initialization === 'error') return <div className="mobile-error" role="alert"><h1>无法读取本地数据</h1><p>本地数据库未被当作空数据处理，请稍后重试。</p><button type="button" className="mobile-primary-button" onClick={() => { setInitialization('loading'); void loadRecords(); }}>重试</button></div>; if (navigation.route === 'library' || navigation.route === 'detail' || navigation.route === 'form') return <MobileLibraryPage route={navigation.route} records={records} detailId={navigation.detailId} formMode={navigation.formMode} onDetail={record => navigation.navigateDetail(record.id)} onForm={openForm} onBack={onBack} onAdd={saveNew} onUpdate={saveUpdate} onDelete={remove} onLock={lock} onStatus={status} onEpisode={episode} onNotify={notify} />; if (navigation.route === 'settings') return <MobileSyncSettingsPage runtime={syncRuntime} isSyncing={isSyncing} onSyncNow={syncNow} onTogglePause={toggleSyncPause} onConfigurationChanged={notifySyncConfigurationChanged} onSyncWorkQueued={scheduleLocalWrite} onReloadRecords={reloadRecords} notify={notify} />; return <PlaceholderPage route={navigation.route} />; }, [episode, initialization, isSyncing, loadRecords, lock, navigation, notify, notifySyncConfigurationChanged, onBack, openForm, records, reloadRecords, remove, saveNew, saveUpdate, scheduleLocalWrite, status, syncNow, syncRuntime, toggleSyncPause]);
  const immersive = navigation.route === 'detail' || navigation.route === 'form';
  return <div className="mobile-shell"><NotificationRegion notices={notices} onDismiss={dismiss} /><main className={`mobile-content ${immersive ? 'mobile-content-immersive' : ''}`}><ErrorBoundary>{page}</ErrorBoundary></main>{!immersive && <><button type="button" className="mobile-fab" aria-label="添加记录" onClick={() => openForm('new')}><span aria-hidden="true">＋</span></button><nav className="mobile-bottom-nav" aria-label="主导航">{tabs.map(tab => <button key={tab.route} type="button" className="mobile-nav-item" aria-current={navigation.route === tab.route ? 'page' : undefined} onClick={() => navigation.navigate(tab.route)}><span className="mobile-nav-icon" aria-hidden="true">{tab.icon}</span><span>{tab.label}</span></button>)}</nav></>}</div>;
}
