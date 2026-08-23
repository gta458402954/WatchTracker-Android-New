import { useEffect, useState } from 'react';
import { MediaType, Status, WatchRecord } from '../../../shared/types';
import { STATUS_CONFIG } from '../../../shared/lib/constants';
import { mediaTypeOf, type RegionOption } from '../../../shared/lib/classification';
import type { RegionFilter } from '../../../shared/lib/countryNames';
import { partitionRegionOptions } from '../../../shared/lib/toolbarPresentation';
import RegionOverflowMenu from './RegionOverflowMenu';

interface StatsBarProps {
  records: WatchRecord[];
  regionOptions: RegionOption[];
  activeMediaTypes: MediaType[];
  onMediaTypeChange: (mediaType: MediaType | 'all') => void;
  filterStatuses: Status[];
  onFilterStatusChange: (status: Status | 'all') => void;
  activeRegions: RegionFilter[];
  onRegionChange: (region: RegionFilter) => void;
  lockFilter: 'all' | 'locked' | 'unlocked';
  onLockFilterChange: (value: 'all' | 'locked' | 'unlocked') => void;
}

function directRegionLimit(width: number): number {
  if (width < 480) return 1;
  if (width < 760) return 2;
  if (width < 1024) return 4;
  return 7;
}

export default function StatsBar({ records, regionOptions, activeMediaTypes, onMediaTypeChange, filterStatuses, onFilterStatusChange, activeRegions, onRegionChange, lockFilter, onLockFilterChange }: StatsBarProps) {
  const visible = records.filter(record => activeMediaTypes.length === 0 || activeMediaTypes.includes(mediaTypeOf(record)));
  const [regionLimit, setRegionLimit] = useState(() => directRegionLimit(typeof window === 'undefined' ? 1200 : window.innerWidth));

  useEffect(() => {
    const update = () => setRegionLimit(directRegionLimit(window.innerWidth));
    window.addEventListener('resize', update);
    return () => window.removeEventListener('resize', update);
  }, []);

  const { direct, overflow } = partitionRegionOptions(regionOptions, activeRegions, regionLimit);
  const selectRegion = (code: RegionFilter) => onRegionChange(activeRegions.includes(code) && activeRegions.length === 1 ? 'all' : code);
  const cycleLock = () => onLockFilterChange(lockFilter === 'all' ? 'locked' : lockFilter === 'locked' ? 'unlocked' : 'all');

  return <div className="border-b border-gray-100 bg-white">
    <div className="scrollbar-none flex gap-1 overflow-x-auto px-4 pt-4">
      {(['all', '电影', '剧集', '纪录片', '综艺', '动画'] as const).map(type => {
        const count = type === 'all' ? records.length : records.filter(record => mediaTypeOf(record) === type).length;
        const label = type === 'all' ? '全部' : type;
        const selected = type === 'all' ? activeMediaTypes.length === 0 : activeMediaTypes.includes(type);
        return <button key={type} aria-pressed={selected} onClick={() => onMediaTypeChange(type)} className={`shrink-0 rounded-t-xl border-b-2 px-3 py-2 text-sm font-medium ${selected ? 'border-indigo-600 bg-indigo-50 text-indigo-700' : 'border-transparent text-gray-400 hover:bg-gray-50 hover:text-gray-600'}`}>{label} {count}</button>;
      })}
    </div>
    <div className="scrollbar-none flex items-center gap-4 overflow-x-auto px-4 py-3">
      <div className="flex shrink-0 items-center gap-4">
        {(['all', '在看', '未看', '已看'] as const).map(status => {
          const config = status === 'all' ? { label: '全部', color: 'text-indigo-700', bg: 'bg-indigo-50 border-indigo-200' } : STATUS_CONFIG[status];
          const count = visible.filter(record => status === 'all' || record.status === status).length;
          const selected = status === 'all' ? filterStatuses.length === 0 : filterStatuses.includes(status);
          return <button key={status} aria-pressed={selected} onClick={() => onFilterStatusChange(status)} className={`flex shrink-0 items-center gap-1.5 ${selected ? '' : 'opacity-60'}`}><span className={`rounded-full border px-2.5 py-0.5 text-xs font-semibold ${config.bg} ${config.color}`}>{config.label}</span><span className="text-sm font-bold text-gray-700">{count}</span></button>;
        })}
      </div>
      {regionOptions.length > 0 && <div aria-label="地区筛选" className="flex shrink-0 items-center gap-2 border-l border-gray-200 pl-4">
        <span className="shrink-0 font-medium text-gray-400">地区</span>
        {direct.map(({ code, label, count }) => {
          const selected = activeRegions.includes(code);
          return <button key={code} type="button" aria-pressed={selected} onClick={() => selectRegion(code)} className={`shrink-0 whitespace-nowrap rounded-full border px-2.5 py-0.5 text-xs font-semibold transition-colors ${selected ? 'border-indigo-200 bg-indigo-50 text-indigo-700' : 'border-gray-200 bg-white text-gray-500 hover:border-indigo-200 hover:text-indigo-600'}`}>{label} <b>{count}</b></button>;
        })}
        {overflow.length > 0 && <RegionOverflowMenu options={overflow} activeRegions={activeRegions} onSelect={selectRegion} />}
      </div>}
      <button
        type="button"
        onClick={cycleLock}
        aria-label="锁定筛选"
        aria-pressed={lockFilter !== 'all'}
        title={lockFilter === 'locked' ? '仅显示已锁定' : lockFilter === 'unlocked' ? '仅显示未锁定' : '显示全部锁定状态'}
        className={`ml-auto flex h-8 shrink-0 items-center gap-1.5 whitespace-nowrap rounded-xl border px-3 text-xs font-semibold transition-colors ${lockFilter !== 'all' ? 'border-indigo-300 bg-indigo-100 text-indigo-700' : 'border-gray-200 text-gray-500 hover:bg-gray-50'}`}
      >
        <span aria-hidden="true">{lockFilter === 'locked' ? '🔒' : lockFilter === 'unlocked' ? '🔓' : '◉'}</span>
        <span>{lockFilter === 'locked' ? '已锁定' : lockFilter === 'unlocked' ? '未锁定' : '全部'}</span>
      </button>
    </div>
  </div>;
}
