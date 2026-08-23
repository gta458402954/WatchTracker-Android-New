import type { RefObject } from 'react';
import type { TmdbMedia, TmdbSeason } from '../../../shared/lib/classification';
import type { RecordFormValues } from './useTmdbRecordSearch';
import SafePosterImage from '../components/SafePosterImage';

interface TmdbSearchSectionProps {
  form: RecordFormValues;
  initialFocusRef: RefObject<HTMLInputElement | null>;
  isSearching: boolean;
  searchResults: TmdbMedia[];
  searchError: string | null;
  showResults: boolean;
  seasons: TmdbSeason[];
  selectedSeries: TmdbMedia | null;
  onSearch: () => void;
  onSelectResult: (item: TmdbMedia) => void;
  onSelectSeason: (season: TmdbSeason) => void;
  onShowResults: (show: boolean) => void;
  onSeasonsChange: (seasons: TmdbSeason[]) => void;
  onSelectedSeriesChange: (series: TmdbMedia | null) => void;
  onChineseNameChange: (value: string) => void;
  onOriginalNameChange: (value: string) => void;
  onReleaseYearChange: (value: string | null) => void;
}

export default function TmdbSearchSection({ form, initialFocusRef, isSearching, searchResults, searchError, showResults, seasons, selectedSeries, onSearch, onSelectResult, onSelectSeason, onShowResults, onSeasonsChange, onSelectedSeriesChange, onChineseNameChange, onOriginalNameChange, onReleaseYearChange }: TmdbSearchSectionProps) {
  return <div className="grid grid-cols-2 gap-3">
    <div className="relative col-span-2">
      <label className="block text-sm font-medium text-gray-700 mb-1">中文名 <span className="text-red-400">*</span></label>
      <div className="flex gap-2">
        <input ref={initialFocusRef} type="text" value={form.chineseName} onChange={event => onChineseNameChange(event.target.value)} placeholder="请输入中文名称" className="flex-1 px-3 py-2 rounded-xl border border-gray-200 text-sm outline-none focus:border-indigo-400 focus:ring-2 focus:ring-indigo-100 transition" required />
        <button type="button" onClick={onSearch} disabled={isSearching || (!form.chineseName && !form.originalName)} className="px-3 py-2 bg-indigo-50 text-indigo-600 rounded-xl text-xs font-semibold hover:bg-indigo-100 disabled:opacity-50 transition-colors flex items-center gap-1">{isSearching ? '搜索中...' : '🔍 自动填充'}</button>
      </div>
      {showResults && <div className="absolute z-[60] left-0 right-0 top-full mt-1 bg-white border border-gray-100 shadow-xl rounded-2xl max-h-60 overflow-y-auto p-1">
        <div className="flex items-center justify-between p-2 border-b border-gray-50"><div className="flex items-center gap-2">{seasons.length > 0 && <button type="button" onClick={() => { onSeasonsChange([]); onSelectedSeriesChange(null); }} className="p-1 hover:bg-gray-100 rounded-full text-indigo-600"><svg className="w-4 h-4" fill="none" stroke="currentColor" viewBox="0 0 24 24"><path strokeLinecap="round" strokeLinejoin="round" strokeWidth={2} d="M15 19l-7-7 7-7" /></svg></button>}<span className="text-[10px] font-bold text-gray-400 uppercase tracking-wider">{seasons.length > 0 ? `选择季节 (${selectedSeries?.name})` : '搜索结果 (TMDB)'}</span></div><button type="button" aria-label="关闭 TMDB 搜索结果" onClick={() => onShowResults(false)} className="text-gray-400 hover:text-gray-600"><svg className="w-4 h-4" fill="none" stroke="currentColor" viewBox="0 0 24 24"><path strokeLinecap="round" strokeLinejoin="round" strokeWidth={2} d="M6 18L18 6M6 6l12 12" /></svg></button></div>
        {seasons.length > 0 ? seasons.map(season => <button key={season.id} type="button" onClick={() => onSelectSeason(season)} className="w-full flex items-center justify-between p-3 hover:bg-indigo-50 rounded-xl transition-colors text-left"><div className="flex flex-col"><span className="text-sm font-bold text-gray-900">{season.name || `第 ${season.season_number} 季`}</span><span className="text-xs text-gray-400">{season.air_date?.split('-')[0] || '未知年份'} · {season.episode_count} 集</span></div><svg className="w-4 h-4 text-indigo-300" fill="none" stroke="currentColor" viewBox="0 0 24 24"><path strokeLinecap="round" strokeLinejoin="round" strokeWidth={2} d="M9 5l7 7-7 7" /></svg></button>) : (searchResults.length === 0 && !isSearching) || searchError ? <div className={`p-4 text-center text-sm ${searchError ? 'text-red-500' : 'text-gray-400'}`}>{searchError ? <div className="flex flex-col gap-1"><span className="font-bold">❌ 搜索出错</span><span className="text-xs opacity-80 break-all">{searchError.includes('error sending request') || searchError.includes('connection') ? '网络连接失败，请检查网络设置或代理' : searchError}</span></div> : '未找到相关影视'}</div> : searchResults.map(item => <button key={item.id} type="button" onClick={() => onSelectResult(item)} className="w-full flex items-start gap-3 p-2 hover:bg-indigo-50 rounded-xl transition-colors text-left">{item.poster_path ? <SafePosterImage key={item.poster_path} posterPath={item.poster_path} size="w92" alt="" compact className="w-10 h-14 object-cover rounded-md flex-shrink-0" /> : <div className="w-10 h-14 bg-gray-100 rounded-md flex-shrink-0 flex items-center justify-center text-xs text-gray-400">无图</div>}<div className="flex-1 min-w-0"><div className="text-sm font-bold text-gray-900 truncate">{item.media_type === 'movie' && <span className="text-[10px] bg-blue-100 text-blue-600 px-1 py-0.5 rounded mr-1">电影</span>}{item.media_type === 'tv' && <span className="text-[10px] bg-green-100 text-green-600 px-1 py-0.5 rounded mr-1">剧集</span>}{item.title || item.name}</div><div className="text-xs text-gray-400 truncate">{item.original_title || item.original_name}</div><div className="text-[10px] text-gray-400 mt-1">{item.release_date || item.first_air_date || '未知日期'}{(item.vote_average ?? 0) > 0 && ` · ⭐ ${item.vote_average?.toFixed(1)}`}</div></div></button>)}
      </div>}
    </div>
    <div><label className="block text-sm font-medium text-gray-700 mb-1">原文名</label><input type="text" value={form.originalName} onChange={event => onOriginalNameChange(event.target.value)} placeholder="英文 / 原名" className="w-full px-3 py-2 rounded-xl border border-gray-200 text-sm outline-none focus:border-indigo-400 focus:ring-2 focus:ring-indigo-100 transition" /></div>
    <div><label className="block text-sm font-medium text-gray-700 mb-1">发布年份</label><input type="text" value={form.releaseYear || ''} onChange={event => onReleaseYearChange(event.target.value || null)} placeholder="如 2024" className="w-full px-3 py-2 rounded-xl border border-gray-200 text-sm outline-none focus:border-indigo-400 focus:ring-2 focus:ring-indigo-100 transition" /></div>
  </div>;
}
