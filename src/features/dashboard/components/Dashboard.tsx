import { useMemo, useRef, useState } from 'react';
import { WatchRecord } from '../../../shared/types';
import { getGenreDistribution } from '../../../shared/lib/analytics';
import { mediaTypeOf } from '../../../shared/lib/classification';
import { displayTitlesOf } from '../../../shared/lib/displayTitle';
import { dashboardWatchingProgress } from '../../../shared/lib/dashboardProgress';
import { useAccessibleDialog } from '../../../shared/lib/useAccessibleDialog';
import {
  buildDiscoveryQueue,
  discoveryEmptyMessage,
  discoveryFilterOptions,
  estimateDiscoveryViewing,
  type DiscoveryDurationLimit,
  type DiscoveryFilters,
  type DiscoveryMediaFilter,
} from '../../../shared/lib/discovery';
import { Bar, BarChart, ResponsiveContainer, Tooltip, XAxis, YAxis } from 'recharts';

interface DashboardProps { onClose: () => void; records: WatchRecord[]; }
type DateRange = 'week' | 'month' | 'year' | 'all';
const RANGE_LABELS: Record<DateRange, string> = { week: '近 7 天', month: '近 30 天', year: '近一年', all: '全部' };

function completionDate(record: WatchRecord) {
  if (!record.endDate) return null;
  const value = /^\d{4}-\d{2}-\d{2}$/.test(record.endDate) ? record.endDate + 'T00:00:00' : record.endDate;
  const date = new Date(value);
  return Number.isNaN(date.getTime()) ? null : date;
}
function completedMinutes(record: WatchRecord) {
  const episodic = Boolean(record.totalEpisodes) || ['剧集', '综艺'].includes(mediaTypeOf(record));
  return episodic
    ? (record.totalEpisodes || 1) * (record.episodeRuntime || 45)
    : estimateDiscoveryViewing(record).minutes;
}
function formatDuration(minutes: number) { return minutes >= 60 ? `${Math.floor(minutes / 60)} 小时 ${minutes % 60 ? `${minutes % 60} 分` : ''}` : `${minutes} 分`; }

const DEFAULT_DISCOVERY_FILTERS: DiscoveryFilters = {
  durationLimit: 120,
  mediaType: '全部',
  platform: null,
  endedOnly: false,
};
const DURATION_LABELS: Record<DiscoveryDurationLimit, string> = {
  30: '30 分钟内',
  60: '1 小时内',
  120: '2 小时内',
  0: '不限',
};

export default function Dashboard({ onClose, records }: DashboardProps) {
  const [range, setRange] = useState<DateRange>('all');
  const [discoveryFilters, setDiscoveryFilters] = useState<DiscoveryFilters>(DEFAULT_DISCOVERY_FILTERS);
  const [seenIds, setSeenIds] = useState<Set<string>>(() => new Set());
  const [skippedIds, setSkippedIds] = useState<Set<string>>(() => new Set());
  const [detailRecordId, setDetailRecordId] = useState<string | null>(null);
  const initialFocusRef = useRef<HTMLButtonElement>(null);
  const dialogRef = useAccessibleDialog<HTMLDivElement>({ onEscape: onClose, initialFocusRef });

  const rangeStart = useMemo(() => {
    const start = new Date();
    if (range === 'week') start.setDate(start.getDate() - 6);
    else if (range === 'month') start.setDate(start.getDate() - 29);
    else if (range === 'year') start.setFullYear(start.getFullYear() - 1);
    else start.setFullYear(1970);
    start.setHours(0, 0, 0, 0);
    return start;
  }, [range]);

  const summary = useMemo(() => {
    const completed = records.filter(record => record.status === '已看' && completionDate(record) && completionDate(record)! >= rangeStart);
    const added = records.filter(record => new Date(record.createdAt) >= rangeStart);
    const watching = records.filter(record => record.status === '在看');
    const pending = records.filter(record => record.status === '未看');
    const minutes = completed.reduce((total, record) => total + completedMinutes(record), 0);
    return { completed, added, watching, pending, minutes };
  }, [records, rangeStart]);

  const discoveryOptions = useMemo(() => discoveryFilterOptions(records), [records]);
  const discoveryQueue = useMemo(
    () => buildDiscoveryQueue(records, discoveryFilters, skippedIds),
    [discoveryFilters, records, skippedIds],
  );
  const recommendation = discoveryQueue.candidates.find(candidate => !seenIds.has(candidate.record.id)) ?? null;
  const detailRecord = detailRecordId ? records.find(record => record.id === detailRecordId) ?? null : null;
  const recommendationTitles = recommendation ? displayTitlesOf(recommendation.record) : null;
  const detailTitles = detailRecord ? displayTitlesOf(detailRecord) : null;
  const roundExhausted = discoveryQueue.candidates.length > 0 && recommendation === null;

  const updateDiscoveryFilters = (updates: Partial<DiscoveryFilters>) => {
    setDiscoveryFilters(current => ({ ...current, ...updates }));
    setSeenIds(new Set());
  };

  const showNextRecommendation = () => {
    if (!recommendation) return;
    setSeenIds(current => new Set(current).add(recommendation.record.id));
  };

  const skipRecommendation = () => {
    if (!recommendation) return;
    setSkippedIds(current => new Set(current).add(recommendation.record.id));
    setSeenIds(current => new Set(current).add(recommendation.record.id));
  };

  const trend = useMemo(() => {
    const count = range === 'week' ? 7 : range === 'month' ? 5 : 12;
    const buckets = Array.from({ length: count }, () => ({ name: '', value: 0, start: new Date() }));
    buckets.forEach((bucket, index) => {
      const date = new Date();
      if (range === 'week') date.setDate(date.getDate() - (count - 1 - index));
      else if (range === 'month') date.setDate(date.getDate() - (count - 1 - index) * 7);
      else date.setMonth(date.getMonth() - (count - 1 - index));
      date.setHours(0, 0, 0, 0);
      bucket.start = date;
      bucket.name = range === 'week' ? `${date.getMonth() + 1}/${date.getDate()}` : range === 'month' ? `第 ${index + 1} 周` : `${date.getMonth() + 1} 月`;
    });
    summary.completed.forEach(record => {
      const date = completionDate(record);
      if (!date) return;
      const index = range === 'week'
        ? Math.floor((date.getTime() - buckets[0].start.getTime()) / 86_400_000)
        : range === 'month'
          ? Math.floor((date.getTime() - buckets[0].start.getTime()) / (7 * 86_400_000))
          : (date.getFullYear() - buckets[0].start.getFullYear()) * 12 + date.getMonth() - buckets[0].start.getMonth();
      if (index >= 0 && index < buckets.length) buckets[index].value++;
    });
    return buckets.map(({ name, value }) => ({ name, value }));
  }, [range, summary.completed]);

  const recentCompleted = useMemo(() => [...summary.completed].sort((a, b) => completionDate(b)!.getTime() - completionDate(a)!.getTime()).slice(0, 3), [summary.completed]);
  const genres = useMemo(() => getGenreDistribution(records.filter(record => record.status === '已看')).slice(0, 5), [records]);

  return <div ref={dialogRef} role="dialog" aria-modal="true" aria-labelledby="dashboard-dialog-title" tabIndex={-1} className="fixed inset-0 z-50 flex flex-col bg-[#0b1020] p-5 text-slate-200 sm:p-7 overflow-hidden">
    <header className="relative z-10 flex shrink-0 items-center justify-between border-b border-slate-800 pb-5">
      <div><p className="text-xs font-bold uppercase tracking-[0.2em] text-indigo-400">WatchTracker</p><h2 id="dashboard-dialog-title" className="mt-1 text-2xl font-black text-white">观看概览</h2></div>
      <div className="flex items-center gap-3"><div className="hidden rounded-xl bg-slate-900 p-1 sm:flex">{(Object.keys(RANGE_LABELS) as DateRange[]).map(item => <button key={item} onClick={() => setRange(item)} className={`rounded-lg px-3 py-2 text-xs font-bold transition ${range === item ? 'bg-indigo-500 text-white shadow' : 'text-slate-400 hover:text-white'}`}>{RANGE_LABELS[item]}</button>)}</div><button ref={initialFocusRef} onClick={onClose} className="rounded-xl bg-slate-800 p-2 text-slate-400 hover:bg-slate-700 hover:text-white" aria-label="关闭观看概览"><svg className="h-6 w-6" fill="none" stroke="currentColor" viewBox="0 0 24 24"><path strokeLinecap="round" strokeLinejoin="round" strokeWidth="2" d="M6 18 18 6M6 6l12 12" /></svg></button></div>
    </header>
    <div className="relative z-10 mt-4 flex gap-2 overflow-x-auto pb-1 sm:hidden">{(Object.keys(RANGE_LABELS) as DateRange[]).map(item => <button key={item} onClick={() => setRange(item)} className={`shrink-0 rounded-lg px-3 py-2 text-xs font-bold ${range === item ? 'bg-indigo-500 text-white' : 'bg-slate-800 text-slate-400'}`}>{RANGE_LABELS[item]}</button>)}</div>
    <main className="custom-scrollbar relative z-10 mt-5 flex-1 overflow-y-auto pr-2">
      <section aria-label="今晚看什么推荐" className="rounded-2xl border border-indigo-500/30 bg-gradient-to-r from-indigo-950 to-slate-900 p-5">
        <div className="flex flex-wrap items-center gap-2 border-b border-indigo-400/15 pb-4">
          <span className="mr-1 text-xs font-bold text-indigo-300">今晚看什么</span>
          {([30, 60, 120, 0] as DiscoveryDurationLimit[]).map(minutes => (
            <button
              key={minutes}
              type="button"
              onClick={() => updateDiscoveryFilters({ durationLimit: minutes })}
              className={`rounded-lg px-3 py-2 text-xs font-bold ${discoveryFilters.durationLimit === minutes ? 'bg-indigo-500 text-white' : 'bg-slate-800 text-slate-300'}`}
            >
              {DURATION_LABELS[minutes]}
            </button>
          ))}
          <select
            aria-label="推荐媒体类型"
            value={discoveryFilters.mediaType}
            onChange={event => updateDiscoveryFilters({ mediaType: event.target.value as DiscoveryMediaFilter })}
            className="rounded-lg border border-slate-700 bg-slate-800 px-3 py-2 text-xs font-bold text-slate-200 outline-none"
          >
            <option value="全部">全部类型</option>
            {discoveryOptions.mediaTypes.map(type => <option key={type} value={type}>{type}</option>)}
          </select>
          <select
            aria-label="推荐平台"
            value={discoveryFilters.platform ?? ''}
            onChange={event => updateDiscoveryFilters({ platform: event.target.value || null })}
            className="rounded-lg border border-slate-700 bg-slate-800 px-3 py-2 text-xs font-bold text-slate-200 outline-none"
          >
            <option value="">全部平台</option>
            {discoveryOptions.platforms.map(platform => <option key={platform} value={platform}>{platform}</option>)}
          </select>
          <label className="flex cursor-pointer items-center gap-2 rounded-lg bg-slate-800 px-3 py-2 text-xs font-bold text-slate-300">
            <input
              type="checkbox"
              checked={discoveryFilters.endedOnly}
              onChange={event => updateDiscoveryFilters({ endedOnly: event.target.checked })}
              className="accent-indigo-500"
            />
            仅已完结
          </label>
        </div>

        {recommendation ? (
          <div className="mt-4 flex flex-col justify-between gap-4 md:flex-row md:items-end">
            <div className="min-w-0">
              <div className="flex flex-wrap items-center gap-2">
                <h3 className="text-xl font-black text-white">{recommendationTitles?.primary}</h3>
                {recommendation.record.isLocked && <span className="rounded-full bg-amber-400/10 px-2 py-0.5 text-[10px] font-bold text-amber-300">已锁定 · 可查看</span>}
              </div>
              <p className="mt-2 text-sm text-slate-300">
                {mediaTypeOf(recommendation.record)} · {recommendation.viewing.episodic ? '单集' : '整部'}约 {formatDuration(recommendation.viewing.minutes)} · {recommendation.breakdown.completion ? '已完结' : '尚未完结'} · 推荐分 {recommendation.score}
              </p>
              <p className="mt-2 text-sm text-indigo-200">推荐理由：{recommendation.reasons.length ? recommendation.reasons.join(' · ') : '符合当前筛选条件'}</p>
              {(recommendation.viewing.estimated || recommendation.notes.length > 0) && (
                <p className="mt-1 text-xs text-slate-500">
                  {[recommendation.viewing.estimated
                    ? `${recommendation.viewing.episodic ? '单集' : '整部'}时长未知，按 ${recommendation.viewing.minutes} 分钟估算`
                    : null, ...recommendation.notes].filter(Boolean).join(' · ')}
                </p>
              )}
            </div>
            <div className="flex shrink-0 flex-wrap gap-2">
              <button type="button" onClick={() => setDetailRecordId(recommendation.record.id)} className="rounded-lg bg-white/10 px-3 py-2 text-xs font-bold text-white hover:bg-white/20">查看条目</button>
              <button type="button" onClick={skipRecommendation} className="rounded-lg bg-amber-400/10 px-3 py-2 text-xs font-bold text-amber-200 hover:bg-amber-400/20">本轮跳过</button>
              <button type="button" onClick={showNextRecommendation} className="rounded-lg bg-indigo-500 px-3 py-2 text-xs font-bold text-white hover:bg-indigo-400">换一个</button>
            </div>
          </div>
        ) : (
          <div className="mt-5 flex flex-col items-start justify-between gap-3 md:flex-row md:items-center">
            <div>
              <h3 className="text-lg font-black text-white">{roundExhausted ? '本轮候选已看完' : discoveryEmptyMessage(discoveryQueue.emptyReason)}</h3>
              <p className="mt-1 text-sm text-slate-400">不会自动放宽筛选，也不会加入已看或在看条目。</p>
            </div>
            {roundExhausted && (
              <button type="button" onClick={() => setSeenIds(new Set())} className="rounded-lg bg-indigo-500 px-3 py-2 text-xs font-bold text-white hover:bg-indigo-400">重新浏览</button>
            )}
          </div>
        )}

        {detailRecord && (
          <div role="region" aria-labelledby="discovery-detail-title" className="mt-4 rounded-2xl border border-slate-700 bg-slate-950/80 p-4">
            <div className="flex items-start justify-between gap-4">
              <div>
                <p className="text-[10px] font-bold uppercase tracking-wider text-indigo-300">只读条目摘要</p>
                <h4 id="discovery-detail-title" className="mt-1 text-lg font-black text-white">{detailTitles?.primary}</h4>
                {detailTitles?.secondary && <p className="mt-1 text-xs text-slate-500">{detailTitles.secondary}</p>}
              </div>
              <button type="button" onClick={() => setDetailRecordId(null)} aria-label="关闭条目摘要" className="rounded-lg bg-slate-800 px-3 py-1.5 text-xs font-bold text-slate-300 hover:text-white">关闭</button>
            </div>
            <div className="mt-3 grid gap-2 text-xs text-slate-300 sm:grid-cols-2 lg:grid-cols-4">
              <p>类型：{mediaTypeOf(detailRecord)}</p>
              <p>年份：{detailRecord.releaseYear || '未知'}</p>
              <p>平台：{detailRecord.platform || '未知'}</p>
              <p>IMDb：{detailRecord.imdbRating?.toFixed(1) || '未知'}</p>
              <p className="sm:col-span-2">题材：{detailRecord.genres || '未知'}</p>
              <p className="sm:col-span-2">备注：{detailRecord.notes || '无'}</p>
            </div>
          </div>
        )}
      </section>
      <section className="mt-5 grid gap-4 md:grid-cols-3"><article className="rounded-2xl border border-slate-800 bg-slate-900/70 p-5"><p className="text-xs font-bold text-slate-500">本期完成</p><p className="mt-2 text-3xl font-black text-white">{summary.completed.length}<span className="ml-1 text-sm font-medium text-slate-400">部</span></p><p className="mt-2 text-sm text-slate-400">估算观看时长 {formatDuration(summary.minutes)}</p></article><article className="rounded-2xl border border-slate-800 bg-slate-900/70 p-5"><p className="text-xs font-bold text-slate-500">待看清单</p><p className="mt-2 text-3xl font-black text-white">{summary.pending.length}<span className="ml-1 text-sm font-medium text-slate-400">部</span></p><p className="mt-2 text-sm text-slate-400">全量待看 · 在看 {summary.watching.length} 部</p></article><article className="rounded-2xl border border-slate-800 bg-slate-900/70 p-5"><p className="text-xs font-bold text-slate-500">本期新增</p><p className="mt-2 text-3xl font-black text-emerald-400">{summary.added.length}<span className="ml-1 text-sm font-medium text-slate-400">部</span></p><p className="mt-2 text-sm text-slate-400">加入清单的影视条目</p></article></section>
      <section className="mt-5 grid gap-5 lg:grid-cols-[1.35fr_0.65fr]"><article className="rounded-2xl border border-slate-800 bg-slate-900/60 p-5"><div className="flex items-center justify-between"><h3 className="font-bold text-white">完成趋势</h3><span className="text-xs text-slate-500">{range === 'week' ? '按日' : range === 'month' ? '按周' : range === 'all' ? '近 12 个月 · 按月' : '按月'}</span></div><div className="mt-4 h-52"><ResponsiveContainer width="100%" height="100%"><BarChart data={trend}><XAxis dataKey="name" tickLine={false} axisLine={false} stroke="#64748b" fontSize={11}/><YAxis allowDecimals={false} tickLine={false} axisLine={false} stroke="#64748b" fontSize={11}/><Tooltip contentStyle={{ backgroundColor: '#0f172a', borderColor: '#334155', borderRadius: '10px' }}/><Bar dataKey="value" fill="#818cf8" radius={[5,5,0,0]}/></BarChart></ResponsiveContainer></div></article><article className="rounded-2xl border border-slate-800 bg-slate-900/60 p-5"><h3 className="font-bold text-white">正在观看</h3><div className="mt-3 space-y-3">{summary.watching.length ? summary.watching.slice(0,3).map(record => <div key={record.id} className="rounded-xl bg-slate-800/70 p-3"><p className="truncate text-sm font-bold text-white">{displayTitlesOf(record).primary}</p><p className="mt-1 text-xs text-slate-400">{dashboardWatchingProgress(record)}</p></div>) : <p className="py-8 text-center text-sm text-slate-500">暂无正在观看的项目</p>}</div></article></section>
      <section className="mt-5 grid gap-5 pb-5 lg:grid-cols-2"><article className="rounded-2xl border border-slate-800 bg-slate-900/60 p-5"><h3 className="font-bold text-white">最近完成</h3><div className="mt-3 divide-y divide-slate-800">{recentCompleted.length ? recentCompleted.map(record => <div key={record.id} className="flex items-center justify-between py-3"><div><p className="text-sm font-semibold text-white">{displayTitlesOf(record).primary}</p><p className="mt-1 text-xs text-slate-500">{mediaTypeOf(record)} · {record.endDate || "完成日期未知"}</p></div><span className="text-sm font-bold text-amber-400">{record.rating ? `★ ${record.rating}` : '未评分'}</span></div>) : <p className="py-8 text-center text-sm text-slate-500">当前范围内暂无完成记录</p>}</div></article><article className="rounded-2xl border border-slate-800 bg-slate-900/60 p-5"><h3 className="font-bold text-white">常看题材</h3><div className="mt-4 space-y-3">{genres.map(genre => <div key={genre.name}><div className="mb-1 flex justify-between text-xs"><span className="text-slate-300">{genre.name}</span><span className="text-slate-500">{genre.value} 部</span></div><div className="h-2 rounded-full bg-slate-800"><div className="h-full rounded-full bg-violet-400" style={{ width: `${genres[0] ? genre.value / genres[0].value * 100 : 0}%` }}/></div></div>)}</div></article></section>
    </main>
  </div>;
}
