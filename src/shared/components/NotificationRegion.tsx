import { useCallback, useEffect, useRef, useState } from 'react';
import type { NoticeInput, NoticeTone } from '../lib/feedback';

interface Notice extends NoticeInput {
  id: number;
}

const toneStyles: Record<NoticeTone, string> = {
  success: 'border-emerald-200 bg-emerald-50 text-emerald-800',
  error: 'border-red-200 bg-red-50 text-red-800',
  warning: 'border-amber-200 bg-amber-50 text-amber-900',
  info: 'border-indigo-200 bg-indigo-50 text-indigo-800',
};

// This hook is colocated with its single presentation component so the notice
// state contract cannot drift from the rendered notification region.
// eslint-disable-next-line react-refresh/only-export-components
export function useNotifications() {
  const [notices, setNotices] = useState<Notice[]>([]);
  const nextId = useRef(1);
  const timers = useRef(new Map<number, ReturnType<typeof setTimeout>>());

  const dismiss = useCallback((id: number) => {
    const timer = timers.current.get(id);
    if (timer) clearTimeout(timer);
    timers.current.delete(id);
    setNotices(current => current.filter(notice => notice.id !== id));
  }, []);

  const notify = useCallback((tone: NoticeTone, message: string) => {
    const id = nextId.current++;
    setNotices(current => [...current, { id, tone, message }]);
    const timer = setTimeout(() => {
      timers.current.delete(id);
      setNotices(current => current.filter(notice => notice.id !== id));
    }, tone === 'error' ? 6000 : 4000);
    timers.current.set(id, timer);
  }, []);

  useEffect(() => () => {
    for (const timer of timers.current.values()) clearTimeout(timer);
    timers.current.clear();
  }, []);

  return { notices, notify, dismiss };
}

export default function NotificationRegion({
  notices,
  onDismiss,
}: {
  notices: Notice[];
  onDismiss: (id: number) => void;
}) {
  return (
    <div
      aria-live="polite"
      aria-relevant="additions"
      className="fixed right-4 top-4 z-[100] flex w-[min(24rem,calc(100vw-2rem))] flex-col gap-2"
    >
      {notices.map(notice => (
        <div
          key={notice.id}
          role={notice.tone === 'error' ? 'alert' : 'status'}
          className={`flex items-start gap-3 rounded-2xl border px-4 py-3 text-sm font-medium shadow-lg ${toneStyles[notice.tone]}`}
        >
          <span className="min-w-0 flex-1">{notice.message}</span>
          <button
            type="button"
            aria-label="关闭通知"
            onClick={() => onDismiss(notice.id)}
            className="rounded px-1 font-bold opacity-60 hover:opacity-100 focus:outline-none focus:ring-2 focus:ring-current"
          >
            ×
          </button>
        </div>
      ))}
    </div>
  );
}
