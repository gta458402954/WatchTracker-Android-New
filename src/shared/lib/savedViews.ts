import { normalizeWatchlistQuery, SORT_OPTIONS, type SortBy, type ViewMode, type WatchlistQueryV1 } from './watchlistQuery.ts';

export const SAVED_VIEWS_SETTING_KEY = 'watchlist_saved_views_v1';
export const STARTUP_VIEW_SETTING_KEY = 'watchlist_startup_view_id_v1';
export const MAX_SAVED_VIEWS = 20;

export interface SavedWatchlistViewV1 {
  id: string;
  name: string;
  query: WatchlistQueryV1;
  sortBy: SortBy;
  viewMode: ViewMode;
  createdAt: string;
  updatedAt: string;
}

export interface SavedViewsFileV1 {
  schemaVersion: 1;
  views: readonly SavedWatchlistViewV1[];
}

function validView(value: unknown): SavedWatchlistViewV1 | null {
  if (!value || typeof value !== 'object') return null;
  const candidate = value as Partial<SavedWatchlistViewV1>;
  const name = typeof candidate.name === 'string' ? candidate.name.trim() : '';
  if (typeof candidate.id !== 'string' || !candidate.id || !name || name.length > 30) return null;
  if (!SORT_OPTIONS.includes(candidate.sortBy as SortBy)) return null;
  if (candidate.viewMode !== 'list' && candidate.viewMode !== 'poster') return null;
  if (typeof candidate.createdAt !== 'string' || typeof candidate.updatedAt !== 'string') return null;
  if (!candidate.query || candidate.query.schemaVersion !== 1) return null;
  return { ...candidate as SavedWatchlistViewV1, name, query: normalizeWatchlistQuery(candidate.query) };
}

export function parseSavedViews(raw: string | null): SavedWatchlistViewV1[] {
  if (!raw) return [];
  const parsed: unknown = JSON.parse(raw);
  if (!parsed || typeof parsed !== 'object' || (parsed as Partial<SavedViewsFileV1>).schemaVersion !== 1) {
    throw new Error('unsupported_saved_views_schema');
  }
  const values = (parsed as Partial<SavedViewsFileV1>).views;
  if (!Array.isArray(values)) throw new Error('invalid_saved_views');
  const result: SavedWatchlistViewV1[] = [];
  const ids = new Set<string>();
  const names = new Set<string>();
  for (const value of values) {
    const view = validView(value);
    if (!view) throw new Error('invalid_saved_view');
    const nameKey = view.name.toLocaleLowerCase();
    if (ids.has(view.id) || names.has(nameKey)) throw new Error('duplicate_saved_view');
    ids.add(view.id);
    names.add(nameKey);
    result.push(view);
    if (result.length === MAX_SAVED_VIEWS) break;
  }
  return result;
}

export function serializeSavedViews(views: readonly SavedWatchlistViewV1[]): string {
  return JSON.stringify({ schemaVersion: 1, views } satisfies SavedViewsFileV1);
}

export function validateSavedViewName(name: string, views: readonly SavedWatchlistViewV1[], exceptId?: string): string | null {
  const trimmed = name.trim();
  if (!trimmed) return '请输入视图名称。';
  if (trimmed.length > 30) return '视图名称不能超过 30 个字符。';
  if (views.some(view => view.id !== exceptId && view.name.toLocaleLowerCase() === trimmed.toLocaleLowerCase())) {
    return '已存在同名视图。';
  }
  return null;
}
