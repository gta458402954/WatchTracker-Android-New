import { useCallback, useEffect, useState } from 'react';
import { getSettingAsync, setSettingAsync } from '../../../shared/lib/database';
import {
  MAX_SAVED_VIEWS,
  SAVED_VIEWS_SETTING_KEY,
  STARTUP_VIEW_SETTING_KEY,
  parseSavedViews,
  serializeSavedViews,
  validateSavedViewName,
  type SavedWatchlistViewV1,
} from '../../../shared/lib/savedViews';
import type { SortBy, ViewMode, WatchlistQueryV1 } from '../../../shared/lib/watchlistQuery';

interface ViewSnapshot {
  query: WatchlistQueryV1;
  sortBy: SortBy;
  viewMode: ViewMode;
}

export function useSavedWatchlistViews(enabled: boolean, onError: (message: string) => void) {
  const [views, setViews] = useState<SavedWatchlistViewV1[]>([]);
  const [startupViewId, setStartupViewIdState] = useState<string | null>(null);
  const [loaded, setLoaded] = useState(false);

  useEffect(() => {
    if (!enabled) return;
    let cancelled = false;
    Promise.all([getSettingAsync(SAVED_VIEWS_SETTING_KEY), getSettingAsync(STARTUP_VIEW_SETTING_KEY)])
      .then(([rawViews, rawStartup]) => {
        if (cancelled) return;
        const parsed = parseSavedViews(rawViews);
        setViews(parsed);
        setStartupViewIdState(parsed.some(view => view.id === rawStartup) ? rawStartup : null);
      })
      .catch(() => {
        if (!cancelled) onError('保存视图读取失败，已安全回退到全部记录。');
      })
      .finally(() => {
        if (!cancelled) setLoaded(true);
      });
    return () => { cancelled = true; };
  }, [enabled, onError]);

  const persist = useCallback(async (next: SavedWatchlistViewV1[]) => {
    await setSettingAsync(SAVED_VIEWS_SETTING_KEY, serializeSavedViews(next));
    setViews(next);
  }, []);

  const createView = useCallback(async (name: string, snapshot: ViewSnapshot) => {
    const error = validateSavedViewName(name, views);
    if (error) throw new Error(error);
    if (views.length >= MAX_SAVED_VIEWS) throw new Error(`最多保存 ${MAX_SAVED_VIEWS} 个视图。`);
    const now = new Date().toISOString();
    const view: SavedWatchlistViewV1 = {
      id: crypto.randomUUID(), name: name.trim(), ...snapshot, createdAt: now, updatedAt: now,
    };
    await persist([...views, view]);
    return view;
  }, [persist, views]);

  const updateView = useCallback(async (id: string, snapshot: ViewSnapshot) => {
    const current = views.find(view => view.id === id);
    if (!current) throw new Error('保存视图已不存在。');
    const updated = { ...current, ...snapshot, updatedAt: new Date().toISOString() };
    await persist(views.map(view => view.id === id ? updated : view));
    return updated;
  }, [persist, views]);

  const deleteView = useCallback(async (id: string) => {
    const next = views.filter(view => view.id !== id);
    await persist(next);
    if (startupViewId === id) {
      await setSettingAsync(STARTUP_VIEW_SETTING_KEY, '');
      setStartupViewIdState(null);
    }
  }, [persist, startupViewId, views]);

  const setStartupViewId = useCallback(async (id: string | null) => {
    if (id && !views.some(view => view.id === id)) throw new Error('无法将不存在的视图设为启动视图。');
    await setSettingAsync(STARTUP_VIEW_SETTING_KEY, id ?? '');
    setStartupViewIdState(id);
  }, [views]);

  return { views, startupViewId, loaded, createView, updateView, deleteView, setStartupViewId };
}
