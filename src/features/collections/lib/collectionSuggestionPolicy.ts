import type { CollectionMember, WatchRecord } from '../../../shared/types';
import type { TmdbMedia } from '../../../shared/lib/classification';
import { seasonIsAired, seasonNumberOf } from './seriesDiscovery.ts';

export const COLLECTION_SUGGESTION_DISMISSALS_KEY = 'collection_suggestion_dismissals_v1';

export interface CollectionSuggestionDismissal {
  key: string;
  name: string;
  sourceKind: 'manual' | 'tmdb-movie-collection' | 'tmdb-tv-show';
  dismissedAt: string;
}

interface StoredDismissals {
  version: 1;
  entries: CollectionSuggestionDismissal[];
}

export function parseSuggestionDismissals(raw: string | null | undefined): CollectionSuggestionDismissal[] {
  if (!raw) return [];
  try {
    const value = JSON.parse(raw) as Partial<StoredDismissals>;
    if (value.version !== 1 || !Array.isArray(value.entries)) return [];
    return value.entries.filter((entry): entry is CollectionSuggestionDismissal => (
      typeof entry?.key === 'string'
      && entry.key.length > 0
      && typeof entry.name === 'string'
      && ['manual', 'tmdb-movie-collection', 'tmdb-tv-show'].includes(entry.sourceKind ?? '')
      && typeof entry.dismissedAt === 'string'
    ));
  } catch {
    return [];
  }
}

export function serializeSuggestionDismissals(entries: CollectionSuggestionDismissal[]): string {
  const byKey = new Map(entries.map(entry => [entry.key, entry]));
  return JSON.stringify({ version: 1, entries: [...byKey.values()] } satisfies StoredDismissals);
}

export function upsertSuggestionDismissal(
  entries: CollectionSuggestionDismissal[],
  next: CollectionSuggestionDismissal,
): CollectionSuggestionDismissal[] {
  return [...entries.filter(entry => entry.key !== next.key), next];
}

export function suggestionIsCovered(recordIds: string[], members: CollectionMember[]): boolean {
  if (recordIds.length === 0) return true;
  if (recordIds.length === 1) return members.some(member => member.recordId === recordIds[0]);
  const candidateIds = new Set(recordIds);
  const coveredByCollection = new Map<string, Set<string>>();
  for (const member of members) {
    if (!candidateIds.has(member.recordId)) continue;
    const covered = coveredByCollection.get(member.collectionId) ?? new Set<string>();
    covered.add(member.recordId);
    coveredByCollection.set(member.collectionId, covered);
  }
  return [...coveredByCollection.values()].some(covered => covered.size === candidateIds.size);
}

export type TvSuggestionEligibility = 'actionable' | 'complete' | 'unknown';

export function tvSuggestionEligibility(
  detail: TmdbMedia | null | undefined,
  parentId: number,
  candidateRecordIds: string[],
  records: WatchRecord[],
  now = new Date(),
): TvSuggestionEligibility {
  if (!Array.isArray(detail?.seasons)) return 'unknown';
  const airedSeasons = detail.seasons.filter(season => seasonIsAired(season, now));
  if (airedSeasons.length === 0) return 'unknown';

  const candidateIds = new Set(candidateRecordIds);
  const relatedRecords = records.filter(record => record.tmdbParentId === parentId || candidateIds.has(record.id));
  if (relatedRecords.length === 0) return 'unknown';

  // A TMDB-confirmed single aired work is already represented by the matched local record,
  // even when old data did not explicitly label it as season 1.
  if (airedSeasons.length === 1) return 'complete';

  const localSeasons = new Set(relatedRecords
    .map(record => record.tmdbSeasonNumber ?? seasonNumberOf(record))
    .filter((season): season is number => season != null && Number.isInteger(season) && season > 0));
  const allAiredSeasonsExist = airedSeasons.every(season => localSeasons.has(season.season_number ?? -1));

  // Multiple explicit local seasons still benefit from being grouped when no collection covers them.
  if (allAiredSeasonsExist && candidateRecordIds.length <= 1) return 'complete';
  return 'actionable';
}
