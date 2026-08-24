import { useCallback, useEffect, useMemo, useState } from 'react';
import type { WatchRecord } from '../shared/types';
import { useRecordRepository } from '../features/watchlist/hooks/useRecordRepository';
import RecordForm from '../features/watchlist/components/RecordForm';
import { displayTitlesOf } from '../shared/lib/displayTitle';
import NotificationRegion, { useNotifications } from '../shared/components/NotificationRegion';
import ErrorBoundary from '../shared/components/ErrorBoundary';
import { publicFailureMessage } from '../shared/lib/feedback';
import { useMobileNavigation, type AndroidBackAction, type MobileRoute } from '../platform/navigation';

declare global {
  interface Window {
    __WATCHTRACKER_ANDROID_BACK__?: () => AndroidBackAction;
  }
}

type InitializationState = 'loading' | 'ready' | 'error';

const tabs: Array<{ route: Exclude<MobileRoute, 'form'>; label: string; icon: string }> = [
  { route: 'library', label: '片库', icon: '▦' },
  { route: 'discover', label: '发现', icon: '⌕' },
  { route: 'collections', label: '收藏', icon: '♡' },
  { route: 'stats', label: '统计', icon: '◔' },
  { route: 'settings', label: '设置', icon: '⚙' },
];

function localDateString(date = new Date()): string {
  const pad = (value: number) => String(value).padStart(2, '0');
  return `${date.getFullYear()}-${pad(date.getMonth() + 1)}-${pad(date.getDate())}`;
}

function PlaceholderPage({ route }: { route: Exclude<MobileRoute, 'library' | 'form'> }) {
  const tab = tabs.find(item => item.route === route)!;
  return (
    <section className="mobile-placeholder" aria-labelledby="mobile-placeholder-title">
      <span className="mobile-placeholder-icon" aria-hidden="true">{tab.icon}</span>
      <h1 id="mobile-placeholder-title">{tab.label}</h1>
      <p>此功能正在开发中，当前不会修改本地数据。</p>
    </section>
  );
}

function LibraryPage({ records, onEdit, onDelete, onAdd }: {
  records: WatchRecord[];
  onEdit: (record: WatchRecord) => void;
  onDelete: (record: WatchRecord) => void;
  onAdd: () => void;
}) {
  if (!records.length) {
    return (
      <section className="mobile-empty" aria-labelledby="mobile-empty-title">
        <span aria-hidden="true">🎭</span>
        <h1 id="mobile-empty-title">片库还是空的</h1>
        <p>添加第一条记录后，它会保存在本机。</p>
        <button type="button" className="mobile-primary-button" onClick={onAdd}>添加第一条记录</button>
      </section>
    );
  }
  return (
    <section className="mobile-library" aria-labelledby="mobile-library-title">
      <div className="mobile-section-heading">
        <div><p className="mobile-eyebrow">OFFLINE LIBRARY</p><h1 id="mobile-library-title">我的片库</h1></div>
        <span className="mobile-count">{records.length} 条</span>
      </div>
      <div className="mobile-record-list">
        {records.map(record => {
          const titles = displayTitlesOf(record);
          return (
            <article className="mobile-record-card" key={record.id}>
              <div className="mobile-record-card-body">
                <div className="mobile-record-title">{titles.primary || titles.secondary || '未命名记录'}</div>
                {titles.secondary && <div className="mobile-record-subtitle">{titles.secondary}</div>}
                <div className="mobile-record-meta"><span>{record.mediaType || '其他'}</span><span>{record.status}</span>{record.releaseYear && <span>{record.releaseYear}</span>}{record.isLocked && <span aria-label="已锁定">🔒 已锁定</span>}</div>
              </div>
              {!record.isLocked && <div className="mobile-record-actions">
                <button type="button" className="mobile-quiet-button" onClick={() => onEdit(record)}>编辑</button>
                <button type="button" className="mobile-quiet-button mobile-danger" onClick={() => onDelete(record)}>删除</button>
              </div>}
            </article>
          );
        })}
      </div>
    </section>
  );
}

export default function MobileApp() {
  const navigation = useMobileNavigation();
  const { notices, notify, dismiss } = useNotifications();
  const {
    records,
    loadRecords: reloadRecords,
    addRecord,
    updateRecord,
    deleteRecord,
  } = useRecordRepository(() => undefined);
  const [initialization, setInitialization] = useState<InitializationState>('loading');
  const [editingRecord, setEditingRecord] = useState<WatchRecord | null>(null);

  const loadRecords = useCallback(async () => {
    try {
      await reloadRecords();
      setInitialization('ready');
    } catch (error) {
      console.error('[MobileApp.Initialize]', error);
      setInitialization('error');
    }
  }, [reloadRecords]);

  useEffect(() => {
    // Loading crosses the Rust IPC boundary; readiness updates after the await.
    // eslint-disable-next-line react-hooks/set-state-in-effect
    void loadRecords();
  }, [loadRecords]);

  useEffect(() => {
    // MainActivity calls this synchronously through Android's back dispatcher.
    window.__WATCHTRACKER_ANDROID_BACK__ = navigation.handleBack;
    return () => {
      delete window.__WATCHTRACKER_ANDROID_BACK__;
    };
  }, [navigation.handleBack]);

  const openForm = useCallback((record: WatchRecord | null = null) => {
    if (record?.isLocked) {
      notify('warning', '已锁定记录不能编辑。');
      return;
    }
    setEditingRecord(record);
    navigation.navigate('form');
  }, [navigation, notify]);

  const closeForm = useCallback(() => {
    setEditingRecord(null);
    navigation.back();
  }, [navigation]);

  const handleDelete = useCallback(async (record: WatchRecord) => {
    const current = records.find(item => item.id === record.id);
    if (!current) {
      notify('warning', '记录已不存在，请刷新片库。');
      return;
    }
    if (record.isLocked || current.isLocked) {
      notify('warning', '已锁定记录不能删除。');
      return;
    }
    if (!window.confirm(`确定删除“${displayTitlesOf(record).primary}”吗？`)) return;
    try {
      await deleteRecord(record.id);
      notify('success', '记录已删除。');
    } catch (error) {
      console.error('[MobileApp.DeleteRecord]', error);
      notify('error', publicFailureMessage('删除记录'));
    }
  }, [deleteRecord, notify, records]);

  const handleSave = useCallback(async (data: Omit<WatchRecord, 'id' | 'createdAt'>) => {
    if (editingRecord) {
      const current = records.find(item => item.id === editingRecord.id);
      if (!current) {
        notify('warning', '记录已不存在，请刷新片库。');
        return false;
      }
      if (editingRecord.isLocked || current.isLocked) {
        notify('warning', '已锁定记录不能编辑。');
        return false;
      }
    }
    const next = { ...data };
    if (next.status === '在看' && !next.startDate) next.startDate = localDateString();
    if (next.status === '已看' && !next.endDate) next.endDate = localDateString();
    try {
      if (editingRecord) {
        await updateRecord(editingRecord.id, next);
        notify('success', '记录已更新。');
      } else {
        await addRecord(next);
        notify('success', '记录已添加。');
      }
      return true;
    } catch (error) {
      console.error('[MobileApp.SaveRecord]', error);
      notify('error', publicFailureMessage(editingRecord ? '更新记录' : '添加记录'));
      return false;
    }
  }, [addRecord, editingRecord, notify, records, updateRecord]);

  const page = useMemo(() => {
    if (initialization === 'loading') return <div className="mobile-loading" role="status">正在读取本地片库…</div>;
    if (initialization === 'error') return <div className="mobile-error" role="alert"><h1>无法读取本地数据</h1><p>本地数据库未被当作空数据处理，请稍后重试。</p><button type="button" className="mobile-primary-button" onClick={() => { setInitialization('loading'); void loadRecords(); }}>重试</button></div>;
    if (navigation.route === 'library') return <LibraryPage records={records} onEdit={record => openForm(record)} onDelete={handleDelete} onAdd={() => openForm()} />;
    if (navigation.route === 'form') return null;
    return <PlaceholderPage route={navigation.route} />;
  }, [handleDelete, initialization, loadRecords, navigation.route, openForm, records]);

  return (
    <div className="mobile-shell">
      <NotificationRegion notices={notices} onDismiss={dismiss} />
      <header className="mobile-topbar"><span className="mobile-brand-mark" aria-hidden="true">◈</span><span>WatchTracker</span><span className="mobile-offline-badge">离线优先</span></header>
      <main className="mobile-content"><ErrorBoundary key={navigation.route}>{page}</ErrorBoundary></main>
      {navigation.route === 'form' && (
        <RecordForm
          record={editingRecord}
          onSave={async data => handleSave(data)}
          onDelete={recordId => { const target = records.find(item => item.id === recordId); if (target) void handleDelete(target); }}
          onNotify={notify}
          onClose={closeForm}
          capabilities={{ tmdb: false, collections: false, sync: false, mobile: true }}
        />
      )}
      <button type="button" className="mobile-fab" aria-label="添加记录" onClick={() => openForm()}><span aria-hidden="true">＋</span></button>
      <nav className="mobile-bottom-nav" aria-label="主导航">
        {tabs.map(tab => (
          <button key={tab.route} type="button" className="mobile-nav-item" aria-current={navigation.route === tab.route ? 'page' : undefined} onClick={() => navigation.navigate(tab.route)}>
            <span className="mobile-nav-icon" aria-hidden="true">{tab.icon}</span><span>{tab.label}</span>
          </button>
        ))}
      </nav>
    </div>
  );
}
