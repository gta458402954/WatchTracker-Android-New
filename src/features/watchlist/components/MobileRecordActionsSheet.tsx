import { useEffect, useRef, type RefObject } from 'react';
import type { WatchRecord } from '../../../shared/types';
import { displayTitlesOf } from '../../../shared/lib/displayTitle';

interface MobileRecordActionsSheetProps {
  record: WatchRecord;
  onEdit: () => void;
  onLock: (locked: boolean) => void;
  onDelete: () => void;
  onClose: () => void;
  returnFocusRef: RefObject<HTMLButtonElement | null>;
}

function titleOf(record: WatchRecord): string {
  const titles = displayTitlesOf(record);
  return titles.primary || titles.secondary || '未命名记录';
}

export default function MobileRecordActionsSheet({
  record,
  onEdit,
  onLock,
  onDelete,
  onClose,
  returnFocusRef,
}: MobileRecordActionsSheetProps) {
  const sheetRef = useRef<HTMLDivElement>(null);
  const title = titleOf(record);

  useEffect(() => {
    const timer = window.requestAnimationFrame(() => {
      const firstAction = sheetRef.current?.querySelector<HTMLElement>('.mobile-sheet-action');
      (firstAction ?? sheetRef.current?.querySelector<HTMLElement>('button, [href], input, select, textarea, [tabindex]:not([tabindex="-1"])'))?.focus();
    });
    return () => window.cancelAnimationFrame(timer);
  }, []);

  useEffect(() => () => { returnFocusRef.current?.focus(); }, [returnFocusRef]);

  useEffect(() => {
    const listener = (event: KeyboardEvent) => {
      if (event.key === 'Escape') {
        event.preventDefault();
        onClose();
        return;
      }
      if (event.key !== 'Tab' || !sheetRef.current) return;
      const focusable = Array.from(sheetRef.current.querySelectorAll<HTMLElement>('button, [href], input, select, textarea, [tabindex]:not([tabindex="-1"])'))
        .filter(item => !item.hasAttribute('disabled'));
      if (!focusable.length) return;
      const first = focusable[0];
      const last = focusable[focusable.length - 1];
      if (event.shiftKey && document.activeElement === first) {
        event.preventDefault();
        last.focus();
      } else if (!event.shiftKey && document.activeElement === last) {
        event.preventDefault();
        first.focus();
      }
    };
    window.addEventListener('keydown', listener);
    return () => window.removeEventListener('keydown', listener);
  }, [onClose]);

  useEffect(() => {
    window.__WATCHTRACKER_ANDROID_BACK_OVERLAY__ = () => {
      onClose();
      return true;
    };
    return () => { delete window.__WATCHTRACKER_ANDROID_BACK_OVERLAY__; };
  }, [onClose]);

  const run = (action: () => void) => {
    onClose();
    action();
  };

  return (
    <div
      className="mobile-sheet-backdrop mobile-record-actions-backdrop"
      role="presentation"
      onMouseDown={event => { if (event.target === event.currentTarget) onClose(); }}
    >
      <div ref={sheetRef} role="dialog" aria-modal="true" aria-labelledby="mobile-record-actions-title" className="mobile-sheet mobile-record-actions-sheet">
        <div className="mobile-sheet-header">
          <h2 id="mobile-record-actions-title">{title}</h2>
          <button type="button" aria-label="关闭更多操作" className="mobile-icon-button" onClick={onClose}>×</button>
        </div>
        <div className="mobile-record-actions-list">
          {record.isLocked ? (
            <button type="button" className="mobile-sheet-action" onClick={() => run(() => onLock(false))}>解锁</button>
          ) : (
            <>
              <button type="button" className="mobile-sheet-action" onClick={() => run(onEdit)}>编辑</button>
              <button type="button" className="mobile-sheet-action" onClick={() => run(() => onLock(true))}>锁定</button>
              <button type="button" className="mobile-sheet-action mobile-sheet-action-danger" onClick={() => run(onDelete)}>删除</button>
            </>
          )}
        </div>
        <button type="button" className="mobile-quiet-button mobile-sheet-cancel" onClick={onClose}>取消</button>
      </div>
    </div>
  );
}
