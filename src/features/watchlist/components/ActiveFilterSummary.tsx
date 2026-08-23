import type { QuerySummaryItem } from '../../../shared/lib/watchlistQuery';

export default function ActiveFilterSummary({ items, onRemove, onClear }: {
  items: QuerySummaryItem[];
  onRemove: (dimension: QuerySummaryItem['dimension']) => void;
  onClear: () => void;
}) {
  if (!items.length) return null;
  return <div aria-label="当前高级筛选条件" className="border-b border-indigo-100 bg-indigo-50/60 px-4 py-2">
    <div className="mx-auto flex max-w-5xl flex-wrap items-center gap-2">
      <span className="text-xs font-bold text-indigo-700">高级条件</span>
      {items.map(item => <button key={item.dimension} type="button" onClick={() => onRemove(item.dimension)} className="rounded-full border border-indigo-200 bg-white px-2.5 py-1 text-xs text-indigo-700" title="移除此条件">{item.label} ×</button>)}
      <button type="button" onClick={onClear} className="ml-auto text-xs font-bold text-gray-500">清除高级条件</button>
    </div>
  </div>;
}
