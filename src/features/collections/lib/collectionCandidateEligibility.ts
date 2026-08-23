import type { TmdbMedia } from '../../../shared/lib/classification.ts';

export interface CollectionMatchSeed {
  id: number;
  mediaType: 'movie' | 'tv';
  label: string;
  originalLabel: string | null;
  year: string | null;
  posterPath: string | null;
}

export type MovieCandidateEligibility = 'actionable' | 'complete' | 'unknown';

function validTmdbId(value: number | null | undefined): value is number {
  return Number.isInteger(value) && (value ?? 0) > 0;
}

function yearOf(item: TmdbMedia): string | null {
  const value = item.release_date || item.first_air_date;
  return /^\d{4}/.test(value ?? '') ? value!.slice(0, 4) : null;
}

export function normalizeCollectionSearchMatches(results: TmdbMedia[] | null | undefined): CollectionMatchSeed[] {
  const matches: CollectionMatchSeed[] = [];
  for (const item of results ?? []) {
    let seed: CollectionMatchSeed | null = null;
    if (item.media_type === 'movie' && validTmdbId(item.id)) {
      seed = {
        id: item.id,
        mediaType: 'movie',
        label: item.title || item.original_title || `TMDB ${item.id}`,
        originalLabel: item.original_title && item.original_title !== item.title ? item.original_title : null,
        year: yearOf(item),
        posterPath: item.poster_path ?? null,
      };
    } else if (item.media_type === 'tv' && validTmdbId(item.id)) {
      seed = {
        id: item.id,
        mediaType: 'tv',
        label: item.name || item.original_name || `TMDB ${item.id}`,
        originalLabel: item.original_name && item.original_name !== item.name ? item.original_name : null,
        year: yearOf(item),
        posterPath: item.poster_path ?? null,
      };
    } else if (item.media_type === 'tv_season' && validTmdbId(item.show_id)) {
      seed = {
        id: item.show_id,
        mediaType: 'tv',
        label: item.name || `TMDB ${item.show_id}`,
        originalLabel: null,
        year: yearOf(item),
        posterPath: item.poster_path ?? null,
      };
    }
    if (seed && !matches.some(other => other.id === seed.id && other.mediaType === seed.mediaType)) matches.push(seed);
  }
  return matches;
}

function isReleased(date: string | null | undefined, now: Date): boolean {
  if (!date) return false;
  const parsed = new Date(`${date}T00:00:00Z`);
  return !Number.isNaN(parsed.valueOf()) && parsed <= now;
}

export function movieCollectionCandidateEligibility(
  collectionDetail: TmdbMedia | null | undefined,
  now = new Date(),
): MovieCandidateEligibility {
  if (!Array.isArray(collectionDetail?.parts)) return 'unknown';
  const releasedIds = [...new Set(collectionDetail.parts
    .filter(part => validTmdbId(part.id) && isReleased(part.release_date, now))
    .map(part => part.id as number))];

  // A collection with only one currently released work cannot produce a useful grouping or completion action.
  if (releasedIds.length < 2) return 'complete';

  // Library coverage is evaluated before this function. Two released parts remain actionable
  // even when both records already exist locally, because they may still need to be grouped.
  return 'actionable';
}

export function collectionCandidateDescription(seed: CollectionMatchSeed, sourceName: string, missingCount?: number): string {
  const identity = [seed.originalLabel, seed.year, `TMDB ${seed.id}`].filter(Boolean).join(' · ');
  const target = seed.mediaType === 'movie' ? `电影合集：${sourceName}` : `电视剧系列：${sourceName}`;
  const difference = typeof missingCount === 'number' && missingCount > 0 ? ` · 至少缺少 ${missingCount} 部` : '';
  return `${identity || `TMDB ${seed.id}`} · ${target}${difference}`;
}
