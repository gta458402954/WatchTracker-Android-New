import type { WatchRecord } from '../../../shared/types/index.ts';
import type { TmdbMedia } from '../../../shared/lib/classification.ts';

export type MovieCollectionMatchStatus = 'member' | 'library' | 'missing' | 'conflict' | 'unresolved';

export interface MovieCollectionCandidate {
  movie: TmdbMedia;
  status: MovieCollectionMatchStatus;
  recordId?: string;
  conflictRecordIds?: string[];
}

export function normalizeImdbId(value: string | null | undefined): string | null {
  const normalized = value?.trim().toLowerCase() ?? '';
  return /^tt\d+$/.test(normalized) ? normalized : null;
}

function hasExplicitNonMovieIdentity(record: WatchRecord): boolean {
  return record.tmdbMediaKind === 'tv' || record.tmdbMediaKind === 'tv-season';
}

export function classifyMovieCollectionPart(
  movie: TmdbMedia,
  records: WatchRecord[],
  memberRecordIds: ReadonlySet<string>,
): MovieCollectionCandidate {
  const tmdbId = Number.isInteger(movie.id) && (movie.id ?? 0) > 0 ? movie.id! : null;
  const imdbId = normalizeImdbId(movie.external_ids?.imdb_id || movie.imdb_id);
  if (tmdbId == null && imdbId == null) return { movie, status: 'unresolved' };

  const tmdbMatches = tmdbId == null
    ? []
    : records.filter(record => record.tmdbMediaKind === 'movie' && record.tmdbId === tmdbId);
  const imdbMatches = imdbId == null
    ? []
    : records.filter(record => normalizeImdbId(record.imdbId) === imdbId);
  const ids = [...new Set([...tmdbMatches, ...imdbMatches].map(record => record.id))];
  if (ids.length > 1) return { movie, status: 'conflict', conflictRecordIds: ids };
  if (ids.length === 1) {
    const matched = records.find(record => record.id === ids[0]);
    if (matched && hasExplicitNonMovieIdentity(matched)) {
      return { movie, status: 'conflict', conflictRecordIds: ids };
    }
    return { movie, status: memberRecordIds.has(ids[0]) ? 'member' : 'library', recordId: ids[0] };
  }
  return { movie, status: 'missing' };
}
