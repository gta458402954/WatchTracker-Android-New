import { useEffect, useRef, useState } from 'react';
import { createPortal } from 'react-dom';

interface MoreActionsMenuProps {
  viewMode: 'list' | 'poster';
  onViewModeChange: (value: 'list' | 'poster') => void;
  onShowDashboard: () => void;
  onShowCollections: () => void;
  onShowForm: () => void;
}

export default function MoreActionsMenu({ viewMode, onViewModeChange, onShowDashboard, onShowCollections, onShowForm }: MoreActionsMenuProps) {
  const [open, setOpen] = useState(false);
  const [position, setPosition] = useState({ top: 0, right: 8 });
  const triggerRef = useRef<HTMLButtonElement>(null);
  const panelRef = useRef<HTMLDivElement>(null);

  useEffect(() => {
    if (!open) return;
    const updatePosition = () => {
      const rect = triggerRef.current?.getBoundingClientRect();
      if (rect) setPosition({ top: rect.bottom + 8, right: Math.max(8, window.innerWidth - rect.right) });
    };
    const closeOutside = (event: MouseEvent) => {
      const target = event.target as Node;
      if (!panelRef.current?.contains(target) && !triggerRef.current?.contains(target)) setOpen(false);
    };
    const closeWithEscape = (event: KeyboardEvent) => {
      if (event.key === 'Escape') { setOpen(false); triggerRef.current?.focus(); }
    };
    updatePosition();
    panelRef.current?.querySelector<HTMLElement>('button')?.focus();
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

  const run = (action: () => void) => {
    setOpen(false);
    triggerRef.current?.focus();
    action();
  };

  const moveFocus = (event: React.KeyboardEvent<HTMLDivElement>) => {
    if (event.key !== 'ArrowDown' && event.key !== 'ArrowUp') return;
    const items = Array.from(panelRef.current?.querySelectorAll<HTMLElement>('[role="menuitem"]') ?? []);
    if (!items.length) return;
    event.preventDefault();
    const current = items.indexOf(document.activeElement as HTMLElement);
    const offset = event.key === 'ArrowDown' ? 1 : -1;
    items[(current + offset + items.length) % items.length]?.focus();
  };

  return <>
    <button
      ref={triggerRef}
      type="button"
      aria-label="更多操作"
      aria-expanded={open}
      aria-controls="more-actions-menu"
      title="更多操作"
      onClick={() => setOpen(current => !current)}
      className="flex h-9 w-9 shrink-0 items-center justify-center rounded-xl border border-gray-200 text-lg font-bold text-gray-500 hover:bg-gray-50"
    >
      <span aria-hidden="true">⋯</span>
    </button>
    {open && createPortal(
      <div
        id="more-actions-menu"
        ref={panelRef}
        role="menu"
        aria-label="更多操作"
        onKeyDown={moveFocus}
        style={{ top: position.top, right: position.right }}
        className="fixed z-[80] w-56 rounded-2xl border border-gray-100 bg-white p-2 shadow-2xl"
      >
        <button role="menuitem" type="button" onClick={() => run(onShowForm)} className="flex w-full items-center gap-3 rounded-xl px-3 py-2 text-left text-sm font-semibold text-gray-700 hover:bg-indigo-50 hover:text-indigo-700"><span aria-hidden="true">＋</span><span>添加记录</span><kbd className="ml-auto text-[10px] font-normal text-gray-400">Ctrl+N</kbd></button>
        <button role="menuitem" type="button" onClick={() => run(() => onViewModeChange(viewMode === 'list' ? 'poster' : 'list'))} className="flex w-full items-center gap-3 rounded-xl px-3 py-2 text-left text-sm font-semibold text-gray-700 hover:bg-gray-50"><span aria-hidden="true">{viewMode === 'list' ? '▦' : '☷'}</span><span>{viewMode === 'list' ? '切换至海报墙' : '切换至列表'}</span></button>
        <button role="menuitem" type="button" onClick={() => run(onShowCollections)} className="flex w-full items-center gap-3 rounded-xl px-3 py-2 text-left text-sm font-semibold text-gray-700 hover:bg-gray-50"><span aria-hidden="true">🎞️</span><span>系列与收藏集</span></button>
        <button role="menuitem" type="button" onClick={() => run(onShowDashboard)} className="flex w-full items-center gap-3 rounded-xl px-3 py-2 text-left text-sm font-semibold text-gray-700 hover:bg-gray-50"><span aria-hidden="true">📈</span><span>数据看板</span></button>
      </div>,
      document.body,
    )}
  </>;
}
