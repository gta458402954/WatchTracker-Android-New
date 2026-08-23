import { useEffect, useRef, useState } from 'react';
import { createPortal } from 'react-dom';
import type { RegionOption } from '../../../shared/lib/classification';
import type { RegionFilter } from '../../../shared/lib/countryNames';

interface RegionOverflowMenuProps {
  options: RegionOption[];
  activeRegions: RegionFilter[];
  onSelect: (code: RegionFilter) => void;
}

export default function RegionOverflowMenu({ options, activeRegions, onSelect }: RegionOverflowMenuProps) {
  const [open, setOpen] = useState(false);
  const [position, setPosition] = useState({ top: 0, left: 8 });
  const triggerRef = useRef<HTMLButtonElement>(null);
  const panelRef = useRef<HTMLDivElement>(null);

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

  const moveFocus = (event: React.KeyboardEvent<HTMLDivElement>) => {
    if (event.key !== 'ArrowDown' && event.key !== 'ArrowUp' && event.key !== 'ArrowLeft' && event.key !== 'ArrowRight') return;
    const items = Array.from(panelRef.current?.querySelectorAll<HTMLElement>('[role="menuitemcheckbox"]') ?? []);
    if (!items.length) return;
    event.preventDefault();
    const current = items.indexOf(document.activeElement as HTMLElement);
    const forward = event.key === 'ArrowDown' || event.key === 'ArrowRight';
    items[(current + (forward ? 1 : -1) + items.length) % items.length]?.focus();
  };

  return <>
    <button ref={triggerRef} type="button" aria-expanded={open} aria-controls="region-overflow-menu" onClick={() => setOpen(current => !current)} className="shrink-0 whitespace-nowrap rounded-full border border-gray-200 bg-white px-2.5 py-0.5 text-xs font-semibold text-gray-500 hover:border-indigo-200 hover:text-indigo-600">更多地区 {options.length}</button>
    {open && createPortal(
      <div id="region-overflow-menu" ref={panelRef} role="menu" aria-label="更多地区" onKeyDown={moveFocus} style={{ top: position.top, left: position.left }} className="custom-scrollbar fixed z-[80] flex max-h-72 w-72 max-w-[calc(100vw-1rem)] flex-wrap gap-2 overflow-y-auto rounded-2xl border border-gray-100 bg-white p-3 shadow-2xl">
        {options.map(({ code, label, count }) => {
          const selected = activeRegions.includes(code);
          return <button key={code} role="menuitemcheckbox" aria-checked={selected} type="button" onClick={() => { onSelect(code); setOpen(false); triggerRef.current?.focus(); }} className={`whitespace-nowrap rounded-full border px-2.5 py-1 text-xs font-semibold ${selected ? 'border-indigo-200 bg-indigo-50 text-indigo-700' : 'border-gray-200 text-gray-500 hover:border-indigo-200 hover:text-indigo-600'}`}>{label} <b>{count}</b></button>;
        })}
      </div>,
      document.body,
    )}
  </>;
}
