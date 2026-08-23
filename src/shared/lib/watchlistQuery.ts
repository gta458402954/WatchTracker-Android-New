import { MEDIA_TYPES, aggregateRegions, contentTagsOf, mediaTypeOf, regionCodesForTopFilter } from './classification.ts';
import { UNKNOWN_REGION_CODE, countryLabelOf, isCountryLabel, type CountryCode } from './countryNames.ts';
import type { MediaType, Status, WatchRecord } from '../types/index.ts';

export const WATCH_STATUSES: readonly Status[] = ['在看', '未看', '已看'];
export const SORT_OPTIONS = ['createdAt', 'endDate', 'rating', 'releaseYear', 'watchValue'] as const;
export type SortBy = (typeof SORT_OPTIONS)[number];
export type ViewMode = 'list' | 'poster';
export type LockFilter = 'all' | 'locked' | 'unlocked';

export interface NumberRange {
  min: number | null;
  max: number | null;
}

export interface WatchlistQueryV1 {
  schemaVersion: 1;
  searchText: string;
  mediaTypes: MediaType[];
  statuses: Status[];
  regions: CountryCode[];
  platforms: string[];
  genres: string[];
  contentTags: string[];
  lock: LockFilter;
  releaseYear: NumberRange;
  rating: NumberRange;
  imdbRating: NumberRange;
}

export interface FilterOption {
  value: string;
  label: string;
  count: number;
}

export interface WatchlistFilterOptions {
  mediaTypes: FilterOption[];
  statuses: FilterOption[];
  regions: FilterOption[];
  platforms: FilterOption[];
  genres: FilterOption[];
  contentTags: FilterOption[];
}

export type QueryDimension = Exclude<keyof WatchlistQueryV1, 'schemaVersion'>;
export interface QuerySummaryItem { dimension: QueryDimension; label: string }

export const ADVANCED_QUERY_DIMENSIONS = [
  'platforms', 'genres', 'contentTags', 'releaseYear', 'rating', 'imdbRating',
] as const satisfies readonly QueryDimension[];
export type AdvancedQueryDimension = (typeof ADVANCED_QUERY_DIMENSIONS)[number];

export const EMPTY_WATCHLIST_QUERY: WatchlistQueryV1 = Object.freeze({
  schemaVersion: 1,
  searchText: '',
  mediaTypes: [],
  statuses: [],
  regions: [],
  platforms: [],
  genres: [],
  contentTags: [],
  lock: 'all',
  releaseYear: { min: null, max: null },
  rating: { min: null, max: null },
  imdbRating: { min: null, max: null },
});

function normalizedText(value: string): string {
  return value.trim().normalize('NFKC').toLocaleLowerCase();
}

function uniqueText(values: readonly unknown[]): string[] {
  const seen = new Set<string>();
  const result: string[] = [];
  for (const value of values) {
    if (typeof value !== 'string') continue;
    const trimmed = value.trim().normalize('NFKC');
    const key = normalizedText(trimmed);
    if (!key || seen.has(key)) continue;
    seen.add(key);
    result.push(trimmed);
  }
  return result.sort((left, right) => left.localeCompare(right, 'zh-CN'));
}

function normalizeRange(value: unknown, lower: number, upper: number): NumberRange {
  const candidate = value && typeof value === 'object' ? value as Partial<NumberRange> : {};
  const valid = (item: unknown) => typeof item === 'number' && Number.isFinite(item)
    ? Math.min(upper, Math.max(lower, item))
    : null;
  let min = valid(candidate.min);
  let max = valid(candidate.max);
  if (min !== null && max !== null && min > max) [min, max] = [max, min];
  return { min, max };
}

export function normalizeWatchlistQuery(value: unknown): WatchlistQueryV1 {
  const candidate = value && typeof value === 'object' ? value as Partial<WatchlistQueryV1> : {};
  // Preserve unknown future enum values as non-matching conditions. Dropping the
  // only selected value would silently broaden a saved view to every record.
  const mediaTypes = uniqueText(Array.isArray(candidate.mediaTypes) ? candidate.mediaTypes : []) as MediaType[];
  const statuses = uniqueText(Array.isArray(candidate.statuses) ? candidate.statuses : []) as Status[];
  const regions = uniqueText(Array.isArray(candidate.regions) ? candidate.regions : [])
    .map(item => item === UNKNOWN_REGION_CODE ? item : item.toUpperCase())
    .filter(item => item === UNKNOWN_REGION_CODE || /^[A-Z]{2}$/.test(item));
  const lock: LockFilter = ['all', 'locked', 'unlocked'].includes(candidate.lock ?? '')
    ? candidate.lock as LockFilter
    : 'all';

  return {
    schemaVersion: 1,
    searchText: typeof candidate.searchText === 'string' ? candidate.searchText.trim() : '',
    mediaTypes,
    statuses,
    regions,
    platforms: uniqueText(Array.isArray(candidate.platforms) ? candidate.platforms : []),
    genres: uniqueText(Array.isArray(candidate.genres) ? candidate.genres : []),
    contentTags: uniqueText(Array.isArray(candidate.contentTags) ? candidate.contentTags : []),
    lock,
    releaseYear: normalizeRange(candidate.releaseYear, 1800, 2200),
    rating: normalizeRange(candidate.rating, 0, 10),
    imdbRating: normalizeRange(candidate.imdbRating, 0, 10),
  };
}

export function watchlistQueriesEqual(left: WatchlistQueryV1, right: WatchlistQueryV1): boolean {
  return JSON.stringify(normalizeWatchlistQuery(left)) === JSON.stringify(normalizeWatchlistQuery(right));
}

export function activeQueryDimensionCount(query: WatchlistQueryV1): number {
  const normalized = normalizeWatchlistQuery(query);
  return [
    normalized.searchText.trim(), normalized.mediaTypes.length, normalized.statuses.length,
    normalized.regions.length, normalized.platforms.length, normalized.genres.length,
    normalized.contentTags.length, normalized.lock !== 'all',
    normalized.releaseYear.min !== null || normalized.releaseYear.max !== null,
    normalized.rating.min !== null || normalized.rating.max !== null,
    normalized.imdbRating.min !== null || normalized.imdbRating.max !== null,
  ].filter(Boolean).length;
}

export function activeAdvancedQueryDimensionCount(query: WatchlistQueryV1): number {
  const normalized = normalizeWatchlistQuery(query);
  return [
    normalized.platforms.length,
    normalized.genres.length,
    normalized.contentTags.length,
    normalized.releaseYear.min !== null || normalized.releaseYear.max !== null,
    normalized.rating.min !== null || normalized.rating.max !== null,
    normalized.imdbRating.min !== null || normalized.imdbRating.max !== null,
  ].filter(Boolean).length;
}

export function clearAdvancedQuery(query: WatchlistQueryV1): WatchlistQueryV1 {
  const empty = EMPTY_WATCHLIST_QUERY;
  return normalizeWatchlistQuery({
    ...query,
    platforms: empty.platforms,
    genres: empty.genres,
    contentTags: empty.contentTags,
    releaseYear: empty.releaseYear,
    rating: empty.rating,
    imdbRating: empty.imdbRating,
  });
}

function rangeLabel(label: string, range: NumberRange): string | null {
  if (range.min !== null && range.max !== null) return `${label} ${range.min}–${range.max}`;
  if (range.min !== null) return `${label} ≥ ${range.min}`;
  if (range.max !== null) return `${label} ≤ ${range.max}`;
  return null;
}

export function querySummaryItems(query: WatchlistQueryV1): QuerySummaryItem[] {
  const value = normalizeWatchlistQuery(query);
  const items: Array<QuerySummaryItem | null> = [
    value.searchText.trim() ? { dimension: 'searchText', label: `搜索：${value.searchText.trim()}` } : null,
    value.mediaTypes.length ? { dimension: 'mediaTypes', label: value.mediaTypes.join(' 或 ') } : null,
    value.statuses.length ? { dimension: 'statuses', label: value.statuses.join(' 或 ') } : null,
    value.regions.length ? { dimension: 'regions', label: `地区：${value.regions.map(countryLabelOf).join('、')}` } : null,
    value.platforms.length ? { dimension: 'platforms', label: `平台：${value.platforms.join('、')}` } : null,
    value.genres.length ? { dimension: 'genres', label: `题材：${value.genres.join('、')}` } : null,
    value.contentTags.length ? { dimension: 'contentTags', label: `标签：${value.contentTags.join('、')}` } : null,
    value.lock !== 'all' ? { dimension: 'lock', label: value.lock === 'locked' ? '已锁定' : '未锁定' } : null,
    rangeLabel('上映年份', value.releaseYear) ? { dimension: 'releaseYear', label: rangeLabel('上映年份', value.releaseYear)! } : null,
    rangeLabel('个人评分', value.rating) ? { dimension: 'rating', label: rangeLabel('个人评分', value.rating)! } : null,
    rangeLabel('IMDb 评分', value.imdbRating) ? { dimension: 'imdbRating', label: rangeLabel('IMDb 评分', value.imdbRating)! } : null,
  ];
  return items.filter((item): item is QuerySummaryItem => item !== null);
}

export function advancedQuerySummaryItems(query: WatchlistQueryV1): QuerySummaryItem[] {
  const advanced = new Set<QueryDimension>(ADVANCED_QUERY_DIMENSIONS);
  return querySummaryItems(query).filter(item => advanced.has(item.dimension));
}

function tags(value: string | null | undefined): string[] {
  return value?.split(/[,，]/).map(item => item.trim()).filter(Boolean) ?? [];
}

function matchesAny(actual: readonly string[], selected: readonly string[]): boolean {
  if (selected.length === 0) return true;
  const keys = new Set(actual.map(normalizedText));
  return selected.some(item => keys.has(normalizedText(item)));
}

function matchesRange(value: number | null, range: NumberRange): boolean {
  if (range.min === null && range.max === null) return true;
  if (value === null || !Number.isFinite(value)) return false;
  return (range.min === null || value >= range.min) && (range.max === null || value <= range.max);
}

export function filterRecordsByQuery(records: readonly WatchRecord[], query: WatchlistQueryV1): WatchRecord[] {
  const filters = normalizeWatchlistQuery(query);
  const search = normalizedText(filters.searchText);
  return records.filter(record => {
    if (filters.mediaTypes.length && !filters.mediaTypes.includes(mediaTypeOf(record))) return false;
    if (filters.statuses.length && !filters.statuses.includes(record.status)) return false;
    if (filters.regions.length && !filters.regions.includes(regionCodesForTopFilter(record)[0])) return false;
    if (!matchesAny(record.platform ? [record.platform] : [], filters.platforms)) return false;
    if (!matchesAny(tags(record.genres), filters.genres)) return false;
    const ordinaryTags = contentTagsOf(record).filter(item => !isCountryLabel(item));
    if (!matchesAny(ordinaryTags, filters.contentTags)) return false;
    if (filters.lock === 'locked' && !record.isLocked) return false;
    if (filters.lock === 'unlocked' && record.isLocked) return false;
    const year = /^\d{4}$/.test(record.releaseYear ?? '') ? Number(record.releaseYear) : null;
    if (!matchesRange(year, filters.releaseYear)) return false;
    if (!matchesRange(record.rating, filters.rating)) return false;
    if (!matchesRange(record.imdbRating ?? null, filters.imdbRating)) return false;
    if (!search) return true;
    return [record.chineseName, record.originalName, record.platform, record.notes]
      .some(value => normalizedText(value).includes(search));
  });
}

function aggregate(values: string[], label: (value: string) => string = value => value): FilterOption[] {
  const entries = new Map<string, { value: string; count: number }>();
  for (const value of values) {
    const trimmed = value.trim().normalize('NFKC');
    const key = normalizedText(trimmed);
    if (!key) continue;
    const current = entries.get(key);
    if (current) current.count += 1;
    else entries.set(key, { value: trimmed, count: 1 });
  }
  return [...entries.values()]
    .map(item => ({ ...item, label: label(item.value) }))
    .sort((left, right) => right.count - left.count || left.label.localeCompare(right.label, 'zh-CN'));
}

export function watchlistFilterOptions(records: readonly WatchRecord[]): WatchlistFilterOptions {
  const content = records.flatMap(record => contentTagsOf(record))
    .filter(item => !isCountryLabel(item));
  return {
    mediaTypes: MEDIA_TYPES.map(value => ({ value, label: value, count: records.filter(record => mediaTypeOf(record) === value).length })),
    statuses: WATCH_STATUSES.map(value => ({ value, label: value, count: records.filter(record => record.status === value).length })),
    regions: aggregateRegions(records).map(option => ({ value: option.code, label: option.label, count: option.count })),
    platforms: aggregate(records.map(record => record.platform).filter(Boolean)),
    genres: aggregate(records.flatMap(record => tags(record.genres))),
    contentTags: aggregate(content),
  };
}
