import { useMemo, useRef, useState } from 'react';
import type { CollectionDraft, WatchCollection } from '../../../shared/types';
import type { NoticeTone } from '../../../shared/lib/feedback';
import { useAccessibleDialog } from '../../../shared/lib/useAccessibleDialog';

interface CollectionMembershipProps {
  collections: WatchCollection[];
  selectedCollectionIds: string[];
  onSelectedCollectionIdsChange: (ids: string[]) => void;
  collectionDrafts: CollectionDraft[];
  onCollectionDraftsChange: (drafts: CollectionDraft[]) => void;
  onNotify?: (tone: NoticeTone, message: string) => void;
}

export default function CollectionMembership({ collections, selectedCollectionIds, onSelectedCollectionIdsChange, collectionDrafts, onCollectionDraftsChange, onNotify }: CollectionMembershipProps) {
  const [showManager, setShowManager] = useState(false);
  const [search, setSearch] = useState('');
  const [creating, setCreating] = useState(false);
  const [draftName, setDraftName] = useState('');
  const [draftKind, setDraftKind] = useState<WatchCollection['collectionKind']>('manual');
  const managerTriggerRef = useRef<HTMLButtonElement>(null);
  const searchInputRef = useRef<HTMLInputElement>(null);
  const managerDialogRef = useAccessibleDialog<HTMLDivElement>({ enabled: showManager, onEscape: () => setShowManager(false), initialFocusRef: searchInputRef });
  const selectedCollections = useMemo(() => collections.filter(item => selectedCollectionIds.includes(item.id)), [collections, selectedCollectionIds]);
  const visibleCollections = useMemo(() => {
    const query = search.trim().toLowerCase();
    return collections.filter(item => !query || item.name.toLowerCase().includes(query));
  }, [collections, search]);
  const setSelected = (ids: string[]) => onSelectedCollectionIdsChange(ids);
  const setDrafts = (drafts: CollectionDraft[]) => onCollectionDraftsChange(drafts);

  return <>
    <section className="rounded-2xl border border-gray-100 bg-gray-50/70 p-3" aria-labelledby="record-groups-title">
      <div className="flex items-center justify-between gap-3">
        <div className="min-w-0"><h3 id="record-groups-title" className="text-sm font-semibold text-gray-700">整理与归组</h3><p className="mt-0.5 text-[10px] text-gray-400">只调整收藏集关系，不修改条目内容</p></div>
        <button ref={managerTriggerRef} type="button" onClick={() => setShowManager(true)} className="shrink-0 rounded-lg border border-indigo-100 bg-white px-3 py-1.5 text-xs font-bold text-indigo-600 hover:bg-indigo-50">管理</button>
      </div>
      <div className="mt-3 flex min-w-0 flex-wrap items-center gap-2">
        {!selectedCollections.length && !collectionDrafts.length && <span className="text-xs text-gray-400">尚未加入收藏集</span>}
        {selectedCollections.slice(0, 2).map(collection => <span key={collection.id} className="max-w-full truncate rounded-full border border-indigo-200 bg-indigo-50 px-3 py-1.5 text-xs font-semibold text-indigo-700">🎞️ {collection.name}</span>)}
        {collectionDrafts.map(draft => <span key={draft.temporaryId} className="max-w-full truncate rounded-full border border-dashed border-indigo-300 bg-white px-3 py-1.5 text-xs font-semibold text-indigo-700">＋ {draft.name} · 待创建</span>)}
        {selectedCollections.length > 2 && <span className="rounded-full bg-gray-200 px-2.5 py-1.5 text-xs font-bold text-gray-600">+{selectedCollections.length - 2}</span>}
      </div>
    </section>

    {showManager && <div className="fixed inset-0 z-[80] flex items-center justify-center bg-slate-900/35 p-4" onMouseDown={event => { if (event.target === event.currentTarget) setShowManager(false); }}>
      <div ref={managerDialogRef} role="dialog" aria-modal="true" aria-label="管理所属收藏集" tabIndex={-1} className="flex max-h-[75vh] w-full max-w-md flex-col rounded-3xl bg-white p-5 shadow-2xl">
        <div className="flex items-center justify-between gap-3"><div><h3 className="text-lg font-black text-gray-900">管理所属收藏集</h3><p className="mt-1 text-xs text-gray-400">选择会在保存记录时生效</p></div><button type="button" aria-label="关闭收藏集管理" onClick={() => setShowManager(false)} className="rounded-lg px-2 py-1 text-xl text-gray-400 hover:bg-gray-100">×</button></div>
        <div className="mt-4 flex gap-2"><input ref={searchInputRef} value={search} onChange={event => setSearch(event.target.value)} placeholder="搜索收藏集" className="min-w-0 flex-1 rounded-xl border border-gray-200 px-4 py-2.5 text-sm outline-none focus:border-indigo-400" /><button type="button" onClick={() => { setCreating(true); setDraftName(search); }} className="shrink-0 rounded-xl border border-indigo-200 px-3 text-xs font-bold text-indigo-600">＋ 新建</button></div>
        {creating && <div className="mt-3 space-y-2 rounded-2xl border border-indigo-100 bg-indigo-50/50 p-3"><input value={draftName} onChange={event => setDraftName(event.target.value)} placeholder="收藏集名称" maxLength={80} className="w-full rounded-xl border px-3 py-2 text-sm" /><select value={draftKind} onChange={event => setDraftKind(event.target.value as WatchCollection['collectionKind'])} className="w-full rounded-xl border px-3 py-2 text-sm"><option value="manual">普通收藏集</option><option value="tv-series">电视剧系列</option><option value="movie-series">电影系列</option><option value="universe">影视宇宙</option></select><div className="flex gap-2"><button type="button" disabled={!draftName.trim()} onClick={() => { const normalized = draftName.trim().toLowerCase(); if (collections.some(item => item.normalizedName === normalized) || collectionDrafts.some(item => item.name.trim().toLowerCase() === normalized)) { onNotify?.('warning', '已经存在同名收藏集。'); return; } setDrafts([...collectionDrafts, { temporaryId: crypto.randomUUID(), name: draftName.trim(), description: null, collectionKind: draftKind }]); setCreating(false); setDraftName(''); }} className="flex-1 rounded-lg bg-indigo-600 py-2 text-xs font-bold text-white disabled:bg-gray-300">加入待创建列表</button><button type="button" onClick={() => setCreating(false)} className="rounded-lg border bg-white px-3 text-xs">取消</button></div></div>}
        <div className="mt-3 flex-1 space-y-4 overflow-y-auto">
          {collectionDrafts.length > 0 && <section><h4 className="mb-1 px-1 text-[11px] font-bold uppercase tracking-wider text-gray-400">待创建</h4>{collectionDrafts.map(draft => <div key={draft.temporaryId} className="flex items-center gap-3 rounded-xl px-3 py-2.5"><span className="min-w-0 flex-1 truncate text-sm font-semibold text-indigo-700">＋ {draft.name}</span><span className="text-[10px] text-gray-400">{draft.collectionKind === 'tv-series' ? '电视剧系列' : draft.collectionKind === 'movie-series' ? '电影系列' : draft.collectionKind === 'universe' ? '影视宇宙' : '普通收藏集'}</span><button type="button" onClick={() => setDrafts(collectionDrafts.filter(item => item.temporaryId !== draft.temporaryId))} className="text-red-400">×</button></div>)}</section>}
          {(['已加入', '其他收藏集'] as const).map(group => { const values = visibleCollections.filter(item => group === '已加入' ? selectedCollectionIds.includes(item.id) : !selectedCollectionIds.includes(item.id)); if (!values.length) return null; return <section key={group}><h4 className="mb-1 px-1 text-[11px] font-bold uppercase tracking-wider text-gray-400">{group}</h4>{values.map(collection => { const checked = selectedCollectionIds.includes(collection.id); return <label key={collection.id} className="flex cursor-pointer items-center gap-3 rounded-xl px-3 py-2.5 hover:bg-gray-50"><input type="checkbox" checked={checked} onChange={() => setSelected(checked ? selectedCollectionIds.filter(id => id !== collection.id) : [...selectedCollectionIds, collection.id])} className="h-4 w-4 rounded border-gray-300 text-indigo-600" /><span className="min-w-0 flex-1 truncate text-sm font-semibold text-gray-700">🎞️ {collection.name}</span>{checked && <span className="text-xs font-bold text-emerald-600">已加入</span>}</label>; })}</section>; })}
          {!visibleCollections.length && !collectionDrafts.length && <p className="py-8 text-center text-sm text-gray-400">没有匹配的收藏集，可直接新建</p>}
        </div>
        <button type="button" onClick={() => setShowManager(false)} className="mt-4 rounded-xl bg-indigo-600 py-2.5 text-sm font-bold text-white">完成</button>
      </div>
    </div>}
  </>;
}
