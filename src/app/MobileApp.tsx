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
  const repository = useMobileRecordRepository(() => undefined);
  const { records, reloadRecords, addRecord, updateMobileRecord, deleteMobileRecord } = repository;
  const [initialization, setInitialization] = useState<InitializationState>('loading');
  const initializationPromise = useRef<Promise<void> | null>(null);
  const loadRecords = useCallback(() => {
    if (initializationPromise.current) return initializationPromise.current;
    const pending = (async () => { try { await new Promise(resolve => window.setTimeout(resolve, 1500)); await reloadRecords(); setInitialization('ready'); } catch (error) { console.error('[MobileApp.Initialize]', error); setInitialization('error'); } finally { initializationPromise.current = null; } })();
    initializationPromise.current = pending;
    return pending;
  }, [reloadRecords]);
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
  const page = useMemo(() => { if (initialization === 'loading') return <div className="mobile-loading" role="status">正在读取本地片库…</div>; if (initialization === 'error') return <div className="mobile-error" role="alert"><h1>无法读取本地数据</h1><p>本地数据库未被当作空数据处理，请稍后重试。</p><button type="button" className="mobile-primary-button" onClick={() => { setInitialization('loading'); void loadRecords(); }}>重试</button></div>; if (navigation.route === 'library' || navigation.route === 'detail' || navigation.route === 'form') return <MobileLibraryPage route={navigation.route} records={records} detailId={navigation.detailId} formMode={navigation.formMode} onDetail={record => navigation.navigateDetail(record.id)} onForm={openForm} onBack={onBack} onAdd={saveNew} onUpdate={saveUpdate} onDelete={remove} onLock={lock} onStatus={status} onNotify={notify} />; return <PlaceholderPage route={navigation.route} />; }, [initialization, loadRecords, lock, navigation, notify, onBack, openForm, records, remove, saveNew, saveUpdate, status]);
  const immersive = navigation.route === 'detail' || navigation.route === 'form';
  return <div className="mobile-shell"><NotificationRegion notices={notices} onDismiss={dismiss} />{!immersive && <header className="mobile-topbar"><span className="mobile-brand-mark" aria-hidden="true">◈</span><span>WatchTracker</span><span className="mobile-offline-badge">离线优先</span></header>}<main className={`mobile-content ${immersive ? 'mobile-content-immersive' : ''}`}><ErrorBoundary>{page}</ErrorBoundary></main>{!immersive && <><button type="button" className="mobile-fab" aria-label="添加记录" onClick={() => openForm('new')}><span aria-hidden="true">＋</span></button><nav className="mobile-bottom-nav" aria-label="主导航">{tabs.map(tab => <button key={tab.route} type="button" className="mobile-nav-item" aria-current={navigation.route === tab.route ? 'page' : undefined} onClick={() => navigation.navigate(tab.route)}><span className="mobile-nav-icon" aria-hidden="true">{tab.icon}</span><span>{tab.label}</span></button>)}</nav></>}</div>;
}
