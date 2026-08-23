import type { MediaType, Status, WatchRecord } from '../types';
import { MEDIA_TYPES } from './classification.ts';

const VALID_STATUSES: readonly Status[] = ['在看', '未看', '已看'];

export function normalizeImportedRecord(
  value: unknown,
  index: number,
  now = new Date(),
): WatchRecord {
  if (!value || typeof value !== 'object' || Array.isArray(value)) {
    throw new Error(`第 ${index + 1} 条记录格式无效`);
  }

  const source = value as Record<string, unknown>;
  const text = (key: string, fallback = '') => typeof source[key] === 'string'
    ? source[key] as string
    : fallback;
  const nullableText = (key: string) => typeof source[key] === 'string' && source[key] !== ''
    ? source[key] as string
    : null;
  const nullableNumber = (key: string) => typeof source[key] === 'number' && Number.isFinite(source[key])
    ? source[key] as number
    : null;
  const requestedType = nullableText('mediaType');
  const mediaType = requestedType && MEDIA_TYPES.includes(requestedType as MediaType)
    ? requestedType as MediaType
    : '电影';
  const requestedStatus = text('status', '已看');
  const status = VALID_STATUSES.includes(requestedStatus as Status)
    ? requestedStatus as Status
    : '已看';
  const revision = nullableNumber('rev');

  return {
    id: nullableText('id') ?? `imported-${now.getTime()}-${index}`,
    originalName: text('originalName'),
    chineseName: text('chineseName'),
    progress: text('progress'),
    totalEpisodes: nullableNumber('totalEpisodes'),
    episodeTrackingEnabled: source.episodeTrackingEnabled === true,
    nextEpisode: nullableNumber('nextEpisode'),
    movieProgress: nullableNumber('movieProgress'),
    movieDuration: nullableNumber('movieDuration'),
    releaseYear: source.releaseYear == null ? null : String(source.releaseYear),
    posterPath: nullableText('posterPath'),
    status,
    platform: text('platform'),
    rating: nullableNumber('rating'),
    startDate: text('startDate'),
    endDate: text('endDate'),
    notes: text('notes'),
    createdAt: nullableText('createdAt') ?? now.toISOString(),
    updatedAt: nullableText('updatedAt'),
    imdbId: nullableText('imdbId'),
    isLocked: source.isLocked === true,
    genres: nullableText('genres'),
    originCountry: nullableText('originCountry'),
    imdbRating: nullableNumber('imdbRating'),
    tmdbStatus: nullableText('tmdbStatus'),
    interestLevel: nullableNumber('interestLevel'),
    episodeRuntime: nullableNumber('episodeRuntime'),
    mediaType,
    contentTags: nullableText('contentTags'),
    tmdbMediaKind: ['movie', 'tv', 'tv-season'].includes(text('tmdbMediaKind')) ? text('tmdbMediaKind') as WatchRecord['tmdbMediaKind'] : null,
    tmdbId: nullableNumber('tmdbId'),
    tmdbParentId: nullableNumber('tmdbParentId'),
    tmdbSeasonNumber: nullableNumber('tmdbSeasonNumber'),
    seriesRecordKind: ['season', 'whole-series', 'single-work'].includes(text('seriesRecordKind')) ? text('seriesRecordKind') as WatchRecord['seriesRecordKind'] : null,
    rev: revision !== null && Number.isInteger(revision) && revision >= 0 ? revision : 0,
    revActor: nullableText('revActor') ?? '',
  };
}

export function normalizeImportedRecords(value: unknown, now = new Date()): WatchRecord[] {
  if (!Array.isArray(value)) throw new Error('无效的 JSON 格式');
  return value.map((record, index) => normalizeImportedRecord(record, index, now));
}
