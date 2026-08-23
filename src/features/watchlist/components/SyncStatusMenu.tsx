import { useEffect, useRef, useState } from 'react';
import { createPortal } from 'react-dom';
import type { SyncRuntimeState } from '../../../shared/lib/database';
import { syncPresentation } from '../../../shared/lib/toolbarPresentation';

interface SyncStatusMenuProps {
  hasCredentials: boolean;
  syncing: boolean;
  message: string;
  runtime: SyncRuntimeState | null;
  paused: boolean;
  onSync: () => void;
  onTogglePause: () => void;
  onOpenSettings: () => void;
}

function formatTime(value: string | null | undefined): string {
  if (!value) return '尚无成功记录';
  const date = new Date(value);
  return Number.isNaN(date.getTime()) ? value : date.toLocaleString('zh-CN', { hour12: false });
}

export default function SyncStatusMenu({ hasCredentials, syncing, message, runtime, paused, onSync, onTogglePause, onOpenSettings }: SyncStatusMenuProps) {
  const [open, setOpen] = useState(false);
  const [position, setPosition] = useState({ top: 0, right: 8 });
  const triggerRef = useRef<HTMLButtonElement>(null);
  const panelRef = useRef<HTMLDivElement>(null);
  const presentation = syncPresentation({ hasCredentials, syncing, message, runtime, paused });

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

  const closeAndRun = (action: () => void) => {
    action();
    setOpen(false);
    triggerRef.current?.focus();
  };

  return <>
    <button
      ref={triggerRef}
      type="button"
      aria-label={`云端同步：${presentation.label}`}
      aria-expanded={open}
      aria-controls="sync-status-menu"
      title={presentation.description}
      onClick={() => setOpen(current => !current)}
      className={`flex h-9 shrink-0 items-center gap-2 whitespace-nowrap rounded-xl border px-3 text-xs font-bold transition-colors ${presentation.className}`}
    >
      {syncing ? <svg aria-hidden="true" className="h-4 w-4 animate-spin" fill="none" viewBox="0 0 24 24"><circle className="opacity-25" cx="12" cy="12" r="10" stroke="currentColor" strokeWidth="4" /><path className="opacity-75" fill="currentColor" d="M4 12a8 8 0 018-8v4l3-3-3-3v4a10 10 0 100 10z" /></svg> : <span aria-hidden="true" className="h-2 w-2 rounded-full bg-current" />}
      <span>{presentation.label}</span>
    </button>
    {open && createPortal(
      <div
        id="sync-status-menu"
        ref={panelRef}
        role="dialog"
        aria-label="同步状态"
        style={{ top: position.top, right: position.right }}
        className="fixed z-[80] w-80 max-w-[calc(100vw-1rem)] rounded-2xl border border-gray-100 bg-white p-4 shadow-2xl"
      >
        <div className="flex items-start justify-between gap-3">
          <div><p className="font-bold text-gray-900">云端同步</p><p className="mt-1 text-xs leading-5 text-gray-500">{presentation.description}</p></div>
          <span className={`rounded-full border px-2 py-1 text-[11px] font-bold ${presentation.className}`}>{presentation.label}</span>
        </div>
        {message && <p role="status" className="mt-3 rounded-xl bg-gray-50 px-3 py-2 text-xs text-gray-600">{message}</p>}
        <dl className="mt-4 grid grid-cols-2 gap-2 text-xs">
          <div className="rounded-xl bg-gray-50 p-3"><dt className="text-gray-400">待发布</dt><dd className="mt-1 font-bold text-gray-800">{presentation.pendingCount} 项</dd></div>
          <div className="rounded-xl bg-gray-50 p-3"><dt className="text-gray-400">冲突</dt><dd className="mt-1 font-bold text-gray-800">{runtime?.conflictCount ?? 0} 项</dd></div>
          <div className="col-span-2 rounded-xl bg-gray-50 p-3"><dt className="text-gray-400">最近成功</dt><dd className="mt-1 font-bold text-gray-800">{formatTime(runtime?.scheduler.lastSuccessAt)}</dd></div>
        </dl>
        <div className="mt-4 grid grid-cols-2 gap-2">
          <button type="button" disabled={!hasCredentials || syncing} onClick={() => closeAndRun(onSync)} className="rounded-xl bg-indigo-600 px-3 py-2 text-sm font-bold text-white disabled:cursor-not-allowed disabled:opacity-40">立即同步</button>
          <button type="button" disabled={!hasCredentials} onClick={() => closeAndRun(onTogglePause)} className="rounded-xl border border-gray-200 px-3 py-2 text-sm font-bold text-gray-600 disabled:cursor-not-allowed disabled:opacity-40">{paused ? '恢复自动同步' : '暂停自动同步'}</button>
          <button type="button" onClick={() => closeAndRun(onOpenSettings)} className="col-span-2 rounded-xl border border-gray-200 px-3 py-2 text-sm font-bold text-gray-600 hover:bg-gray-50">{hasCredentials ? '打开同步设置' : '配置 WebDAV'}</button>
        </div>
      </div>,
      document.body,
    )}
  </>;
}
