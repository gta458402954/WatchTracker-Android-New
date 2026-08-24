import type { WatchRecord } from '../../../shared/types/index.ts';
import {
  classifyTmdb,
  inferPlatformFromTmdb,
  mergeContentTags,
  positiveEpisodeRuntimeOf,
  type TmdbMedia,
  type TmdbSeason,
} from '../../../shared/lib/classification.ts';
import { formatSeasonTitles } from '../../../shared/lib/seasonTitles.ts';

function missingText(value: string | null | undefined): boolean {
  return !value?.trim();
}

export function tmdbOriginalLanguageLocale(originalLanguage?: string | null): string {
  const locales: Record<string, string> = {
    de: 'de-DE', en: 'en-US', es: 'es-ES', fr: 'fr-FR', hi: 'hi-IN', it: 'it-IT',
    ja: 'ja-JP', ko: 'ko-KR', pt: 'pt-BR', ru: 'ru-RU', th: 'th-TH', zh: 'zh-CN',
  };
  return locales[originalLanguage?.trim().toLocaleLowerCase() ?? ''] ?? 'en-US';
}

export function seasonRecordMetadata(
  series: TmdbMedia,
  season: TmdbSeason,
  existing?: Partial<WatchRecord>,
  originalSeason?: TmdbSeason,
): Partial<WatchRecord> {
  const seriesName = series.name || series.title || existing?.chineseName || '';
  const originalName = series.original_name || series.original_title || seriesName;
  const classification = classifyTmdb(series, true, existing?.mediaType);
  const platform = inferPlatformFromTmdb(
    classification.originCountry,
    series.networks?.[0]?.name || series.production_companies?.[0]?.name,
  );
  const runtime = positiveEpisodeRuntimeOf(series);
  const seasonNumber = season.season_number ?? originalSeason?.season_number ?? 0;
  const titles = formatSeasonTitles({
    seriesName,
    originalName,
    seasonNumber,
    localizedSeasonName: season.name,
    originalSeasonName: originalSeason?.name,
  });
  const next: Partial<WatchRecord> = {
    chineseName: titles.chineseName,
    originalName: titles.originalName,
    releaseYear: season.air_date?.slice(0, 4) || null,
    posterPath: season.poster_path || series.poster_path || null,
    totalEpisodes: season.episode_count || null,
    imdbId: series.external_ids?.imdb_id || series.imdb_id || existing?.imdbId || null,
    imdbRating: series.vote_average || null,
    tmdbStatus: series.status || null,
    mediaType: classification.mediaType,
    tmdbMediaKind: 'tv-season',
    tmdbId: season.id ?? null,
    tmdbParentId: series.id ?? null,
    tmdbSeasonNumber: seasonNumber || null,
    seriesRecordKind: 'season',
  };
  if (missingText(existing?.genres) && classification.genres) next.genres = classification.genres;
  if (missingText(existing?.originCountry) && classification.originCountry) next.originCountry = classification.originCountry;
  if (classification.contentTags) next.contentTags = mergeContentTags(existing?.contentTags, classification.contentTags);
  if (missingText(existing?.platform) && platform) next.platform = platform;
  if (existing?.episodeRuntime == null && runtime !== null) next.episodeRuntime = runtime;
  return next;
}

export function movieRecordMetadata(movie: TmdbMedia): Partial<WatchRecord> {
  const classification = classifyTmdb(movie, false, '电影');
  const platform = inferPlatformFromTmdb(
    classification.originCountry,
    movie.production_companies?.[0]?.name,
  );
  return {
    chineseName: movie.title || movie.name || '',
    originalName: movie.original_title || movie.original_name || movie.title || movie.name || '',
    releaseYear: movie.release_date?.slice(0, 4) || null,
    posterPath: movie.poster_path || null,
    imdbId: movie.external_ids?.imdb_id || movie.imdb_id || null,
    imdbRating: movie.vote_average || null,
    tmdbStatus: movie.status || null,
    mediaType: classification.mediaType,
    genres: classification.genres,
    originCountry: classification.originCountry,
    contentTags: classification.contentTags,
    platform: platform || '',
    movieDuration: movie.runtime ? movie.runtime * 60 : null,
    tmdbMediaKind: 'movie',
    tmdbId: movie.id ?? null,
    tmdbParentId: null,
    tmdbSeasonNumber: null,
    seriesRecordKind: 'single-work',
  };
}
