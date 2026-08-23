import { useMemo, useRef } from 'react';
import { useAccessibleDialog } from '../../../shared/lib/useAccessibleDialog';
import {
  type FilterOption,
  type NumberRange,
  type WatchlistFilterOptions,
  type WatchlistQueryV1,
} from '../../../shared/lib/watchlistQuery';

interface AdvancedFilterPanelProps {
  query: WatchlistQueryV1;
  options: WatchlistFilterOptions;
  onChange: (query: WatchlistQueryV1) => void;
  onClear: () => void;
  onClose: () => void;
}

function toggle(values: string[], value: string): string[] {
  return values.includes(value) ? values.filter(item => item !== value) : [...values, value];
}

function MultiOptions({ label, selected, options, onChange }: {
  label: string;
  selected: string[];
  options: FilterOption[];
  onChange: (values: string[]) => void;
}) {
  const allOptions = useMemo(() => {
    const known = new Set(options.map(option => option.value));
    return [
      ...options,
      ...selected.filter(value => !known.has(value)).map(value => ({ value, label: value, count: 0 })),
    ];
  }, [options, selected]);
  if (!allOptions.length) return null;
  return <fieldset className="space-y-2">
    <legend className="text-xs font-bold text-gray-600">{label}</legend>
    <div className="flex flex-wrap gap-2">
      {allOptions.map(option => {
        const active = selected.includes(option.value);
        return <button
          key={option.value}
          type="button"
          aria-pressed={active}
          onClick={() => onChange(toggle(selected, option.value))}
          className={`rounded-full border px-3 py-1.5 text-xs font-semibold ${active ? 'border-indigo-300 bg-indigo-50 text-indigo-700' : 'border-gray-200 bg-white text-gray-500'}`}
        >
          {option.label} <span className="opacity-60">{option.count}</span>{option.count === 0 ? ' · 当前无匹配值' : ''}
        </button>;
      })}
    </div>
  </fieldset>;
}

function RangeInputs({ label, value, onChange, min, max, step = 1 }: {
  label: string;
  value: NumberRange;
  onChange: (range: NumberRange) => void;
  min: number;
  max: number;
  step?: number;
}) {
  const update = (key: 'min' | 'max', raw: string) => onChange({
    ...value,
    [key]: raw === '' ? null : Number(raw),
  });
  return <fieldset>
    <legend className="mb-2 text-xs font-bold text-gray-600">{label}</legend>
    <div className="flex items-center gap-2">
      <input aria-label={`${label}最小值`} type="number" min={min} max={max} step={step} value={value.min ?? ''} onChange={event => update('min', event.target.value)} className="min-w-0 flex-1 rounded-xl border border-gray-200 px-3 py-2 text-sm" placeholder="不限" />
      <span className="text-gray-400">至</span>
      <input aria-label={`${label}最大值`} type="number" min={min} max={max} step={step} value={value.max ?? ''} onChange={event => update('max', event.target.value)} className="min-w-0 flex-1 rounded-xl border border-gray-200 px-3 py-2 text-sm" placeholder="不限" />
    </div>
  </fieldset>;
}

export default function AdvancedFilterPanel({ query, options, onChange, onClear, onClose }: AdvancedFilterPanelProps) {
  const closeRef = useRef<HTMLButtonElement>(null);
  const dialogRef = useAccessibleDialog<HTMLDivElement>({ onEscape: onClose, initialFocusRef: closeRef });
  const update = <K extends keyof WatchlistQueryV1>(key: K, value: WatchlistQueryV1[K]) => onChange({ ...query, [key]: value });

  return <div className="fixed inset-0 z-[70] flex items-start justify-center bg-black/30 p-0 sm:p-6" onMouseDown={event => event.target === event.currentTarget && onClose()}>
    <div ref={dialogRef} role="dialog" aria-modal="true" aria-labelledby="advanced-filter-title" tabIndex={-1} className="custom-scrollbar h-full w-full overflow-y-auto bg-white p-5 shadow-2xl outline-none sm:h-auto sm:max-h-[calc(100vh-3rem)] sm:max-w-3xl sm:rounded-3xl">
      <div className="mb-5 flex items-center justify-between">
        <div><h2 id="advanced-filter-title" className="text-lg font-black text-gray-900">高级筛选</h2><p className="text-xs text-gray-400">同一字段满足任一项，不同字段需要同时满足</p></div>
        <button ref={closeRef} type="button" aria-label="关闭高级筛选" onClick={onClose} className="rounded-xl px-3 py-2 text-gray-500 hover:bg-gray-100">✕</button>
      </div>
      <p className="mb-5 rounded-2xl bg-gray-50 px-4 py-3 text-xs leading-5 text-gray-500">媒体类型、观看状态、地区和锁定条件请使用顶部快捷筛选。</p>
      <div className="space-y-5">
        <MultiOptions label="平台" selected={query.platforms} options={options.platforms} onChange={values => update('platforms', values)} />
        <MultiOptions label="题材" selected={query.genres} options={options.genres} onChange={values => update('genres', values)} />
        <MultiOptions label="内容标签" selected={query.contentTags} options={options.contentTags} onChange={values => update('contentTags', values)} />
        <div className="grid gap-4 sm:grid-cols-3">
          <RangeInputs label="上映年份" value={query.releaseYear} min={1800} max={2200} onChange={value => update('releaseYear', value)} />
          <RangeInputs label="个人评分" value={query.rating} min={0} max={10} step={0.1} onChange={value => update('rating', value)} />
          <RangeInputs label="IMDb 评分" value={query.imdbRating} min={0} max={10} step={0.1} onChange={value => update('imdbRating', value)} />
        </div>
      </div>
      <div className="sticky bottom-0 mt-6 flex justify-end gap-3 border-t border-gray-100 bg-white pt-4">
        <button type="button" onClick={onClear} className="rounded-xl border border-gray-200 px-4 py-2 text-sm font-semibold text-gray-500">清除高级条件</button>
        <button type="button" onClick={onClose} className="rounded-xl bg-indigo-600 px-5 py-2 text-sm font-bold text-white">完成</button>
      </div>
    </div>
  </div>;
}
