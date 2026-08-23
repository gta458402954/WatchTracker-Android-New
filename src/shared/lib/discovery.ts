import type { MediaType, WatchRecord } from '../types';
import { mediaTypeOf } from './classification.ts';

export type DiscoveryDurationLimit = 30 | 60 | 120 | 0;
export type DiscoveryMediaFilter = MediaType | '全部';

export interface DiscoveryFilters {
  durationLimit: DiscoveryDurationLimit;
  mediaType: DiscoveryMediaFilter;
  platform: string | null;
  endedOnly: boolean;
}

export interface ViewingEstimate {
  minutes: number;
  episodic: boolean;
  estimated: boolean;
}

export interface DiscoveryScoreBreakdown {
  interest: number;
  imdb: number;
  completion: number;
  genres: number;
  platform: number;
}

export interface DiscoveryCandidate {
  record: WatchRecord;
  viewing: ViewingEstimate;
  score: number;
  breakdown: DiscoveryScoreBreakdown;
  reasons: string[];
  notes: string[];
}

export type DiscoveryEmptyReason =
  | 'no_unwatched'
  | 'media_type'
  | 'platform'
  | 'ended'
  | 'duration'
  | 'skipped';

export interface DiscoveryQueueResult {
  candidates: DiscoveryCandidate[];
  emptyReason: DiscoveryEmptyReason | null;
}

const MEDIA_ORDER: MediaType[] = ['电影', '剧集', '纪录片', '综艺', '动画'];
const INTEREST_POINTS: Record<number, number> = { 1: 0, 2: 14, 3: 28, 4: 38, 5: 50 };
const INTEREST_LABELS: Record<number, string> = {
  1: '随便看看',
  2: '有点兴趣',
  3: '值得一看',
  4: '非常期待',
  5: '必看神作',
};

function compareText(a: string, b: string): number {
  return a < b ? -1 : a > b ? 1 : 0;
}

function normalizedText(value: string | null | undefined): string {
  return (value ?? '').trim().normalize('NFKC').toLowerCase();
}

function tagsOf(value: string | null | undefined): string[] {
  return [...new Set((value ?? '')
    .split(',')
    .map(normalizedText)
    .filter(Boolean))];
}

function validInterest(value: number | null | undefined): number | null {
  return Number.isInteger(value) && value != null && value >= 1 && value <= 5 ? value : null;
}

function validImdb(value: number | null | undefined): number | null {
  return typeof value === 'number' && Number.isFinite(value) && value >= 0 && value <= 10 ? value : null;
}

export function isEpisodicDiscoveryRecord(record: WatchRecord): boolean {
  return Boolean(record.totalEpisodes && record.totalEpisodes > 0)
    || ['剧集', '综艺'].includes(mediaTypeOf(record));
}

export function estimateDiscoveryViewing(record: WatchRecord): ViewingEstimate {
  const episodic = isEpisodicDiscoveryRecord(record);
  if (episodic) {
    const runtime = record.episodeRuntime;
    return typeof runtime === 'number' && Number.isFinite(runtime) && runtime > 0
      ? { minutes: Math.ceil(runtime), episodic: true, estimated: false }
      : { minutes: 45, episodic: true, estimated: true };
  }
  const duration = record.movieDuration;
  return typeof duration === 'number' && Number.isFinite(duration) && duration > 0
    ? { minutes: Math.ceil(duration / 60), episodic: false, estimated: false }
    : { minutes: 120, episodic: false, estimated: true };
}

export function isDiscoveryCompletedProduction(record: WatchRecord): boolean {
  if (!isEpisodicDiscoveryRecord(record)) return true;
  return record.tmdbStatus === 'Ended' || record.tmdbStatus === 'Miniseries';
}

function preferenceProfile(records: WatchRecord[]) {
  const highlyRated = records.filter(record => record.status === '已看' && (record.rating ?? 0) >= 8);
  const genres = new Set(highlyRated.flatMap(record => tagsOf(record.genres)));
  const platforms = new Set(highlyRated.map(record => normalizedText(record.platform)).filter(Boolean));
  return { genres, platforms };
}

export function scoreDiscoveryRecord(record: WatchRecord, allRecords: WatchRecord[]): Omit<DiscoveryCandidate, 'record' | 'viewing'> {
  const interest = validInterest(record.interestLevel);
  const imdb = validImdb(record.imdbRating);
  const completed = isDiscoveryCompletedProduction(record);
  const profile = preferenceProfile(allRecords);
  const matchingGenres = tagsOf(record.genres).filter(genre => profile.genres.has(genre)).slice(0, 2);
  const platformMatches = Boolean(normalizedText(record.platform) && profile.platforms.has(normalizedText(record.platform)));
  const breakdown: DiscoveryScoreBreakdown = {
    interest: INTEREST_POINTS[interest ?? 3],
    imdb: Math.round((imdb ?? 6) * 3),
    completion: completed ? 8 : 0,
    genres: matchingGenres.length * 4,
    platform: platformMatches ? 4 : 0,
  };
  const score = Math.min(100, Object.values(breakdown).reduce((total, value) => total + value, 0));
  const reasons = [
    interest == null ? null : INTEREST_LABELS[interest],
    imdb == null ? null : `IMDb ${imdb.toFixed(1)}`,
    completed ? '已完结' : null,
    matchingGenres.length ? `匹配喜爱题材：${matchingGenres.join('、')}` : null,
    platformMatches ? `匹配常看平台：${record.platform.trim()}` : null,
  ].filter((reason): reason is string => Boolean(reason)).slice(0, 3);
  const notes = [
    imdb == null ? 'IMDb 评分缺失，按 6.0 计分' : null,
  ].filter((note): note is string => Boolean(note));
  return { score, breakdown, reasons, notes };
}

export function discoveryFilterOptions(records: WatchRecord[]): { mediaTypes: MediaType[]; platforms: string[] } {
  const unwatched = records.filter(record => record.status === '未看');
  const presentMedia = new Set(unwatched.map(mediaTypeOf));
  const mediaTypes = MEDIA_ORDER.filter(type => presentMedia.has(type));
  const platformByKey = new Map<string, string>();
  for (const record of unwatched) {
    const display = record.platform.trim();
    const key = normalizedText(display);
    const previous = platformByKey.get(key);
    if (key && (!previous || compareText(display, previous) < 0)) platformByKey.set(key, display);
  }
  const platforms = [...platformByKey.entries()]
    .sort(([a], [b]) => compareText(a, b))
    .map(([, display]) => display);
  return { mediaTypes, platforms };
}

export function buildDiscoveryQueue(
  records: WatchRecord[],
  filters: DiscoveryFilters,
  skippedIds: ReadonlySet<string> = new Set(),
): DiscoveryQueueResult {
  const unwatched = records.filter(record => record.status === '未看');
  if (!unwatched.length) return { candidates: [], emptyReason: 'no_unwatched' };

  const byMedia = filters.mediaType === '全部'
    ? unwatched
    : unwatched.filter(record => mediaTypeOf(record) === filters.mediaType);
  if (!byMedia.length) return { candidates: [], emptyReason: 'media_type' };

  const platformKey = normalizedText(filters.platform);
  const byPlatform = !platformKey
    ? byMedia
    : byMedia.filter(record => normalizedText(record.platform) === platformKey);
  if (!byPlatform.length) return { candidates: [], emptyReason: 'platform' };

  const byCompletion = filters.endedOnly
    ? byPlatform.filter(isDiscoveryCompletedProduction)
    : byPlatform;
  if (!byCompletion.length) return { candidates: [], emptyReason: 'ended' };

  const withViewing = byCompletion.map(record => ({ record, viewing: estimateDiscoveryViewing(record) }));
  const byDuration = filters.durationLimit
    ? withViewing.filter(item => item.viewing.minutes <= filters.durationLimit)
    : withViewing;
  if (!byDuration.length) return { candidates: [], emptyReason: 'duration' };

  const afterSkipped = byDuration.filter(item => !skippedIds.has(item.record.id));
  if (!afterSkipped.length) return { candidates: [], emptyReason: 'skipped' };

  const candidates = afterSkipped.map(({ record, viewing }) => ({
    record,
    viewing,
    ...scoreDiscoveryRecord(record, records),
  })).sort((a, b) => {
    if (a.score !== b.score) return b.score - a.score;
    const interestA = validInterest(a.record.interestLevel) ?? 3;
    const interestB = validInterest(b.record.interestLevel) ?? 3;
    if (interestA !== interestB) return interestB - interestA;
    const imdbA = validImdb(a.record.imdbRating) ?? 6;
    const imdbB = validImdb(b.record.imdbRating) ?? 6;
    if (imdbA !== imdbB) return imdbB - imdbA;
    const titleOrder = compareText(
      normalizedText(a.record.chineseName || a.record.originalName),
      normalizedText(b.record.chineseName || b.record.originalName),
    );
    return titleOrder || compareText(a.record.id, b.record.id);
  });
  return { candidates, emptyReason: null };
}

export function discoveryEmptyMessage(reason: DiscoveryEmptyReason | null): string {
  switch (reason) {
    case 'no_unwatched': return '暂无待看作品';
    case 'media_type': return '没有符合当前类型的作品';
    case 'platform': return '没有符合当前平台的作品';
    case 'ended': return '没有符合条件的已完结作品';
    case 'duration': return '没有符合当前时长的作品';
    case 'skipped': return '本轮候选已全部跳过';
    default: return '暂无符合条件的待看作品';
  }
}
