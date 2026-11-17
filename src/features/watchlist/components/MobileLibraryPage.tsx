import ViewingDateField from '../record-form/ViewingDateField';
import { validViewingDateDraft, viewingDatePrecision, viewingDateText } from '../../../shared/lib/viewingDate';
import { useCallback, useEffect, useMemo, useRef, useState, type FormEvent, type RefObject } from 'react';
import type { MobileRoute } from '../../../platform/navigation';
import type { MediaType, Status, WatchRecord } from '../../../shared/types';
import { MEDIA_TYPE_VALUES, STATUS_VALUES } from '../../../shared/types';
import { displayTitlesOf } from '../../../shared/lib/displayTitle';
import SafePosterImage from './SafePosterImage';
import { getEmptyRecord } from '../../../shared/lib/constants';
import { initialRecordFormValues, mediaTypeChange, smartProgress, type RecordFormValues } from '../record-form/recordFormModel';
import { useTmdbRecordSearch, extractImdbId } from '../record-form/useTmdbRecordSearch';
import {
  applyMobileLibraryQuery, DEFAULT_MOBILE_LIBRARY_PREFERENCES, isRecordFormDirty, MOBILE_LIBRARY_PREFERENCES_KEY,
  normalizeMobilePreferences, type MobileLibraryPreferences, type MobileSortBy,
} from '../mobileLibraryModel';
import { getSettingAsync, setSettingAsync } from '../../../shared/lib/database';
import type { NoticeTone } from '../../../shared/lib/feedback';
import MobileEpisodeTracking, { MobileEpisodeCardStatus, MobileEpisodeQuickAction, type MobileEpisodeActionHandler } from './MobileEpisodeTracking.tsx';
import MobileRecordActionsSheet from './MobileRecordActionsSheet.tsx';
import MobilePosterWall from './MobilePosterWall.tsx';
import { isMobileEpisodeTrackable } from '../mobileEpisodeTracking.ts';

interface MobileLibraryPageProps {
  records: WatchRecord[];
  route: MobileRoute;
  detailId: string | null;
  formMode: 'new' | 'edit';
  onDetail: (record: WatchRecord) => void;
  onForm: (mode: 'new' | 'edit', record?: WatchRecord) => void;
  onBack: () => void;
  onAdd: (value: RecordFormValues) => Promise<boolean>;
  onUpdate: (record: WatchRecord, value: RecordFormValues) => Promise<boolean>;
  onDelete: (record: WatchRecord) => Promise<boolean>;
  onLock: (record: WatchRecord, locked: boolean) => Promise<boolean>;
  onStatus: (record: WatchRecord, status: Status) => Promise<boolean>;
  onEpisode: MobileEpisodeActionHandler;
  onNotify: (tone: NoticeTone, message: string) => void;
}

function titleOf(record: WatchRecord): string {
  const titles = displayTitlesOf(record);
  return titles.primary || titles.secondary || '未命名记录';
}

function EmptyState({ kind, onAdd }: { kind: 'library' | 'search' | 'filter'; onAdd: () => void }) {
  const copy = kind === 'library' ? ['片库还是空的', '添加第一条记录后，它会保存在本机。'] : kind === 'search' ? ['没有找到匹配记录', '试试其他标题、平台或备注关键词。'] : ['当前筛选没有结果', '清除部分筛选条件后再试。'];
  return <section className="mobile-empty" data-empty-state={kind} aria-live="polite"><span aria-hidden="true">{kind === 'library' ? '🎭' : '⌕'}</span><h1>{copy[0]}</h1><p>{copy[1]}</p>{kind === 'library' && <button type="button" className="mobile-primary-button" onClick={onAdd}>添加第一条记录</button>}</section>;
}

function FilterSheet({ prefs, setPrefs, records, onClose, returnFocusRef }: { prefs: MobileLibraryPreferences; setPrefs: (next: MobileLibraryPreferences) => void; records: WatchRecord[]; onClose: () => void; returnFocusRef: RefObject<HTMLButtonElement | null> }) {
  const [draft, setDraft] = useState(prefs);
  const ref = useRef<HTMLDivElement>(null);
  useEffect(() => { const timer = window.requestAnimationFrame(() => { const first = ref.current?.querySelector<HTMLElement>('button, [href], input, select, textarea, [tabindex]:not([tabindex="-1"])'); (first ?? ref.current)?.focus(); }); return () => window.cancelAnimationFrame(timer); }, []);
  useEffect(() => () => { returnFocusRef.current?.focus(); }, [returnFocusRef]);
  useEffect(() => { const listener = (event: KeyboardEvent) => { if (event.key !== 'Tab' || !ref.current) return; const focusable = Array.from(ref.current.querySelectorAll<HTMLElement>('button, [href], input, select, textarea, [tabindex]:not([tabindex="-1"])')).filter(item => !item.hasAttribute('disabled')); if (!focusable.length) return; const first = focusable[0]; const last = focusable[focusable.length - 1]; if (event.shiftKey && document.activeElement === first) { event.preventDefault(); last.focus(); } else if (!event.shiftKey && document.activeElement === last) { event.preventDefault(); first.focus(); } }; window.addEventListener('keydown', listener); return () => window.removeEventListener('keydown', listener); }, []);
  useEffect(() => { const listener = (event: KeyboardEvent) => { if (event.key === 'Escape') onClose(); }; window.addEventListener('keydown', listener); return () => window.removeEventListener('keydown', listener); }, [onClose]);
  useEffect(() => { window.__WATCHTRACKER_ANDROID_BACK_OVERLAY__ = () => { onClose(); return true; }; return () => { delete window.__WATCHTRACKER_ANDROID_BACK_OVERLAY__; }; }, [onClose]);
  const toggle = <T extends string>(key: 'mediaTypes' | 'statuses', value: T) => setDraft(prev => ({ ...prev, [key]: prev[key].includes(value as never) ? prev[key].filter(item => item !== value) : [...prev[key], value] }));
  return <div className="mobile-sheet-backdrop" role="presentation" onMouseDown={event => { if (event.target === event.currentTarget) onClose(); }}>
    <div ref={ref} tabIndex={-1} role="dialog" aria-modal="true" aria-labelledby="mobile-filter-title" className="mobile-sheet">
      <div className="mobile-sheet-header"><h2 id="mobile-filter-title">筛选片库</h2><button type="button" aria-label="关闭筛选" className="mobile-icon-button" onClick={onClose}>×</button></div>
      <fieldset><legend>状态</legend><div className="mobile-chip-row">{STATUS_VALUES.map(value => <button key={value} type="button" className={`mobile-chip ${draft.statuses.includes(value) ? 'selected' : ''}`} aria-pressed={draft.statuses.includes(value)} onClick={() => toggle('statuses', value)}>{value} <small>{records.filter(record => record.status === value).length}</small></button>)}</div></fieldset>
      <fieldset><legend>类型</legend><div className="mobile-chip-row">{MEDIA_TYPE_VALUES.map(value => <button key={value} type="button" className={`mobile-chip ${draft.mediaTypes.includes(value) ? 'selected' : ''}`} aria-pressed={draft.mediaTypes.includes(value)} onClick={() => toggle('mediaTypes', value)}>{value}</button>)}</div></fieldset>
      <fieldset><legend>锁定</legend><div className="mobile-chip-row">{(['all', 'locked', 'unlocked'] as const).map(value => <button key={value} type="button" className={`mobile-chip ${draft.lock === value ? 'selected' : ''}`} aria-pressed={draft.lock === value} onClick={() => setDraft(prev => ({ ...prev, lock: value }))}>{value === 'all' ? '全部' : value === 'locked' ? '已锁定' : '未锁定'}</button>)}</div></fieldset>
      <div className="mobile-sheet-actions"><button type="button" className="mobile-quiet-button" onClick={() => setDraft(prev => ({ ...prev, mediaTypes: [], statuses: [], lock: 'all' }))}>清除筛选</button><button type="button" className="mobile-primary-button" onClick={() => { setPrefs({ ...draft }); onClose(); }}>应用筛选</button></div>
    </div>
  </div>;
}

function LibraryToolbar({ search, setSearch, prefs, setPrefs, onFilter, filterTriggerRef, filteredCount, totalCount, viewMode, onToggleViewMode }: { search: string; setSearch: (value: string) => void; prefs: MobileLibraryPreferences; setPrefs: (next: MobileLibraryPreferences) => void; onFilter: () => void; filterTriggerRef: RefObject<HTMLButtonElement | null>; filteredCount: number; totalCount: number; viewMode: 'list' | 'poster'; onToggleViewMode: (mode: 'list' | 'poster') => void }) {
  const chips = [
    ...prefs.statuses.map(value => ({ key: `status-${value}`, label: value, ariaLabel: `移除状态：${value}`, clear: () => setPrefs({ ...prefs, statuses: prefs.statuses.filter(item => item !== value) }) })),
    ...prefs.mediaTypes.map(value => ({ key: `media-${value}`, label: value, ariaLabel: `移除类型：${value}`, clear: () => setPrefs({ ...prefs, mediaTypes: prefs.mediaTypes.filter(item => item !== value) }) })),
    ...(prefs.lock === 'all' ? [] : [{ key: 'lock', label: prefs.lock === 'locked' ? '已锁定' : '未锁定', ariaLabel: '移除锁定筛选', clear: () => setPrefs({ ...prefs, lock: 'all' as const }) }]),
  ];
  const countLabel = filteredCount === totalCount ? String(totalCount) : `${filteredCount} / ${totalCount}`;
  return <>
    <div className="mobile-section-heading"><h1 id="mobile-library-title">我的片库</h1><span className="mobile-count" aria-label={`片库数量 ${countLabel}`}>{countLabel}</span></div>
    <div className="mobile-search-row"><label className="mobile-search-label"><span className="mobile-search-icon" aria-hidden="true"><svg viewBox="0 0 24 24" focusable="false"><circle cx="11" cy="11" r="6" /><path d="m16 16 4 4" /></svg></span><span className="sr-only">搜索片库</span><input aria-label="搜索片库" value={search} onChange={event => setSearch(event.target.value)} placeholder="搜索标题、平台、备注" />{search && <button type="button" className="mobile-search-clear" aria-label="清空搜索" onClick={() => setSearch('')}>×</button>}</label></div>
    <div className="mobile-toolbar-row">
      {chips.length ? <div className="mobile-active-chips" aria-label="当前筛选条件"><span className="sr-only">已启用筛选</span>{chips.map(chip => <button key={chip.key} type="button" aria-label={chip.ariaLabel} onClick={chip.clear}>{chip.label} ×</button>)}</div> : <span className="mobile-toolbar-spacer" aria-hidden="true" />}
      <div className="mobile-view-toggle" role="group" aria-label="视图切换">
        <button
          type="button"
          className={`mobile-view-toggle-btn ${viewMode === 'list' ? 'selected' : ''}`}
          aria-pressed={viewMode === 'list'}
          aria-label="切换到列表视图"
          title="列表视图"
          onClick={() => onToggleViewMode('list')}
        >
          ☰
        </button>
        <button
          type="button"
          className={`mobile-view-toggle-btn ${viewMode === 'poster' ? 'selected' : ''}`}
          aria-pressed={viewMode === 'poster'}
          aria-label="切换到海报墙视图"
          title="海报墙视图"
          onClick={() => onToggleViewMode('poster')}
        >
          ▦
        </button>
      </div>
      <label className="mobile-sort-control"><span className="sr-only">排序</span><select aria-label="排序" value={prefs.sortBy} onChange={event => setPrefs({ ...prefs, sortBy: event.target.value as MobileSortBy })}><option value="createdAt">最新添加</option><option value="startDate">开播/开始</option><option value="endDate">完成时间</option><option value="releaseYear">上映年份</option><option value="rating">最高评分</option><option value="watchValue">追剧推荐</option></select></label>
      <button ref={filterTriggerRef} type="button" className="mobile-filter-button" aria-label="打开筛选" onClick={onFilter}>筛选</button>
    </div>
  </>;
}

function RecordCard({ record, onOpen, onStatus, onMore, onEpisode }: { record: WatchRecord; onOpen: () => void; onStatus: (status: Status) => void; onMore: (trigger: HTMLButtonElement) => void; onEpisode: MobileEpisodeActionHandler }) {
  const title = titleOf(record);
  const episodic = isMobileEpisodeTrackable(record);
  return <article className="mobile-record-card" data-record-id={record.id}>
    <div className="mobile-record-card-main">
      <button type="button" className="mobile-record-open" onClick={onOpen} aria-label={`打开 ${title}`}>
        {record.posterPath ? <SafePosterImage posterPath={record.posterPath} alt={`${title} 海报`} className="mobile-record-thumbnail" compact autoDownload={false} /> : <div className="mobile-record-thumbnail mobile-poster-fallback" aria-label={`${title} 无海报`}>无图</div>}
        <div className="mobile-record-card-body"><div className="mobile-record-title">{title}</div>{displayTitlesOf(record).secondary && <div className="mobile-record-subtitle">{displayTitlesOf(record).secondary}</div>}<div className="mobile-record-meta"><span>{record.mediaType}</span><span>{record.status}</span>{record.releaseYear && <span>{record.releaseYear}</span>}{record.rating != null && <span>★ {record.rating}</span>}{record.isLocked && <span>🔒 已锁定</span>}</div><MobileEpisodeCardStatus record={record} /></div>
      </button>
      <button type="button" className="mobile-record-more" aria-label={`更多操作：${title}`} title="更多操作" onClick={event => onMore(event.currentTarget)}><span aria-hidden="true">⋮</span></button>
    </div>
    <div className="mobile-record-quick-action">{episodic ? <MobileEpisodeQuickAction record={record} onAction={onEpisode} onOpen={onOpen} /> : <button type="button" className="mobile-primary-button mobile-status-quick-button" disabled={record.isLocked} onClick={() => onStatus(record.status === '已看' ? '未看' : '已看')}>{record.status === '已看' ? '标为未看' : '标为已看'}</button>}</div>
  </article>;
}

function DetailPage({ record, onBack, onEdit, onDelete, onLock, onStatus, onEpisode }: { record: WatchRecord; onBack: () => void; onEdit: () => void; onDelete: () => void; onLock: () => void; onStatus: (status: Status) => void; onEpisode: MobileEpisodeActionHandler }) {
  const title = titleOf(record);
  const episodic = isMobileEpisodeTrackable(record);
  const fields: Array<[string, string | number | null | undefined]> = [['中文名', record.chineseName], ['原名', record.originalName], ['类型', record.mediaType], ['状态', record.status], ['进度', record.progress], ['总集数', record.totalEpisodes], ['平台', record.platform], ['上映年份', record.releaseYear], ['评分', record.rating], ['开始日期', viewingDateText(record.startDate, 'start')], ['完成日期', viewingDateText(record.endDate, 'end')], ['备注', record.notes], ['IMDb ID', record.imdbId], ['类型标签', record.genres], ['地区', record.originCountry], ['内容标签', record.contentTags]];
  return <section className="mobile-detail-page" aria-labelledby="mobile-detail-title"><div className="mobile-page-header"><button type="button" className="mobile-icon-button" aria-label="返回片库" onClick={onBack}>‹</button><h1 id="mobile-detail-title">{title}</h1><button type="button" className="mobile-icon-button" aria-label="编辑记录" onClick={onEdit} disabled={record.isLocked}>✎</button></div><div className="mobile-detail-hero">{record.posterPath && <SafePosterImage posterPath={record.posterPath} alt={`${title} 海报`} className="mobile-detail-poster" />}<div><h2>{title}</h2><p>{record.originalName || '暂无原名'}</p><span className="mobile-status-pill">{record.status}</span>{record.isLocked && <span className="mobile-status-pill locked">🔒 已锁定</span>}</div></div><div className="mobile-detail-actions">{!episodic && <button type="button" className="mobile-primary-button" disabled={record.isLocked} onClick={() => onStatus(record.status === '已看' ? '未看' : '已看')}>{record.status === '已看' ? '标为未看' : '标为已看'}</button>}<button type="button" className="mobile-quiet-button" onClick={onLock}>{record.isLocked ? '解锁' : '锁定'}</button><button type="button" className="mobile-quiet-button mobile-danger" disabled={record.isLocked} onClick={onDelete}>删除</button></div>{episodic && <MobileEpisodeTracking key={record.id} record={record} onAction={onEpisode} />}<dl className="mobile-detail-grid">{fields.map(([label, value]) => value !== null && value !== undefined && value !== '' ? <div key={label}><dt>{label}</dt><dd>{value}</dd></div> : null)}</dl>{record.isLocked && <p className="mobile-locked-hint">此记录已锁定。可以查看和解锁；编辑、删除和逐集写入会被拒绝。</p>}{record.notes && <p className="mobile-detail-notes">{record.notes}</p>}</section>;
}

function MobileRecordForm({ record, onSave, onDelete, onClose, onNotify }: { record: WatchRecord | null; onSave: (value: RecordFormValues) => Promise<boolean>; onDelete: () => void; onClose: () => void; onNotify: (tone: NoticeTone, message: string) => void }) {
  // Keep the draft stable while repository reloads replace the record object.
  // eslint-disable-next-line react-hooks/exhaustive-deps
  const initial = useMemo(() => record ? initialRecordFormValues(record) : getEmptyRecord(), [record?.id]);
  const [form, setForm] = useState<RecordFormValues>(initial);
  const [startYearOnly, setStartYearOnly] = useState(viewingDatePrecision(initial.startDate) === 'year');
  const [endYearOnly, setEndYearOnly] = useState(viewingDatePrecision(initial.endDate) === 'year');
  const [saving, setSaving] = useState(false);
  const [searchQuery, setSearchQuery] = useState('');
  const allowCloseRef = useRef(false);
  const dirty = isRecordFormDirty(initial, form);
  const isEpisodic = form.mediaType === '剧集' || form.mediaType === '动画' || form.mediaType === '纪录片' || form.mediaType === '综艺';

  const {
    isSearching, searchResults, searchError, showResults, setShowResults,
    seasons, setSeasons, selectedSeries, setSelectedSeries,
    handleTMDBSearch, handleSelectResult, handleSelectSeason,
  } = useTmdbRecordSearch({ form, setForm, isEpisodic, onNotify });

  useEffect(() => {
    // The route remounts the form for each record; this also handles hot navigation.
    // eslint-disable-next-line react-hooks/set-state-in-effect
    setForm(initial);
  }, [initial]);

  const leave = useCallback(() => {
    if (allowCloseRef.current) { window.history.back(); return; }
    if (!dirty || window.confirm('有未保存的修改，确定离开吗？')) onClose();
  }, [dirty, onClose]);

  useEffect(() => {
    const listener = (event: KeyboardEvent) => { if (event.key === 'Escape') { event.preventDefault(); leave(); } };
    window.addEventListener('keydown', listener);
    return () => window.removeEventListener('keydown', listener);
  }, [leave]);

  useEffect(() => {
    if (!dirty) { delete window.__WATCHTRACKER_ANDROID_BACK_OVERLAY__; return; }
    window.__WATCHTRACKER_ANDROID_BACK_OVERLAY__ = () => {
      if (allowCloseRef.current) { window.history.back(); return true; }
      if (window.confirm('有未保存的修改，确定离开吗？')) window.history.back();
      return true;
    };
    return () => { delete window.__WATCHTRACKER_ANDROID_BACK_OVERLAY__; };
  }, [dirty]);

  const set = <K extends keyof RecordFormValues>(key: K, value: RecordFormValues[K]) => setForm(prev => ({ ...prev, [key]: value }));

  const triggerSearch = async (override?: string) => {
    const raw = (override || searchQuery || form.imdbId || form.chineseName || form.originalName || '').trim();
    if (!raw) {
      onNotify('warning', '请输入片名或 IMDb ID (如 tt0111161)');
      return;
    }
    const imdb = extractImdbId(raw);
    if (imdb && !form.imdbId) {
      set('imdbId', imdb);
    }
    await handleTMDBSearch(raw);
  };

  const submit = async (event: FormEvent) => {
    event.preventDefault();
    if (!form.chineseName.trim() && !form.originalName.trim()) {
      onNotify('warning', '至少填写中文名或原名。');
      return;
    }
    if (!validViewingDateDraft(form.startDate) || !validViewingDateDraft(form.endDate)) {
      onNotify('warning', '请输入合法的四位年份或完整公历日期。');
      return;
    }
    setSaving(true);
    const ok = await onSave({
      ...form,
      chineseName: form.chineseName.trim(),
      originalName: form.originalName.trim(),
      progress: smartProgress(form.progress),
    });
    setSaving(false);
    if (ok) {
      allowCloseRef.current = true;
      window.history.back();
    }
  };

  return (
    <div className="mobile-form-page" role="dialog" aria-modal="true" aria-label={record ? '编辑记录' : '添加新记录'} aria-labelledby="mobile-form-title">
      <form onSubmit={submit}>
        <header className="mobile-form-header">
          <button type="button" className="mobile-quiet-button" onClick={leave}>取消</button>
          <h1 id="mobile-form-title">{record ? '编辑记录' : '添加新记录'}</h1>
          <button type="submit" className="mobile-primary-button" disabled={saving}>
            {saving ? '保存中…' : record ? '保存修改' : '添加记录'}
          </button>
        </header>

        <div className="mobile-form-body">
          {/* TMDB Quick Search / Autofill */}
          <div className="mobile-search-card">
            <label>
              <span>🔍 快速检索 / 自动填充</span>
              <div className="mobile-search-input-group">
                <input
                  type="text"
                  placeholder="输入片名、IMDb ID (tt...) 或链接"
                  value={searchQuery}
                  onChange={e => {
                    const val = e.target.value;
                    setSearchQuery(val);
                    const imdb = extractImdbId(val);
                    if (imdb && !form.imdbId) set('imdbId', imdb);
                  }}
                  onKeyDown={e => { if (e.key === 'Enter') { e.preventDefault(); void triggerSearch(searchQuery); } }}
                />
                <button
                  type="button"
                  className="mobile-search-action-btn"
                  disabled={isSearching || (!searchQuery.trim() && !form.imdbId && !form.chineseName && !form.originalName)}
                  onClick={() => void triggerSearch(searchQuery)}
                >
                  {isSearching ? '搜索中…' : '检索'}
                </button>
              </div>
            </label>
            {showResults && (
              <div className="mobile-search-results-panel">
                <div className="mobile-search-results-header">
                  <div style={{ display: 'flex', alignItems: 'center', gap: '8px' }}>
                    {seasons.length > 0 && (
                      <button
                        type="button"
                        className="mobile-quiet-button"
                        style={{ minWidth: 'auto', minHeight: 'auto', padding: '2px 6px' }}
                        onClick={() => { setSeasons([]); setSelectedSeries(null); }}
                      >
                        ‹ 返回
                      </button>
                    )}
                    <span>{seasons.length > 0 ? `选择分季 (${selectedSeries?.name || selectedSeries?.title})` : 'TMDB 搜索结果'}</span>
                  </div>
                  <button
                    type="button"
                    className="mobile-quiet-button"
                    style={{ minWidth: 'auto', minHeight: 'auto', padding: '2px 6px' }}
                    onClick={() => setShowResults(false)}
                  >
                    ✕
                  </button>
                </div>
                {seasons.length > 0 ? (
                  seasons.map(season => (
                    <button
                      key={season.id}
                      type="button"
                      className="mobile-search-result-item"
                      onClick={() => void handleSelectSeason(season)}
                    >
                      <div style={{ display: 'flex', flexDirection: 'column' }}>
                        <strong>{season.name || `第 ${season.season_number} 季`}</strong>
                        <small>{season.air_date?.split('-')[0] || '未知年份'} · {season.episode_count} 集</small>
                      </div>
                      <span style={{ marginLeft: 'auto', color: '#94a3b8' }}>›</span>
                    </button>
                  ))
                ) : searchError ? (
                  <div className="mobile-search-error-box">
                    <p>❌ {searchError}</p>
                    {searchError.includes('TMDB API KEY') && (
                      <p className="mobile-sync-note" style={{ marginTop: '4px' }}>可在底部导航【设置】中配置 TMDB API KEY。</p>
                    )}
                  </div>
                ) : searchResults.length === 0 && !isSearching ? (
                  <div className="mobile-search-empty-box">未找到匹配的影视内容</div>
                ) : (
                  searchResults.map(item => (
                    <button
                      key={item.id}
                      type="button"
                      className="mobile-search-result-item"
                      onClick={() => void handleSelectResult(item, () => undefined)}
                    >
                      {item.poster_path ? (
                        <SafePosterImage posterPath={item.poster_path} size="w92" alt="" className="mobile-search-result-poster" />
                      ) : (
                        <div className="mobile-search-result-poster-placeholder">无图</div>
                      )}
                      <div className="mobile-search-result-info">
                        <div className="mobile-search-result-title">
                          {item.media_type === 'movie' && <span className="mobile-badge movie">电影</span>}
                          {item.media_type === 'tv' && <span className="mobile-badge tv">剧集</span>}
                          {item.title || item.name}
                        </div>
                        <div className="mobile-search-result-subtitle">{item.original_title || item.original_name}</div>
                        <div className="mobile-search-result-meta">
                          {item.release_date || item.first_air_date || '未知年份'}
                          {(item.vote_average ?? 0) > 0 && ` · ⭐ ${item.vote_average?.toFixed(1)}`}
                        </div>
                      </div>
                    </button>
                  ))
                )}
              </div>
            )}
          </div>

          <label>
            中文名
            <input
              autoFocus
              placeholder="请输入中文名称"
              value={form.chineseName}
              onChange={event => {
                const val = event.target.value;
                const imdb = extractImdbId(val);
                if (imdb && !form.imdbId) set('imdbId', imdb);
                set('chineseName', val);
              }}
            />
          </label>

          <label>
            原名
            <input
              placeholder="请输入原名"
              value={form.originalName}
              onChange={event => set('originalName', event.target.value)}
            />
          </label>

          <div className="mobile-form-grid">
            <label>
              类型
              <select value={form.mediaType} onChange={event => setForm(prev => mediaTypeChange(prev, event.target.value as MediaType).form)}>
                {MEDIA_TYPE_VALUES.map(value => <option key={value}>{value}</option>)}
              </select>
            </label>
            <label>
              状态
              <select value={form.status} onChange={event => set('status', event.target.value as Status)}>
                {STATUS_VALUES.map(value => <option key={value}>{value}</option>)}
              </select>
            </label>
          </div>

          <div className="mobile-form-grid">
            <label>
              平台
              <input value={form.platform} onChange={event => set('platform', event.target.value)} />
            </label>
            <label>
              上映年份
              <input inputMode="numeric" value={form.releaseYear || ''} onChange={event => set('releaseYear', event.target.value || null)} />
            </label>
          </div>

          <div className="mobile-form-grid">
            <label>
              进度
              <input value={form.progress} onChange={event => set('progress', event.target.value)} />
            </label>
            <label>
              总集数
              <input inputMode="numeric" value={form.totalEpisodes ?? ''} onChange={event => set('totalEpisodes', event.target.value ? Number(event.target.value) : null)} />
            </label>
          </div>

          <div className="mobile-form-grid">
            <label>
              评分
              <input type="number" min="0" max="10" step="0.1" value={form.rating ?? ''} onChange={event => set('rating', event.target.value ? Number(event.target.value) : null)} />
            </label>
            <label>
              <span>IMDb ID</span>
              <div className="mobile-search-input-group">
                <input
                  value={form.imdbId || ''}
                  placeholder="如 tt0111161 或链接"
                  onChange={event => {
                    const val = event.target.value.trim();
                    const imdb = extractImdbId(val);
                    set('imdbId', imdb || (val || null));
                  }}
                />
                <button
                  type="button"
                  className="mobile-search-action-btn"
                  disabled={isSearching || !form.imdbId}
                  onClick={() => void triggerSearch(form.imdbId || undefined)}
                >
                  {isSearching ? '…' : '检索'}
                </button>
              </div>
            </label>
          </div>

          <div className="mobile-form-grid">
            <ViewingDateField label="开始时间" value={form.startDate || ''} yearOnly={startYearOnly} onYearOnlyChange={setStartYearOnly} onChange={value => set('startDate', value)} />
            <ViewingDateField label="结束时间" value={form.endDate || ''} yearOnly={endYearOnly} onYearOnlyChange={setEndYearOnly} onChange={value => set('endDate', value)} />
          </div>

          <fieldset className="mobile-interest-field">
            <legend>兴趣程度</legend>
            <div className="mobile-chip-row">
              {[1, 3, 5].map(value => (
                <button
                  key={value}
                  type="button"
                  className={`mobile-chip ${form.interestLevel === value ? 'selected' : ''}`}
                  aria-pressed={form.interestLevel === value}
                  onClick={() => set('interestLevel', value)}
                >
                  期待值 {value} 分
                </button>
              ))}
            </div>
          </fieldset>

          <label>
            海报路径
            <div style={{ display: 'flex', gap: '8px', alignItems: 'center' }}>
              {form.posterPath && (
                <SafePosterImage posterPath={form.posterPath} size="w92" alt="海报预览" className="mobile-search-result-poster" />
              )}
              <input value={form.posterPath || ''} onChange={event => set('posterPath', event.target.value || null)} placeholder="如 /xxx.jpg" style={{ flex: 1 }} />
            </div>
          </label>

          <label>
            备注
            <textarea rows={4} value={form.notes} onChange={event => set('notes', event.target.value)} />
          </label>

          {record && (
            <button
              type="button"
              className="mobile-danger-block"
              onClick={() => { if (window.confirm('确定删除这条记录吗？')) onDelete(); }}
            >
              删除记录
            </button>
          )}
        </div>
      </form>
    </div>
  );
}

export default function MobileLibraryPage(props: MobileLibraryPageProps) {
  const { records, detailId, formMode, onDetail, onForm, onBack, onAdd, onUpdate, onDelete, onLock, onStatus, onEpisode, onNotify } = props;
  const [searchInput, setSearchInput] = useState('');
  const [search, setSearch] = useState('');
  const [prefs, setPrefsState] = useState(DEFAULT_MOBILE_LIBRARY_PREFERENCES);
  const [viewMode, setViewMode] = useState<'list' | 'poster'>('list');
  const [filterOpen, setFilterOpen] = useState(false);
  const [actionsRecord, setActionsRecord] = useState<WatchRecord | null>(null);
  const actionsTriggerRef = useRef<HTMLButtonElement>(null);
  const contentRef = useRef<HTMLElement>(null);
  const scrollTopRef = useRef(0);
  const filterTriggerRef = useRef<HTMLButtonElement>(null);
  useEffect(() => { const timer = window.setTimeout(() => setSearch(searchInput), 250); return () => window.clearTimeout(timer); }, [searchInput]);
  useEffect(() => {
    let active = true;
    void getSettingAsync('mobile_library_view_mode').then(val => {
      if (active && (val === 'poster' || val === 'list')) {
        setViewMode(val);
      }
    }).catch(() => undefined);
    return () => { active = false; };
  }, []);
  const handleToggleViewMode = useCallback((mode: 'list' | 'poster') => {
    setViewMode(mode);
    void setSettingAsync('mobile_library_view_mode', mode).catch(() => undefined);
  }, []);
  useEffect(() => { let active = true; void getSettingAsync(MOBILE_LIBRARY_PREFERENCES_KEY).then(raw => { if (!active || !raw) return; try { const parsed = JSON.parse(raw); const normalized = normalizeMobilePreferences(parsed); if (parsed?.version !== 1) onNotify('warning', '片库偏好版本不兼容，已使用默认设置。'); setPrefsState(normalized); } catch { onNotify('warning', '片库偏好读取失败，已使用默认设置。'); } }).catch(() => { if (active) onNotify('warning', '片库偏好读取失败，已使用默认设置。'); }); return () => { active = false; }; }, [onNotify]);
  const setPrefs = useCallback((next: MobileLibraryPreferences) => { const normalized = normalizeMobilePreferences(next); setPrefsState(normalized); void setSettingAsync(MOBILE_LIBRARY_PREFERENCES_KEY, JSON.stringify(normalized)).catch(() => onNotify('warning', '片库偏好保存失败，本次操作仍已生效。')); }, [onNotify]);
  useEffect(() => { if (props.route !== 'library') return; const timer = window.requestAnimationFrame(() => { const container = document.querySelector<HTMLElement>('.mobile-content'); if (container) container.scrollTop = scrollTopRef.current; }); return () => window.cancelAnimationFrame(timer); }, [props.route]);
  const captureScroll = useCallback(() => { scrollTopRef.current = document.querySelector<HTMLElement>('.mobile-content')?.scrollTop ?? 0; }, []);
  const filtered = useMemo(() => applyMobileLibraryQuery(records, search, prefs), [records, search, prefs]);
  const detail = detailId ? records.find(record => record.id === detailId) ?? null : null;
  const formRecord = formMode === 'edit' && detailId ? records.find(record => record.id === detailId) ?? null : null;
  const emptyKind = records.length === 0 ? 'library' : filtered.length === 0 ? (search ? 'search' : 'filter') : null;
  const openActions = useCallback((record: WatchRecord, trigger: HTMLButtonElement) => {
    actionsTriggerRef.current = trigger;
    setActionsRecord(record);
  }, []);
  const closeActions = useCallback(() => setActionsRecord(null), []);
  const openEdit = useCallback((record: WatchRecord) => { closeActions(); captureScroll(); onForm('edit', record); }, [captureScroll, closeActions, onForm]);
  const deleteRecord = useCallback((record: WatchRecord) => { closeActions(); void onDelete(record); }, [closeActions, onDelete]);
  const toggleLock = useCallback((record: WatchRecord, locked: boolean) => { closeActions(); void onLock(record, locked); }, [closeActions, onLock]);
  useEffect(() => { if (detailId && !detail) onBack(); }, [detailId, detail, onBack]);
  if (props.route === 'form' && formMode === 'edit' && !formRecord) return <section className="mobile-error" role="alert"><h1>记录不存在</h1><p>这条记录可能已被删除。</p><button type="button" className="mobile-primary-button" onClick={onBack}>返回片库</button></section>;
  if (props.route === 'form') return <MobileRecordForm record={formRecord} onSave={value => formRecord ? onUpdate(formRecord, value) : onAdd(value)} onDelete={() => { if (formRecord) void onDelete(formRecord); }} onClose={onBack} onNotify={onNotify} />;
  if (props.route === 'detail' && detailId && detail) return <DetailPage record={detail} onBack={onBack} onEdit={() => { if (!detail.isLocked) onForm('edit', detail); else onNotify('warning', '已锁定记录不能编辑。'); }} onDelete={() => void onDelete(detail)} onLock={() => void onLock(detail, !detail.isLocked)} onStatus={status => void onStatus(detail, status)} onEpisode={onEpisode} />;
  return <section className="mobile-library" aria-labelledby="mobile-library-title" ref={contentRef}><LibraryToolbar search={searchInput} setSearch={value => { setSearchInput(value); if (!value) setSearch(''); }} prefs={prefs} setPrefs={setPrefs} onFilter={() => setFilterOpen(true)} filterTriggerRef={filterTriggerRef} filteredCount={filtered.length} totalCount={records.length} viewMode={viewMode} onToggleViewMode={handleToggleViewMode} />{emptyKind ? <EmptyState kind={emptyKind} onAdd={() => onForm('new')} /> : viewMode === 'poster' ? <MobilePosterWall records={filtered} onOpen={record => { captureScroll(); onDetail(record); }} onMore={(record, trigger) => openActions(record, trigger)} onStatus={(record, nextStatus) => void onStatus(record, nextStatus)} /> : <div className="mobile-record-list">{filtered.map(record => <RecordCard key={record.id} record={record} onOpen={() => { captureScroll(); onDetail(record); }} onMore={trigger => openActions(record, trigger)} onStatus={status => void onStatus(record, status)} onEpisode={onEpisode} />)}</div>}{filterOpen && <FilterSheet prefs={prefs} setPrefs={setPrefs} records={records} onClose={() => setFilterOpen(false)} returnFocusRef={filterTriggerRef} />}{actionsRecord && <MobileRecordActionsSheet record={actionsRecord} onEdit={() => openEdit(actionsRecord)} onLock={locked => toggleLock(actionsRecord, locked)} onDelete={() => deleteRecord(actionsRecord)} onClose={closeActions} returnFocusRef={actionsTriggerRef} />}</section>;
}
