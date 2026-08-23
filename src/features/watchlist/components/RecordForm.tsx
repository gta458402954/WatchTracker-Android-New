import React, { useState, useRef } from 'react';
import { WatchRecord, Status, MediaType, type CollectionDraft, type CollectionMember, type WatchCollection } from '../../../shared/types';
import { STATUSES } from '../../../shared/lib/constants';
import { type NoticeTone } from '../../../shared/lib/feedback';
import { useAccessibleDialog } from '../../../shared/lib/useAccessibleDialog';
import {
  formatMovieTime,
  initialRecordFormValues,
  isAlwaysEpisodic,
  mediaTypeChange,
  smartProgress,
} from '../record-form/recordFormModel';
import { useTmdbRecordSearch } from '../record-form/useTmdbRecordSearch';
import CollectionMembership from '../record-form/CollectionMembership';
import TmdbSearchSection from '../record-form/TmdbSearchSection';
import PlaybackFields from '../record-form/PlaybackFields';
import RecordDetailsFields from '../record-form/RecordDetailsFields';

interface RecordFormProps {
  record?: WatchRecord | null;
  onSave: (data: Omit<WatchRecord, 'id' | 'createdAt'>, collectionIds: string[], collectionDrafts: CollectionDraft[]) => Promise<boolean | void> | boolean | void;
  onDelete?: (id: string) => void;
  onClose: () => void;
  onNotify?: (tone: NoticeTone, message: string) => void;
  collections?: WatchCollection[];
  collectionMembers?: CollectionMember[];
}

export default function RecordForm({ record, onSave, onDelete, onClose, onNotify, collections = [], collectionMembers = [] }: RecordFormProps) {
  const [selectedCollectionIds, setSelectedCollectionIds] = useState<string[]>(() => record
    ? collectionMembers.filter(member => member.recordId === record.id).map(member => member.collectionId)
    : []);
  const [collectionDrafts, setCollectionDrafts] = useState<CollectionDraft[]>([]);
  const [form, setForm] = useState<Omit<WatchRecord, 'id' | 'createdAt'>>(() => initialRecordFormValues(record));

  // 电影时间输入状态（秒）
  const [movieProgressStr, setMovieProgressStr] = useState(() => record?.movieProgress != null ? formatMovieTime(record.movieProgress) : '');
  const [movieDurationStr, setMovieDurationStr] = useState(() => record?.movieDuration != null ? formatMovieTime(record.movieDuration) : '');

  const [startYearOnly, setStartYearOnly] = useState(
    !!record?.startDate && /^\d{4}$/.test(record.startDate)
  );
  const [endYearOnly, setEndYearOnly] = useState(
    !!record?.endDate && /^\d{4}$/.test(record.endDate)
  );

  const currentYear = new Date().getFullYear();
  const years = Array.from({ length: currentYear - 1950 + 1 }, (_, i) => currentYear - i);

  const isEpisodic = isAlwaysEpisodic(form.mediaType) || Boolean(form.totalEpisodes);
  const { isSearching, searchResults, searchError, showResults, setShowResults, seasons, setSeasons, selectedSeries, setSelectedSeries, handleTMDBSearch, handleSelectResult, handleSelectSeason } = useTmdbRecordSearch({ form, setForm, isEpisodic, onNotify });

  const progressRef = useRef<HTMLInputElement>(null);
  const initialFocusRef = useRef<HTMLInputElement>(null);
  const dialogRef = useAccessibleDialog<HTMLDivElement>({ onEscape: onClose, initialFocusRef });

  function set<K extends keyof typeof form>(key: K, value: typeof form[K]) {
    setForm(prev => ({ ...prev, [key]: value }));
  }

  function handleMediaTypeChange(mediaType: MediaType) {
    const episodic = isAlwaysEpisodic(mediaType) || (mediaType !== '电影' && isEpisodic);
    setForm(prev => mediaTypeChange(prev, mediaType).form);
    if (episodic) {
      setMovieProgressStr('');
      setMovieDurationStr('');
    }
  }

  function handleProgressChange(e: React.ChangeEvent<HTMLInputElement>) {
    set('progress', e.target.value);
  }

  function handleProgressBlur() {
    const resolved = smartProgress(form.progress);
    if (resolved !== form.progress) {
      set('progress', resolved);
    }
  }

  async function handleSubmit(e: React.FormEvent) {
    e.preventDefault();
    if (!form.chineseName.trim() && !form.originalName.trim()) return;
    const result = await onSave(form, selectedCollectionIds, collectionDrafts);
    if (result !== false) {
      onClose();
    }
  }

  return (
    <div className="fixed inset-0 z-50 flex items-center justify-center p-4">
      <div className="absolute inset-0 bg-black/40 backdrop-blur-sm" onClick={onClose} />
      <div
        ref={dialogRef}
        role="dialog"
        aria-modal="true"
        aria-labelledby="record-form-title"
        tabIndex={-1}
        className="relative bg-white rounded-3xl shadow-2xl w-full max-w-lg max-h-[90vh] overflow-y-auto"
      >
        {/* Header */}
        <div className="flex items-center justify-between px-6 pt-6 pb-4 border-b border-gray-100">
          <h2 id="record-form-title" className="text-lg font-bold text-gray-900">
            {record ? '编辑记录' : '添加新记录'}
          </h2>
          <button
            type="button"
            onClick={onClose}
            aria-label="关闭记录表单"
            className="p-2 rounded-xl hover:bg-gray-100 text-gray-400 hover:text-gray-600 transition-colors"
          >
            <svg className="w-5 h-5" fill="none" stroke="currentColor" viewBox="0 0 24 24">
              <path strokeLinecap="round" strokeLinejoin="round" strokeWidth={2} d="M6 18L18 6M6 6l12 12" />
            </svg>
          </button>
        </div>

        <form onSubmit={handleSubmit} className="px-6 py-5 flex flex-col gap-4">
          <div className="grid grid-cols-2 gap-3">
            <div>
              <label className="mb-1 block text-sm font-medium text-gray-700">内容类型</label>
              <select value={form.mediaType || '电影'} onChange={event => handleMediaTypeChange(event.target.value as MediaType)} className="w-full rounded-xl border border-gray-200 bg-white px-3 py-2 text-sm outline-none focus:border-indigo-400">
                {(['电影', '剧集', '纪录片', '综艺', '动画'] as MediaType[]).map(type => <option key={type} value={type}>{type}</option>)}
              </select>
            </div>
            <div>
              <label className="mb-1 block text-sm font-medium text-gray-700">内容标签</label>
              <input value={form.contentTags || ''} onChange={event => set('contentTags', event.target.value)} placeholder="如：韩国" className="w-full rounded-xl border border-gray-200 px-3 py-2 text-sm outline-none focus:border-indigo-400" />
            </div>
          </div>

          <TmdbSearchSection form={form} initialFocusRef={initialFocusRef} isSearching={isSearching} searchResults={searchResults} searchError={searchError} showResults={showResults} seasons={seasons} selectedSeries={selectedSeries} onSearch={() => void handleTMDBSearch()} onSelectResult={item => void handleSelectResult(item, setMovieDurationStr)} onSelectSeason={season => void handleSelectSeason(season)} onShowResults={setShowResults} onSeasonsChange={setSeasons} onSelectedSeriesChange={setSelectedSeries} onChineseNameChange={value => set('chineseName', value)} onOriginalNameChange={value => set('originalName', value)} onReleaseYearChange={value => set('releaseYear', value)} />
          {/* Status & Progress & Total Episodes */}
          <div className="grid grid-cols-2 gap-3">
            <div>
              <label className="block text-sm font-medium text-gray-700 mb-1">观看状态</label>
              <select
                value={form.status}
                onChange={e => {
                  const newStatus = e.target.value as Status;
                  set('status', newStatus);

                  // 如果是电影且改为“已看”，自动填满进度
                  if (!isEpisodic && newStatus === '已看' && form.movieDuration) {
                    set('movieProgress', form.movieDuration);
                    setMovieProgressStr(formatMovieTime(form.movieDuration));
                  }

                  // 如果是电视剧且改为“已看”，自动填充最后一集
                  if (isEpisodic && newStatus === '已看') {
                    if (form.totalEpisodes) {
                      set('progress', `第${form.totalEpisodes}集`);
                    } else {
                      set('progress', '完结');
                    }
                  }
                }}
                className="w-full px-3 py-2 rounded-xl border border-gray-200 text-sm outline-none focus:border-indigo-400 focus:ring-2 focus:ring-indigo-100 transition bg-white"
              >
                {STATUSES.map(s => <option key={s} value={s}>{s}</option>)}
              </select>
            </div>

            {/* IMDb ID */}
            <div>
              <label className="block text-sm font-medium text-gray-700 mb-1">IMDb ID</label>
              <input
                type="text"
                value={form.imdbId || ''}
                onChange={e => set('imdbId', e.target.value || null)}
                placeholder="如 tt0111161"
                className="w-full px-3 py-2 rounded-xl border border-gray-200 text-sm outline-none focus:border-indigo-400 focus:ring-2 focus:ring-indigo-100 transition"
              />
            </div>

            {isEpisodic && (
              <div>
                <label className="block text-sm font-medium text-gray-700 mb-1">
                  进度
                  <span className="ml-1.5 text-xs text-gray-400 font-normal">（智能识别）</span>
                </label>
                <input
                  ref={progressRef}
                  type="text"
                  value={form.progress}
                  onChange={handleProgressChange}
                  onBlur={handleProgressBlur}
                  placeholder="如 S01、12、完"
                  className="w-full px-3 py-2 rounded-xl border border-gray-200 text-sm outline-none focus:border-indigo-400 focus:ring-2 focus:ring-indigo-100 transition"
                />
              </div>
            )}
          </div>

          <PlaybackFields form={form} isEpisodic={isEpisodic} movieProgressStr={movieProgressStr} movieDurationStr={movieDurationStr} onTotalEpisodesChange={value => set('totalEpisodes', value)} onMovieProgressChange={(value, seconds) => { setMovieProgressStr(value); set('movieProgress', seconds); }} onMovieDurationChange={(value, seconds) => { setMovieDurationStr(value); set('movieDuration', seconds); }} onPlatformChange={value => set('platform', value)} />
          <RecordDetailsFields form={form} startYearOnly={startYearOnly} endYearOnly={endYearOnly} years={years} onStartYearOnlyChange={setStartYearOnly} onEndYearOnlyChange={setEndYearOnly} onInterestLevelChange={value => set('interestLevel', value)} onRatingChange={value => set('rating', value)} onStartDateChange={value => set('startDate', value)} onEndDateChange={value => set('endDate', value)} onNotesChange={value => set('notes', value)} />
          <CollectionMembership
            collections={collections}
            selectedCollectionIds={selectedCollectionIds}
            onSelectedCollectionIdsChange={setSelectedCollectionIds}
            collectionDrafts={collectionDrafts}
            onCollectionDraftsChange={setCollectionDrafts}
            onNotify={onNotify}
          />

          {/* Buttons */}
          <div className="flex gap-3 pt-2">
            {record && onDelete && (
              <button
                type="button"
                onClick={() => { onDelete(record.id); onClose(); }}
                className="px-4 py-2.5 rounded-xl border border-red-200 text-sm font-medium text-red-600 hover:bg-red-50 transition-colors"
              >
                删除
              </button>
            )}
            <button
              type="button"
              onClick={onClose}
              className="flex-1 py-2.5 rounded-xl border border-gray-200 text-sm font-medium text-gray-600 hover:bg-gray-50 transition-colors"
            >
              取消
            </button>
            <button
              type="submit"
              className="flex-1 py-2.5 rounded-xl bg-indigo-600 hover:bg-indigo-700 text-white text-sm font-semibold transition-colors shadow-sm"
            >
              {record ? '保存修改' : '添加记录'}
            </button>
          </div>
        </form>
      </div>
    </div>
  );
}
