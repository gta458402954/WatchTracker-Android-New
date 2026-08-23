import { useEffect, useRef, useState } from 'react';
import { createPortal } from 'react-dom';
import type { SavedWatchlistViewV1 } from '../../../shared/lib/savedViews';

interface SavedViewMenuProps {
  views: SavedWatchlistViewV1[];
  activeViewId: string | null;
  startupViewId: string | null;
  dirty: boolean;
  onSelectAll: () => void;
  onSelect: (view: SavedWatchlistViewV1) => void;
  onCreate: (name: string) => Promise<void>;
  onUpdate: () => Promise<void>;
  onDelete: () => Promise<void>;
  onToggleStartup: () => Promise<void>;
}

export default function SavedViewMenu({ views, activeViewId, startupViewId, dirty, onSelectAll, onSelect, onCreate, onUpdate, onDelete, onToggleStartup }: SavedViewMenuProps) {
  const [open, setOpen] = useState(false);
  const [naming, setNaming] = useState(false);
  const [name, setName] = useState('');
  const [position, setPosition] = useState({ top: 0, left: 0 });
  const triggerRef = useRef<HTMLButtonElement>(null);
  const panelRef = useRef<HTMLDivElement>(null);
  const active = views.find(view => view.id === activeViewId) ?? null;
  const label = active ? `视图：${active.name}${dirty ? ' · 已修改' : ''}` : dirty ? '视图：临时筛选' : '视图：全部记录';

  useEffect(() => {
    if (!open) return;
    const updatePosition = () => {
      const rect = triggerRef.current?.getBoundingClientRect();
      if (!rect) return;
      setPosition({ top: rect.bottom + 8, left: Math.min(rect.left, Math.max(8, window.innerWidth - 296)) });
    };
    const closeOutside = (event: MouseEvent) => {
      const target = event.target as Node;
      if (!panelRef.current?.contains(target) && !triggerRef.current?.contains(target)) setOpen(false);
    };
    const closeWithEscape = (event: KeyboardEvent) => {
      if (event.key !== 'Escape') return;
      setOpen(false);
      triggerRef.current?.focus();
    };
    updatePosition();
    panelRef.current?.querySelector<HTMLElement>('button, input')?.focus();
    document.addEventListener('mousedown', closeOutside);
    document.addEventListener('keydown', closeWithEscape);
    window.addEventListener('resize', updatePosition);
    window.addEventListener('scroll', updatePosition, true);
    return () => {
      document.removeEventListener('mousedown', closeOutside);
      document.removeEventListener('keydown', closeWithEscape);
      window.removeEventListener('resize', updatePosition);
      window.removeEventListener('scroll', updatePosition, true);
    };
  }, [open]);

  async function submit() {
    try {
      await onCreate(name);
      setName('');
      setNaming(false);
      setOpen(false);
      triggerRef.current?.focus();
    } catch {
      // The owner reports a safe persistence/validation error. Keep the form
      // open so the name can be corrected or the operation retried.
    }
  }

  const selectAll = () => { onSelectAll(); setOpen(false); triggerRef.current?.focus(); };
  const selectView = (view: SavedWatchlistViewV1) => { onSelect(view); setOpen(false); triggerRef.current?.focus(); };

  return <>
    <button
      ref={triggerRef}
      type="button"
      aria-expanded={open}
      aria-controls="saved-view-panel"
      onClick={() => setOpen(current => !current)}
      className="h-9 max-w-48 shrink-0 truncate rounded-xl border border-gray-200 bg-white px-3 text-sm font-semibold text-gray-600 hover:bg-gray-50"
      title={label}
    >
      {label}
    </button>
    {open && createPortal(
      <div
        id="saved-view-panel"
        ref={panelRef}
        role="dialog"
        aria-label="保存视图"
        style={{ top: position.top, left: position.left }}
        className="fixed z-[80] w-72 max-w-[calc(100vw-1rem)] rounded-2xl border border-gray-100 bg-white p-3 shadow-2xl"
      >
        <div className="custom-scrollbar max-h-56 space-y-1 overflow-y-auto">
          <button type="button" aria-pressed={!activeViewId && !dirty} onClick={selectAll} className={`w-full rounded-xl px-3 py-2 text-left text-sm font-semibold ${!activeViewId && !dirty ? 'bg-indigo-50 text-indigo-700' : 'text-gray-600 hover:bg-gray-50'}`}>全部记录</button>
          {views.map(view => <button key={view.id} type="button" aria-pressed={activeViewId === view.id} onClick={() => selectView(view)} className={`w-full rounded-xl px-3 py-2 text-left text-sm font-semibold ${activeViewId === view.id ? 'bg-indigo-50 text-indigo-700' : 'text-gray-600 hover:bg-gray-50'}`}>{startupViewId === view.id ? '★ ' : ''}{view.name}{activeViewId === view.id && dirty ? ' · 已修改' : ''}</button>)}
        </div>
        <div className="mt-3 border-t border-gray-100 pt-3">
          {naming ? <form className="space-y-2" onSubmit={event => { event.preventDefault(); void submit(); }}>
            <input autoFocus aria-label="视图名称" maxLength={30} value={name} onChange={event => setName(event.target.value)} className="h-9 w-full rounded-xl border border-indigo-200 px-3 text-sm" placeholder="视图名称" />
            <div className="flex justify-end gap-3"><button type="button" onClick={() => { setNaming(false); setName(''); }} className="text-xs text-gray-400">取消</button><button type="submit" className="text-xs font-bold text-indigo-600">保存</button></div>
          </form> : <button type="button" onClick={() => setNaming(true)} className="w-full rounded-xl border border-gray-200 px-3 py-2 text-sm font-bold text-gray-600">另存当前条件为视图</button>}
          {active && <div className="mt-3 flex flex-wrap gap-x-4 gap-y-2">
            {dirty && <button type="button" onClick={() => void onUpdate()} className="text-xs font-bold text-indigo-600">更新当前视图</button>}
            <button type="button" onClick={() => void onToggleStartup()} className="text-xs font-bold text-amber-600">{startupViewId === active.id ? '取消启动视图' : '设为启动视图'}</button>
            <button type="button" onClick={() => void onDelete()} className="text-xs font-bold text-red-500">删除当前视图</button>
          </div>}
        </div>
      </div>,
      document.body,
    )}
  </>;
}
