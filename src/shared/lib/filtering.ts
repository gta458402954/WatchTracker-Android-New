import type { MediaType, Status, WatchRecord } from '../types/index.ts';
import {
  aggregateRegions,
  mediaTypeOf,
  regionCodesForTopFilter,
  type RegionOption,
} from './classification.ts';
import type { RegionFilter } from './countryNames.ts';

export type MediaTypeFilter = MediaType | 'all';
export type StatusFilter = Status | 'all';
export type LockFilter = 'all' | 'locked' | 'unlocked';

export interface RecordFilters {
  mediaType: MediaTypeFilter;
  status: StatusFilter;
  region: RegionFilter;
  searchText: string;
  lock: LockFilter;
}

export function recordsInRegionScope(
  records: readonly WatchRecord[],
  mediaType: MediaTypeFilter,
  status: StatusFilter,
): WatchRecord[] {
  return records.filter(record =>
    (mediaType === 'all' || mediaTypeOf(record) === mediaType)
    && (status === 'all' || record.status === status),
  );
}

export function regionOptionsForScope(
  records: readonly WatchRecord[],
  mediaType: MediaTypeFilter,
  status: StatusFilter,
): RegionOption[] {
  return aggregateRegions(recordsInRegionScope(records, mediaType, status));
}

export function effectiveRegionOf(
  activeRegion: RegionFilter,
  options: readonly RegionOption[],
): RegionFilter {
  return activeRegion === 'all' || options.some(option => option.code === activeRegion)
    ? activeRegion
    : 'all';
}

export function filterRecords(
  records: readonly WatchRecord[],
  filters: RecordFilters,
): WatchRecord[] {
  const query = filters.searchText.trim().toLocaleLowerCase();

  return recordsInRegionScope(records, filters.mediaType, filters.status).filter(record => {
    if (filters.region !== 'all' && !regionCodesForTopFilter(record).includes(filters.region)) return false;
    if (filters.lock === 'locked' && !record.isLocked) return false;
    if (filters.lock === 'unlocked' && record.isLocked) return false;
    if (!query) return true;

    return [record.chineseName, record.originalName, record.platform, record.notes]
      .some(value => value.toLocaleLowerCase().includes(query));
  });
}
