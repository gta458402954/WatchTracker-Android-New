import { lazy, Suspense, useCallback, useState, useMemo, useEffect, useRef } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { WatchRecord, MediaType, Status, type CollectionDraft } from '../shared/types';
import { useWatchList } from '../features/watchlist/hooks/useWatchList';
import StatsBar from '../features/watchlist/components/StatsBar';
import RecordForm from '../features/watchlist/components/RecordForm';
import SettingsModal from '../features/settings/components/SettingsModal';
import { hasCreds, syncFailureMessage } from '../shared/lib/webdav';
import { calculateWatchValue } from '../shared/lib/analytics';
import { aggregateRegions, mediaTypeOf } from '../shared/lib/classification';
import { countryLabelOf, type RegionFilter } from '../shared/lib/countryNames';
import { initializeApp } from './initialization';
import NotificationRegion, { useNotifications } from '../shared/components/NotificationRegion';
import { notifyOperationFailure, publicFailureMessage, reportOperationFailure } from '../shared/lib/feedback';

// New Split Components
import Header from '../features/watchlist/components/Header';
import ListView from '../features/watchlist/components/ListView';
import PosterWall from '../features/watchlist/components/PosterWall';
import AdvancedFilterPanel from '../features/watchlist/components/AdvancedFilterPanel';
import SavedViewMenu from '../features/watchlist/components/SavedViewMenu';
import ActiveFilterSummary from '../features/watchlist/components/ActiveFilterSummary';
import { useSavedWatchlistViews } from '../features/watchlist/hooks/useSavedWatchlistViews';
import {
  EMPTY_WATCHLIST_QUERY,
  activeQueryDimensionCount,
  activeAdvancedQueryDimensionCount,
  advancedQuerySummaryItems,
  clearAdvancedQuery,
  filterRecordsByQuery,
  normalizeWatchlistQuery,
  watchlistFilterOptions,
  watchlistQueriesEqual,
  type SortBy,
  type ViewMode,
  type WatchlistQueryV1,
  type QueryDimension,
} from '../shared/lib/watchlistQuery';
import type { SavedWatchlistViewV1 } from '../shared/lib/savedViews';
import { useCollections } from '../features/collections/hooks/useCollections';
import CollectionCenter from '../features/collections/components/CollectionCenter';
import { completeMovieCollection, createMissingSeasons, createRecoveryPoint } from '../shared/lib/database';

type InitializationState = 'loading' | 'ready' | 'error';

interface DatabaseCompatibilityIssue {
  code: 'unsupported_newer_database' | 'v19_downgrade_failed';
  detectedVersion: number;
  supportedVersion: number;
}

const Dashboard = lazy(() => import('../features/dashboard/components/Dashboard'));

function localDateString(date = new Date()): string {
  const pad = (value: number) => String(value).padStart(2, '0');
  return date.getFullYear() + '-' + pad(date.getMonth() + 1) + '-' + pad(date.getDate());
}

function savedViewErrorMessage(error: unknown, fallback: string): string {
  const message = error instanceof Error ? error.message : '';
  return /^(请输入视图名称|视图名称不能超过|已存在同名视图|最多保存|保存视图已不存在|无法将不存在的视图)/.test(message)
    ? message
    : fallback;
}

export default function App() {
  const [syncInterval, setSyncInterval] = useState(30);
  const [pullIntervalMinutes, setPullIntervalMinutes] = useState(15);
  const { notices, notify, dismiss } = useNotifications();
  const handleBackgroundError = useCallback((message: string) => {
    notify('warning', message);
  }, [notify]);
  const {
    records, loadRecords, addRecord, updateRecord, deleteRecord, changeNextEpisode, replaceRecords, syncNow,
    syncRuntime, isSyncPaused, toggleSyncPause, notifySyncConfigurationChanged, reloadAndSchedule,
  } = useWatchList(syncInterval, pullIntervalMinutes, handleBackgroundError);

  const [query, setQuery] = useState<WatchlistQueryV1>(() => normalizeWatchlistQuery(EMPTY_WATCHLIST_QUERY));
  const [showForm, setShowForm] = useState(false);
  const [editingRecord, setEditingRecord] = useState<WatchRecord | null>(null);
  const [sortBy, setSortBy] = useState<SortBy>('createdAt');
  const [showSettings, setShowSettings] = useState(false);
  const [settingsInitialTab, setSettingsInitialTab] = useState<'basic' | 'sync'>('basic');
  const [showDashboard, setShowDashboard] = useState(false);
  const [syncing, setSyncing] = useState(false);
  const [syncMsg, setSyncMsg] = useState<string>('');
  const [initializationState, setInitializationState] = useState<InitializationState>('loading');
  const [databaseCompatibilityIssue, setDatabaseCompatibilityIssue] = useState<DatabaseCompatibilityIssue | null>(null);
  const migrationNoticeShownRef = useRef(false);
  const [viewMode, setViewMode] = useState<ViewMode>('list');
  const [showAdvancedFilters, setShowAdvancedFilters] = useState(false);
  const [activeViewId, setActiveViewId] = useState<string | null>(null);
  const startupViewAppliedRef = useRef(false);
  const [hasWebDAVCreds, setHasWebDAVCreds] = useState(false);
  const handleSavedViewError = useCallback((message: string) => notify('warning', message), [notify]);
  const savedViews = useSavedWatchlistViews(initializationState === 'ready', handleSavedViewError);
  const collectionState = useCollections();
  const refreshCollections = collectionState.refresh;
  const [showCollections, setShowCollections] = useState(false);
  const [returnToCollectionsAfterForm, setReturnToCollectionsAfterForm] = useState(false);
  const [collectionResumeId, setCollectionResumeId] = useState<string | null>(null);

  const applySavedView = useCallback((view: SavedWatchlistViewV1) => {
    setQuery(normalizeWatchlistQuery(view.query));
    setSortBy(view.sortBy);
    setViewMode(view.viewMode);
    setActiveViewId(view.id);
  }, []);

  useEffect(() => {
    if (!savedViews.loaded || startupViewAppliedRef.current) return;
    startupViewAppliedRef.current = true;
    const startup = savedViews.views.find(view => view.id === savedViews.startupViewId);
    // The saved startup view is external persisted state and is applied once after loading.
    // eslint-disable-next-line react-hooks/set-state-in-effect
    if (startup) applySavedView(startup);
  }, [applySavedView, savedViews.loaded, savedViews.startupViewId, savedViews.views]);

  const loadInitialState = useCallback(async () => {
    try {
      const compatibilityIssue = await invoke<DatabaseCompatibilityIssue | null>('get_database_compatibility');
      if (compatibilityIssue) {
        setDatabaseCompatibilityIssue(compatibilityIssue);
        setInitializationState('error');
        return;
      }
      setDatabaseCompatibilityIssue(null);
      const result = await initializeApp({
        readCredentials: hasCreds,
        readSyncInterval: () => invoke<string | null>('get_setting', { key: 'sync_interval' }),
        readPullInterval: () => invoke<string | null>('get_setting', { key: 'sync_pull_interval_minutes' }),
        readRecords: loadRecords,
      });
      setHasWebDAVCreds(result.hasWebDAVCredentials);
      setSyncInterval(result.syncInterval);
      setPullIntervalMinutes(result.pullIntervalMinutes);
      await refreshCollections();
      setInitializationState('ready');
      try {
        const migrationNotice = await invoke<string | null>('get_setting', { key: 'database_migration_notice' });
        if (migrationNotice === 'v19_to_v18' && !migrationNoticeShownRef.current) {
          migrationNoticeShownRef.current = true;
          notify('success', '数据库已安全从 V19 转换为 V18，转换前备份已保存在 backups 目录。');
          await invoke('set_setting', { key: 'database_migration_notice', value: '' });
        }
      } catch (noticeError) {
        reportOperationFailure('App.DatabaseMigrationNotice', noticeError);
      }
    } catch (error) {
      reportOperationFailure('App.Initialize', error);
      setInitializationState('error');
    }
  }, [loadRecords, notify, refreshCollections]);

  const retryInitialization = useCallback(() => {
    setInitializationState('loading');
    void loadInitialState();
  }, [loadInitialState]);

  useEffect(() => {
    // Initialization crosses the Tauri IPC boundary; all state updates occur
    // after the awaited reads complete.
    // eslint-disable-next-line react-hooks/set-state-in-effect
    void loadInitialState();
  }, [loadInitialState]);

  useEffect(() => {
    const openNewRecord = (event: KeyboardEvent) => {
      if (!(event.ctrlKey || event.metaKey) || event.key.toLowerCase() !== 'n') return;
      const target = event.target as HTMLElement | null;
      if (target?.closest('input, textarea, select, [contenteditable="true"]') || document.querySelector('[role="dialog"]')) return;
      event.preventDefault();
      setEditingRecord(null);
      setShowForm(true);
    };
    document.addEventListener('keydown', openNewRecord);
    return () => document.removeEventListener('keydown', openNewRecord);
  }, []);

  // 手动同步到坚果云 WebDAV
  async function handleQuickSync() {
    setSyncing(true);
    setSyncMsg('');
    try {
      if (!(await hasCreds())) {
        const message = '请先在设置里配置 WebDAV 账号。';
        setSyncMsg(`⚙️ ${message}`);
        notify('warning', message);
        return;
      }
      const result = await syncNow();
      if (result.ok) {
        if (result.conflictCount) {
          const message = `同步完成，有 ${result.conflictCount} 项冲突需要在设置中选择。`;
          setSyncMsg(`⚠️ ${result.conflictCount} 项冲突`);
          notify('warning', message);
        } else {
          setSyncMsg('✅ 已同步');
          notify('success', '同步完成。');
        }
      } else {
        const safeDetail = syncFailureMessage(result.error);
        const message = safeDetail ?? publicFailureMessage('同步');
        setSyncMsg(`${safeDetail ? '⚠️' : '❌'} ${message}`);
        notify(safeDetail ? 'warning' : 'error', message);
      }
    } catch (error) {
      reportOperationFailure('App.QuickSync', error);
      const message = publicFailureMessage('同步');
      setSyncMsg(`❌ ${message}`);
      notify('error', message);
    } finally {
      setSyncing(false);
      setTimeout(() => setSyncMsg(''), 3000);
    }
  }

  const regionOptions = useMemo(() => {
    const options = aggregateRegions(records.filter(record =>
      (query.mediaTypes.length === 0 || query.mediaTypes.includes(mediaTypeOf(record)))
      && (query.statuses.length === 0 || query.statuses.includes(record.status)),
    ));
    const visible = new Set(options.map(option => option.code));
    return [
      ...options,
      ...query.regions.filter(code => !visible.has(code)).map(code => ({ code, label: countryLabelOf(code), count: 0 })),
    ];
  }, [query.mediaTypes, query.regions, query.statuses, records]);
  const advancedOptions = useMemo(() => watchlistFilterOptions(records), [records]);
  const filterSummary = useMemo(() => advancedQuerySummaryItems(query), [query]);
  const collectionNamesByRecord = useMemo(() => {
    const names = new Map(collectionState.collections.map(collection => [collection.id, collection.name]));
    const result = new Map<string, string[]>();
    for (const member of collectionState.members) {
      const name = names.get(member.collectionId); if (!name) continue;
      result.set(member.recordId, [...(result.get(member.recordId) ?? []), name]);
    }
    return result;
  }, [collectionState.collections, collectionState.members]);

  const filtered = useMemo(() => {
    return filterRecordsByQuery(records, query)
      .sort((a, b) => {
        if (sortBy === 'rating') {
          return (b.rating ?? -1) - (a.rating ?? -1);
        }
        if (sortBy === 'releaseYear') {
          return (b.releaseYear || '').localeCompare(a.releaseYear || '', undefined, { numeric: true });
        }
        if (sortBy === 'endDate') {
          if (!a.endDate && !b.endDate) return 0;
          if (!a.endDate) return 1;
          if (!b.endDate) return -1;
          return b.endDate.localeCompare(a.endDate);
        }
        if (sortBy === 'watchValue') {
          const valA = calculateWatchValue(a, records);
          const valB = calculateWatchValue(b, records);
          return valB - valA;
        }
        return (b.createdAt || '').localeCompare(a.createdAt || '');
      });
  }, [records, query, sortBy]);

  const activeView = savedViews.views.find(view => view.id === activeViewId) ?? null;
  const viewDirty = activeView
    ? !watchlistQueriesEqual(query, activeView.query) || sortBy !== activeView.sortBy || viewMode !== activeView.viewMode
    : activeQueryDimensionCount(query) > 0 || sortBy !== 'createdAt' || viewMode !== 'list';

  const viewSnapshot = useCallback(() => ({ query: normalizeWatchlistQuery(query), sortBy, viewMode }), [query, sortBy, viewMode]);

  function resetToAllRecords() {
    setQuery(normalizeWatchlistQuery(EMPTY_WATCHLIST_QUERY));
    setSortBy('createdAt');
    setViewMode('list');
    setActiveViewId(null);
  }

  function resetQueryConditions() {
    setQuery(normalizeWatchlistQuery(EMPTY_WATCHLIST_QUERY));
    setActiveViewId(null);
  }

  function clearQueryDimension(dimension: QueryDimension) {
    const empty = normalizeWatchlistQuery(EMPTY_WATCHLIST_QUERY);
    setQuery(current => normalizeWatchlistQuery({ ...current, [dimension]: empty[dimension] }));
  }

  async function createSavedView(name: string) {
    try {
      const created = await savedViews.createView(name, viewSnapshot());
      setActiveViewId(created.id);
      notify('success', `已保存视图“${created.name}”。`);
    } catch (error) {
      notify('error', savedViewErrorMessage(error, '保存视图失败，请稍后重试。'));
      throw error;
    }
  }

  async function updateSavedView() {
    if (!activeViewId) return;
    try {
      await savedViews.updateView(activeViewId, viewSnapshot());
      notify('success', '保存视图已更新。');
    } catch (error) {
      notify('error', savedViewErrorMessage(error, '更新视图失败，请稍后重试。'));
    }
  }

  async function deleteSavedView() {
    if (!activeView || !confirm(`确定删除视图“${activeView.name}”吗？当前筛选条件会保留。`)) return;
    try {
      await savedViews.deleteView(activeView.id);
      setActiveViewId(null);
      notify('success', '保存视图已删除，当前条件作为临时筛选保留。');
    } catch (error) {
      notify('error', savedViewErrorMessage(error, '删除视图失败，请稍后重试。'));
    }
  }

  async function toggleStartupView() {
    if (!activeView) return;
    try {
      const next = savedViews.startupViewId === activeView.id ? null : activeView.id;
      await savedViews.setStartupViewId(next);
      notify('success', next ? `“${activeView.name}”已设为启动视图。` : '已取消启动视图。');
    } catch (error) {
      notify('error', savedViewErrorMessage(error, '更新启动视图失败，请稍后重试。'));
    }
  }


  function handleEdit(record: WatchRecord) {
    setReturnToCollectionsAfterForm(false);
    setEditingRecord(record);
    setShowForm(true);
  }

  function handleCollectionRecordEdit(record: WatchRecord, collectionId: string) {
    setCollectionResumeId(collectionId);
    setReturnToCollectionsAfterForm(true);
    setShowCollections(false);
    setEditingRecord(record);
    setShowForm(true);
  }

  async function handleSave(data: Omit<WatchRecord, 'id' | 'createdAt'>, collectionIds: string[] = [], collectionDrafts: CollectionDraft[] = []) {
    const today = localDateString();
    if (data.status === '在看' && !data.startDate) data.startDate = today;
    if (data.status === '已看' && !data.endDate) data.endDate = today;

    try {
      let savedRecord: WatchRecord;
      if (editingRecord) {
        if (data.imdbId) {
          const duplicate = records.find(r => r.imdbId === data.imdbId && r.id !== editingRecord.id);
          if (duplicate) {
            if (!window.confirm(`检测到已有相同 IMDb ID (${data.imdbId}) 的记录：【${duplicate.chineseName}】。\n是否继续修改？`)) {
              return false;
            }
          }
        }
        savedRecord = await updateRecord(editingRecord.id, data);
        notify('success', '记录已更新。');
      } else {
        if (data.imdbId) {
          const duplicate = records.find(r => r.imdbId === data.imdbId);
          if (duplicate) {
            if (!window.confirm(`检测到已有相同 IMDb ID (${data.imdbId}) 的记录：【${duplicate.chineseName}】。\n是否继续添加？`)) {
              return false;
            }
          }
        }
        savedRecord = await addRecord(data);
        notify('success', '记录已添加。');
      }
      try {
        const currentMembers = collectionState.members.filter(member => member.recordId === savedRecord.id);
        const selected = new Set(collectionIds);
        for (const member of currentMembers.filter(member => !selected.has(member.collectionId))) await collectionState.removeMember(member);
        const currentIds = new Set(currentMembers.map(member => member.collectionId));
        for (const collection of collectionState.collections.filter(collection => selected.has(collection.id) && !currentIds.has(collection.id))) await collectionState.addMembers(collection, [savedRecord.id]);
        for (const draft of collectionDrafts) {
          await collectionState.createForRecord(draft.name, draft.description, draft.collectionKind, savedRecord.id);
        }
      } catch (membershipError) {
        reportOperationFailure('App.SaveRecordCollections', membershipError);
        notify('warning', '条目已保存，但收藏集更新失败，请在收藏集中心重试。');
      }
      setEditingRecord(null);
      return true;
    } catch (error) {
      notifyOperationFailure('App.SaveRecord', editingRecord ? '更新记录' : '添加记录', error, notify);
      return false;
    }
  }

  function handleCloseForm() {
    setShowForm(false);
    setEditingRecord(null);
    if (returnToCollectionsAfterForm) {
      setReturnToCollectionsAfterForm(false);
      setShowCollections(true);
    }
  }

  async function handleDelete(id: string) {
    if (confirm('确定要删除这条记录吗？')) {
      try {
        await deleteRecord(id);
        await collectionState.refresh();
        notify('success', '记录已删除。');
      } catch (error) {
        reportOperationFailure('App.DeleteRecord', error);
        notify('error', publicFailureMessage('删除记录'));
      }
    }
  }

  async function handleImport(imported: WatchRecord[]) {
    await replaceRecords(imported);
  }

  async function handleLockToggle(id: string) {
    const r = records.find(r => r.id === id);
    if (r) {
      try {
        await updateRecord(id, { isLocked: !r.isLocked });
      } catch (error) {
        reportOperationFailure('App.ToggleLock', error);
        notify('error', publicFailureMessage('更新锁定状态'));
      }
    }
  }

  async function handleStatusChange(id: string, status: Status) {
    const record = records.find(r => r.id === id);
    const updates: Partial<WatchRecord> = { status };

    if (status === '在看') {
      if (record && !record.startDate) {
        updates.startDate = localDateString();
      }
    }
    if (status === '已看') {
      if (record && !record.endDate) {
        updates.endDate = localDateString();
      }
      if (record) {
        const episodic = Boolean(record.totalEpisodes) || ['剧集', '综艺'].includes(mediaTypeOf(record));
        if (!episodic && record.movieDuration) updates.movieProgress = record.movieDuration;
        if (episodic) updates.progress = record.totalEpisodes ? '第' + record.totalEpisodes + '集' : '完结';
      }
    }
    try {
      await updateRecord(id, updates);
    } catch (error) {
      reportOperationFailure('App.ChangeStatus', error);
      notify('error', publicFailureMessage('更新观看状态'));
    }
  }

  async function handleNextEpisodeChange(record: WatchRecord, nextEpisode: number | null) {
    try {
      const result = await changeNextEpisode(record, nextEpisode);
      const unknown = result.completions.filter(item => item.completedAt === null).length;
      notify('success', nextEpisode === null
        ? `已记录完结，共有 ${result.completions.length} 集完成记录。`
        : `下一集已设为第 ${nextEpisode} 集${unknown ? `；${unknown} 集时间未记录` : ''}。`);
    } catch (error) {
      reportOperationFailure('App.ChangeNextEpisode', error);
      const message = String(error);
      notify('error', message.includes('stale_episode_progress')
        ? '记录已在其他操作中更新，请重试。'
        : message.includes('episode_total_mismatch')
          ? '总集数小于现有下一集或完成历史，请先修正总集数。'
          : message.includes('episode_record_locked')
            ? '条目已锁定，无法更新逐集进度。'
            : publicFailureMessage('更新下一集'));
    }
  }

  return (
    <div className="h-screen overflow-hidden bg-gray-50 flex flex-col">
      <NotificationRegion notices={notices} onDismiss={dismiss} />
      {/* Top Header Bar */}
      <Header
        searchText={query.searchText}
        onSearchChange={searchText => setQuery(current => ({ ...current, searchText }))}
        sortBy={sortBy}
        onSortByChange={setSortBy}
        hasWebDAVCreds={hasWebDAVCreds}
        syncing={syncing}
        syncMsg={syncMsg}
        syncRuntime={syncRuntime}
        onQuickSync={handleQuickSync}
        isSyncPaused={isSyncPaused}
        onToggleSyncPause={toggleSyncPause}
        viewMode={viewMode}
        onViewModeChange={setViewMode}
        onShowDashboard={() => setShowDashboard(true)}
        onShowCollections={() => { setCollectionResumeId(null); setShowCollections(true); }}
        onShowSettings={() => { setSettingsInitialTab('basic'); setShowSettings(true); }}
        onShowSyncSettings={() => { setSettingsInitialTab('sync'); setShowSettings(true); }}
        onShowForm={() => setShowForm(true)}
        activeFilterCount={activeAdvancedQueryDimensionCount(query)}
        onShowAdvancedFilters={() => setShowAdvancedFilters(true)}
        savedViewControl={<SavedViewMenu
          views={savedViews.views}
          activeViewId={activeViewId}
          startupViewId={savedViews.startupViewId}
          dirty={viewDirty}
          onSelectAll={resetToAllRecords}
          onSelect={applySavedView}
          onCreate={createSavedView}
          onUpdate={updateSavedView}
          onDelete={deleteSavedView}
          onToggleStartup={toggleStartupView}
        />}
      />

      {/* Stats and media type tabs */}
      <StatsBar
        records={records}
        regionOptions={regionOptions}
        activeMediaTypes={query.mediaTypes}
        onMediaTypeChange={(mediaType: MediaType | 'all') => setQuery(current => ({
          ...current,
          mediaTypes: mediaType === 'all' || (current.mediaTypes.length === 1 && current.mediaTypes[0] === mediaType) ? [] : [mediaType],
        }))}
        filterStatuses={query.statuses}
        onFilterStatusChange={(status: Status | 'all') => setQuery(current => ({
          ...current,
          statuses: status === 'all' || (current.statuses.length === 1 && current.statuses[0] === status) ? [] : [status],
        }))}
        activeRegions={query.regions}
        onRegionChange={(region: RegionFilter) => setQuery(current => ({ ...current, regions: region === 'all' ? [] : [region] }))}
        lockFilter={query.lock}
        onLockFilterChange={lock => setQuery(current => ({ ...current, lock }))}
      />

      <ActiveFilterSummary
        items={filterSummary}
        onRemove={clearQueryDimension}
        onClear={() => setQuery(current => clearAdvancedQuery(current))}
      />

      {/* Content Main Area */}
      <main className="custom-scrollbar min-h-0 max-w-5xl mx-auto w-full flex-1 overflow-y-auto px-4 pb-8 mt-4">
        {initializationState === 'loading' ? (
          <div className="space-y-8 animate-pulse">
            {[1, 2].map(group => (
              <div key={group}>
                <div className="h-6 w-24 bg-gray-200 rounded-full mb-4" />
                <div className="grid grid-cols-1 sm:grid-cols-2 lg:grid-cols-3 gap-3">
                  {[1, 2, 3].map(i => (
                    <div key={i} className="h-32 bg-gray-100 rounded-2xl" />
                  ))}
                </div>
              </div>
            ))}
          </div>
        ) : initializationState === 'error' ? (
          <div className="mx-auto mt-16 max-w-lg rounded-3xl border border-red-100 bg-white p-8 text-center shadow-sm" role="alert">
            <span className="mb-4 block text-5xl" aria-hidden="true">⚠️</span>
            <h1 className="text-xl font-black text-gray-900">
              {databaseCompatibilityIssue ? '数据库版本不兼容' : '无法读取本地数据'}
            </h1>
            <p className="mt-2 text-sm leading-6 text-gray-500">
              {databaseCompatibilityIssue?.code === 'unsupported_newer_database'
                ? `检测到 V${databaseCompatibilityIssue.detectedVersion} 数据库；当前程序使用 V${databaseCompatibilityIssue.supportedVersion}，只支持自动转换已知的 V19。该数据库未被修改。`
                : databaseCompatibilityIssue?.code === 'v19_downgrade_failed'
                  ? 'V19 数据库自动转换失败，原迁移事务已回滚。请保留 data 和 backups 目录并检查日志。'
                  : '当前列表没有被当作空数据处理。请确认数据目录可用后重试。'}
            </p>
            <button
              type="button"
              onClick={retryInitialization}
              className="mt-6 rounded-xl bg-indigo-600 px-6 py-2.5 text-sm font-bold text-white hover:bg-indigo-700 focus:outline-none focus:ring-2 focus:ring-indigo-500 focus:ring-offset-2"
            >
              重试加载
            </button>
          </div>
        ) : filtered.length === 0 ? (
          <div className="flex flex-col items-center justify-center py-24 text-gray-400">
            <span className="text-5xl mb-4">🎭</span>
            <p className="text-base font-medium">
              {records.length ? '当前筛选没有匹配记录' : '还没有记录，快去添加吧！'}
            </p>
            {records.length ? (
              <div className="mt-4 flex gap-3">
                <button onClick={() => setShowAdvancedFilters(true)} className="rounded-xl border border-gray-200 px-5 py-2 text-sm font-semibold text-gray-600">修改高级筛选</button>
                <button onClick={resetQueryConditions} className="rounded-xl bg-indigo-600 px-5 py-2 text-sm font-semibold text-white">重置全部条件</button>
              </div>
            ) : (
              <button
                onClick={() => setShowForm(true)}
                className="mt-4 px-6 py-2.5 bg-indigo-600 text-white rounded-xl text-sm font-semibold hover:bg-indigo-700 transition-colors"
              >
                添加第一条记录
              </button>
            )}
          </div>
        ) : viewMode === 'list' ? (
            <ListView
              filtered={filtered}
              onEdit={handleEdit}
              onDelete={handleDelete}
              onLockToggle={handleLockToggle}
              onStatusChange={handleStatusChange}
              onNextEpisodeChange={handleNextEpisodeChange}
              collectionNamesByRecord={collectionNamesByRecord}
            />
        ) : (
          <PosterWall
            filtered={filtered}
            onEdit={handleEdit}
            collectionNamesByRecord={collectionNamesByRecord}
          />
        )}
      </main>

      {/* Form Modal */}
      {showForm && (
        <RecordForm
          record={editingRecord}
          onSave={handleSave}
          onDelete={handleDelete}
          onNotify={notify}
          collections={collectionState.collections}
          collectionMembers={collectionState.members}
          onClose={handleCloseForm}
        />
      )}

      {/* Settings Modal */}
      {showSettings && (
        <SettingsModal
          initialTab={settingsInitialTab}
          records={records}
          onClose={async () => {
            setShowSettings(false);
            const credsOk = await hasCreds();
            setHasWebDAVCreds(credsOk);
            if (credsOk) await notifySyncConfigurationChanged();
          }}
          onImport={handleImport}
          onSync={syncNow}
          onUpdateRecord={async (id, updates) => { await updateRecord(id, updates); }}
          onDatabaseRestored={async () => { const loaded = await reloadAndSchedule(); await collectionState.refresh(); return loaded; }}
          syncInterval={syncInterval}
          onSyncIntervalChange={setSyncInterval}
          pullIntervalMinutes={pullIntervalMinutes}
          onPullIntervalChange={setPullIntervalMinutes}
          syncRuntime={syncRuntime}
          onNotify={notify}
        />
      )}

      {/* Dashboard Modal */}
      {showDashboard && (
        <Suspense fallback={<div className="fixed inset-0 z-50 grid place-items-center bg-[#0b1020] text-sm font-semibold text-slate-300">正在加载看板...</div>}>
          <Dashboard
            records={records}
            onClose={() => setShowDashboard(false)}
          />
        </Suspense>
      )}

      {showCollections && (
        <CollectionCenter
          records={records}
          collections={collectionState.collections}
          members={collectionState.members}
          initialSelectedId={collectionResumeId}
          onCreate={collectionState.create}
          onUpdate={collectionState.update}
          onSetOrderMode={collectionState.setOrderMode}
          onBindSource={collectionState.bindSource}
          onDelete={collectionState.remove}
          onAddMembers={collectionState.addMembers}
          onRemoveMember={collectionState.removeMember}
          onReorder={collectionState.reorder}
          onApplySuggestion={collectionState.applySuggestion}
          onCreateMissingSeasons={async (collection, newRecords) => {
            await createRecoveryPoint('series-completion');
            await createMissingSeasons(collection.id, newRecords, collection.rev);
            await Promise.all([loadRecords(), collectionState.refresh()]);
          }}
          onCompleteMovieCollection={async input => {
            await createRecoveryPoint('series-completion');
            const result = await completeMovieCollection(input);
            await Promise.all([loadRecords(), collectionState.refresh()]);
            return result;
          }}
          onEditRecord={handleCollectionRecordEdit}
          onNotify={notify}
          onClose={() => { setShowCollections(false); setCollectionResumeId(null); }}
        />
      )}

      {showAdvancedFilters && (
        <AdvancedFilterPanel
          query={query}
          options={advancedOptions}
          onChange={next => setQuery(normalizeWatchlistQuery(next))}
          onClear={() => setQuery(current => clearAdvancedQuery(current))}
          onClose={() => setShowAdvancedFilters(false)}
        />
      )}
    </div>
  );
}
