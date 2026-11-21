import { compareViewingDatesDesc } from '../../shared/lib/viewingDate.ts';
import { MEDIA_TYPE_VALUES, STATUS_VALUES, type MediaType, type Status, type WatchRecord } from '../../shared/types/watchRecord.generated.ts';
import { displayTitlesOf } from '../../shared/lib/displayTitle.ts';
import { mediaTypeOf } from '../../shared/lib/classification.ts';
import { filterRecordsByQuery, normalizeWatchlistQuery, type SortBy, type WatchlistQueryV1 } from '../../shared/lib/watchlistQuery.ts';
import { calculateWatchValue } from '../../shared/lib/analytics.ts';

export const MOBILE_LIBRARY_PREFERENCES_KEY = 'mobile_library_preferences_v1';
export type MobileSortBy = Extract<SortBy, 'createdAt' | 'endDate' | 'releaseYear' | 'rating' | 'watchValue'> | 'startDate';
export interface MobileLibraryPreferences {
  version: 1;
  sortBy: MobileSortBy;
  mediaTypes: MediaType[];
  statuses: Status[];
  lock: 'all' | 'locked' | 'unlocked';
}

export const DEFAULT_MOBILE_LIBRARY_PREFERENCES: MobileLibraryPreferences = {
  version: 1, sortBy: 'createdAt', mediaTypes: [], statuses: [], lock: 'all',
};

let cachedPreferences: MobileLibraryPreferences | null = null;
let cachedViewMode: 'list' | 'poster' | null = null;

export function getCachedMobilePreferences(): MobileLibraryPreferences | null {
  return cachedPreferences;
}

export function setCachedMobilePreferences(prefs: MobileLibraryPreferences | null): void {
  cachedPreferences = prefs;
}

export function getCachedMobileViewMode(): 'list' | 'poster' | null {
  return cachedViewMode;
}

export function setCachedMobileViewMode(mode: 'list' | 'poster' | null): void {
  cachedViewMode = mode;
}

export function resetCachedMobilePreferencesForTesting(): void {
  cachedPreferences = null;
  cachedViewMode = null;
}

export function normalizeMobilePreferences(value: unknown): MobileLibraryPreferences {
  const candidate = value && typeof value === 'object' ? value as Partial<MobileLibraryPreferences> : {};
  if (candidate.version !== 1) return { ...DEFAULT_MOBILE_LIBRARY_PREFERENCES };
  const sortBy: MobileSortBy = ['createdAt', 'endDate', 'startDate', 'releaseYear', 'rating', 'watchValue'].includes(candidate.sortBy ?? '')
    ? candidate.sortBy as MobileSortBy : 'createdAt';
  const mediaTypes = Array.isArray(candidate.mediaTypes) ? candidate.mediaTypes.filter((v): v is MediaType => MEDIA_TYPE_VALUES.includes(v as MediaType)) : [];
  const statuses = Array.isArray(candidate.statuses) ? candidate.statuses.filter((v): v is Status => STATUS_VALUES.includes(v as Status)) : [];
  const lock = candidate.lock === 'locked' || candidate.lock === 'unlocked' ? candidate.lock : 'all';
  return { version: 1, sortBy, mediaTypes: [...new Set(mediaTypes)], statuses: [...new Set(statuses)], lock };
}

export function mobileQueryFromPreferences(prefs: MobileLibraryPreferences): WatchlistQueryV1 {
  return normalizeWatchlistQuery({ schemaVersion: 1, searchText: '', mediaTypes: prefs.mediaTypes, statuses: prefs.statuses, lock: prefs.lock });
}

function compareNullableDesc(left: number | string | null | undefined, right: number | string | null | undefined): number {
  const leftMissing = left === null || left === undefined || left === '';
  const rightMissing = right === null || right === undefined || right === '';
  if (leftMissing || rightMissing) return leftMissing === rightMissing ? 0 : leftMissing ? 1 : -1;
  if (typeof left === 'number' && typeof right === 'number') return right - left;
  return String(right).localeCompare(String(left), undefined, { numeric: true });
}

export function sortMobileRecords(records: readonly WatchRecord[], sortBy: MobileSortBy, allRecords: readonly WatchRecord[] = records): WatchRecord[] {
  return records.map((record, index) => ({ record, index })).sort((a, b) => {
    let result: number;
    if (sortBy === 'endDate') result = compareViewingDatesDesc(a.record.endDate, b.record.endDate);
    else if (sortBy === 'startDate') result = compareViewingDatesDesc(a.record.startDate, b.record.startDate);
    else if (sortBy === 'releaseYear') result = compareNullableDesc(a.record.releaseYear, b.record.releaseYear);
    else if (sortBy === 'rating') result = compareNullableDesc(a.record.rating, b.record.rating);
    else if (sortBy === 'watchValue') {
      const valA = calculateWatchValue(a.record, allRecords as WatchRecord[]);
      const valB = calculateWatchValue(b.record, allRecords as WatchRecord[]);
      result = valB - valA;
    }
    else result = compareNullableDesc(a.record.createdAt, b.record.createdAt);
    if (result !== 0) return result;
    return (b.record.createdAt || '').localeCompare(a.record.createdAt || '') || a.record.id.localeCompare(b.record.id) || a.index - b.index;
  }).map(item => item.record);
}

export function applyMobileLibraryQuery(records: readonly WatchRecord[], searchText: string, prefs: MobileLibraryPreferences): WatchRecord[] {
  const query = mobileQueryFromPreferences(prefs);
  query.searchText = searchText;
  return sortMobileRecords(filterRecordsByQuery(records, query), prefs.sortBy, records);
}

export function recordSearchText(record: WatchRecord): string {
  const titles = displayTitlesOf(record);
  return [titles.primary, titles.secondary, record.originalName, record.chineseName, record.platform, record.notes].filter(Boolean).join(' ');
}

export function mobileRecordMediaType(record: WatchRecord): MediaType { return mediaTypeOf(record); }

export function canonicalRecordForm(value: Partial<WatchRecord>): string {
  const keys = ['originalName', 'chineseName', 'progress', 'totalEpisodes', 'movieProgress', 'movieDuration', 'releaseYear', 'posterPath', 'status', 'platform', 'rating', 'startDate', 'endDate', 'notes', 'imdbId', 'genres', 'originCountry', 'imdbRating', 'tmdbStatus', 'interestLevel', 'episodeRuntime', 'mediaType', 'contentTags', 'tmdbMediaKind', 'tmdbId', 'tmdbParentId', 'tmdbSeasonNumber', 'seriesRecordKind'] as const;
  return JSON.stringify(Object.fromEntries(keys.map(key => [key, value[key] ?? null])));
}

export function isRecordFormDirty(initial: Partial<WatchRecord>, current: Partial<WatchRecord>): boolean {
  return canonicalRecordForm(initial) !== canonicalRecordForm(current);
}
