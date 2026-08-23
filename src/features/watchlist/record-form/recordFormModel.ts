import type { MediaType, WatchRecord } from '../../../shared/types';
import { formatMovieTime, getEmptyRecord, parseTimeToSeconds } from '../../../shared/lib/constants.ts';
import { mediaTypeOf } from '../../../shared/lib/classification.ts';

export type RecordFormValues = Omit<WatchRecord, 'id' | 'createdAt'>;

export const isAlwaysEpisodic = (mediaType: MediaType | null | undefined) => mediaType === '剧集' || mediaType === '综艺';

export function smartProgress(raw: string): string {
  if (!raw) return '';
  const t = raw.trim();
  if (!t) return '';
  if (['完', 'wan', 'w'].includes(t.toLowerCase())) return '完结';
  if (t === '在看') return '在看';
  if (t === '0') return '完结';
  if (/^S\d+E\d+$/i.test(t)) return t.toUpperCase();
  if (/^S\d+$/i.test(t)) return t.toUpperCase();
  if (/^E\d+$/i.test(t)) return t.toUpperCase();
  if (/^\d+$/.test(t)) return `第${parseInt(t, 10)}集`;
  if (/^第?\d+集?$/.test(t)) {
    const num = t.match(/\d+/)?.[0];
    return num ? `第${num}集` : t;
  }
  return t;
}

/** Copies the exact edit/new defaults used by RecordForm into a testable model. */
export function initialRecordFormValues(record?: WatchRecord | null): RecordFormValues {
  return record
    ? {
        originalName: record.originalName,
        chineseName: record.chineseName,
        progress: record.progress,
        totalEpisodes: record.totalEpisodes,
        movieProgress: record.movieProgress,
        movieDuration: record.movieDuration,
        releaseYear: record.releaseYear,
        posterPath: record.posterPath,
        status: record.status,
        platform: record.platform,
        rating: record.rating,
        startDate: record.startDate,
        endDate: record.endDate,
        notes: record.notes,
        imdbId: record.imdbId || null,
        genres: record.genres || null,
        originCountry: record.originCountry || null,
        imdbRating: record.imdbRating || null,
        tmdbStatus: record.tmdbStatus || null,
        interestLevel: record.interestLevel || null,
        episodeRuntime: record.episodeRuntime || null,
        mediaType: mediaTypeOf(record),
        contentTags: record.contentTags || null,
        tmdbMediaKind: record.tmdbMediaKind || null,
        tmdbId: record.tmdbId ?? null,
        tmdbParentId: record.tmdbParentId ?? null,
        tmdbSeasonNumber: record.tmdbSeasonNumber ?? null,
        seriesRecordKind: record.seriesRecordKind || null,
      }
    : getEmptyRecord();
}

export function mediaTypeChange(
  form: RecordFormValues,
  mediaType: MediaType,
): { form: RecordFormValues; episodic: boolean } {
  const currentlyEpisodic = isAlwaysEpisodic(form.mediaType) || Boolean(form.totalEpisodes);
  const episodic = isAlwaysEpisodic(mediaType) || (mediaType !== '电影' && currentlyEpisodic);
  return {
    form: {
      ...form,
      mediaType,
      ...(episodic
        ? { movieProgress: null, movieDuration: null }
        : { progress: '', totalEpisodes: null }),
    },
    episodic,
  };
}

export { formatMovieTime, parseTimeToSeconds };
