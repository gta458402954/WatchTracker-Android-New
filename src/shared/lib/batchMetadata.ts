import type { MediaType, UpdateWatchRecord, WatchRecord } from '../types/index.ts';
import {
  classifyTmdb,
  inferPlatformFromTmdb,
  mediaTypeOf,
  positiveEpisodeRuntimeOf,
  type TmdbMedia,
} from './classification.ts';

export type TmdbEntityType = 'movie' | 'tv';
export type BatchMetadataField = keyof Pick<UpdateWatchRecord,
  | 'chineseName'
  | 'originalName'
  | 'releaseYear'
  | 'posterPath'
  | 'platform'
  | 'genres'
  | 'originCountry'
  | 'contentTags'
  | 'imdbRating'
  | 'tmdbStatus'
  | 'movieDuration'
  | 'episodeRuntime'
  | 'totalEpisodes'
  | 'tmdbMediaKind'
  | 'tmdbId'
  | 'tmdbParentId'
  | 'tmdbSeasonNumber'
  | 'seriesRecordKind'
>;

export interface TmdbIdentityPlan {
  tmdbMediaKind: 'movie' | 'tv' | 'tv-season';
  tmdbId: number;
  tmdbParentId: number | null;
  tmdbSeasonNumber: number | null;
  seriesRecordKind: 'season' | 'whole-series' | 'single-work';
}

export const BATCH_METADATA_STATE_KEY = 'batch_metadata_no_data_v1';

export interface BatchMetadataNoDataEntry {
  imdbId: string;
  fields: BatchMetadataField[];
  checkedAt: string;
}

export interface BatchMetadataNoDataState {
  version: 1;
  records: Record<string, BatchMetadataNoDataEntry>;
}

export interface TmdbMatch {
  id: number;
  type: TmdbEntityType;
}

export type TmdbMatchResult =
  | { ok: true; match: TmdbMatch }
  | { ok: false; reason: string; candidates?: TmdbMatch[] };

export interface BatchMetadataPatch {
  updates: UpdateWatchRecord;
  fields: BatchMetadataField[];
  seasonNumber: number | null;
  identityPlan?: TmdbIdentityPlan;
  identityConflict?: string;
}

export const BATCH_METADATA_FIELD_LABELS: Record<BatchMetadataField, string> = {
  chineseName: '中文名称',
  originalName: '原始名称',
  releaseYear: '上映年份',
  posterPath: '海报',
  platform: '平台',
  genres: '题材',
  originCountry: '国家',
  contentTags: '地区标签',
  imdbRating: 'TMDB 评分',
  tmdbStatus: 'TMDB 状态',
  movieDuration: '电影时长',
  episodeRuntime: '单集时长',
  totalEpisodes: '总集数',
  tmdbMediaKind: 'TMDB 媒体类型',
  tmdbId: 'TMDB ID',
  tmdbParentId: 'TMDB 父剧 ID',
  tmdbSeasonNumber: 'TMDB 季号',
  seriesRecordKind: '系列记录类型',
};

const EMPTY_NO_DATA_STATE: BatchMetadataNoDataState = { version: 1, records: {} };

function isMissingText(value: string | null | undefined): boolean {
  return !value?.trim();
}

function isPositiveNumber(value: number | null | undefined): value is number {
  return typeof value === 'number' && Number.isFinite(value) && value > 0;
}

function isEpisodicType(type: MediaType): boolean {
  return type === '剧集' || type === '综艺';
}

export function seasonNumberOf(record: Pick<WatchRecord, 'originalName' | 'chineseName' | 'progress'>): number | null {
  const candidates = [
    record.originalName.match(/\bSeason\s+(\d+)\b/i),
    record.chineseName.match(/第\s*(\d+)\s*季/),
    record.progress.match(/\bS(\d{1,3})E\d{1,4}\b/i),
  ];
  for (const match of candidates) {
    const value = match?.[1] ? Number.parseInt(match[1], 10) : 0;
    if (value > 0) return value;
  }
  return null;
}

export function tmdbTypeHintOf(record: WatchRecord): TmdbEntityType | null {
  const type = mediaTypeOf(record);
  if (type === '电影') return 'movie';
  if (isEpisodicType(type)) return 'tv';

  if (
    isPositiveNumber(record.totalEpisodes)
    || isPositiveNumber(record.episodeRuntime)
    || seasonNumberOf(record) != null
    || /\bS\d{1,3}E\d{1,4}\b/i.test(record.progress)
  ) return 'tv';
  if (isPositiveNumber(record.movieDuration) || isPositiveNumber(record.movieProgress)) return 'movie';
  return null;
}

export function missingBatchMetadataFields(
  record: WatchRecord,
  tmdbType: TmdbEntityType | null = tmdbTypeHintOf(record),
): BatchMetadataField[] {
  const fields: BatchMetadataField[] = [];
  const textFields = [
    'chineseName', 'originalName', 'releaseYear', 'posterPath', 'platform',
    'genres', 'originCountry', 'contentTags', 'tmdbStatus',
  ] as const;
  for (const field of textFields) {
    if (isMissingText(record[field])) fields.push(field);
  }
  if (!isPositiveNumber(record.imdbRating)) fields.push('imdbRating');
  if (tmdbType === 'movie') {
    if (!isPositiveNumber(record.movieDuration)) fields.push('movieDuration');
    if (record.tmdbMediaKind == null) fields.push('tmdbMediaKind');
    if (!isPositiveNumber(record.tmdbId)) fields.push('tmdbId');
    if (record.seriesRecordKind == null) fields.push('seriesRecordKind');
  } else if (tmdbType === 'tv') {
    if (!isPositiveNumber(record.episodeRuntime)) fields.push('episodeRuntime');
    if (!isPositiveNumber(record.totalEpisodes)) fields.push('totalEpisodes');
    const seasonNumber = record.tmdbSeasonNumber ?? seasonNumberOf(record);
    if (seasonNumber != null) {
      if (record.tmdbMediaKind == null) fields.push('tmdbMediaKind');
      if (!isPositiveNumber(record.tmdbId)) fields.push('tmdbId');
      if (!isPositiveNumber(record.tmdbParentId)) fields.push('tmdbParentId');
      if (!isPositiveNumber(record.tmdbSeasonNumber)) fields.push('tmdbSeasonNumber');
      if (record.seriesRecordKind == null) fields.push('seriesRecordKind');
    } else if (record.tmdbMediaKind === 'tv' || record.seriesRecordKind === 'whole-series') {
      if (!isPositiveNumber(record.tmdbId)) fields.push('tmdbId');
      if (record.tmdbMediaKind == null) fields.push('tmdbMediaKind');
      if (record.seriesRecordKind == null) fields.push('seriesRecordKind');
    }
  } else {
    if (!isPositiveNumber(record.movieDuration)) fields.push('movieDuration');
    if (!isPositiveNumber(record.episodeRuntime)) fields.push('episodeRuntime');
    if (!isPositiveNumber(record.totalEpisodes)) fields.push('totalEpisodes');
  }
  return fields;
}

export function isBatchMetadataCandidate(
  record: WatchRecord,
  noDataFields: ReadonlySet<BatchMetadataField> = new Set(),
): boolean {
  if (!record.imdbId?.trim() || record.isLocked) return false;
  return missingBatchMetadataFields(record).some(field => !noDataFields.has(field));
}

export function selectTmdbMatch(record: WatchRecord, results: TmdbMedia[]): TmdbMatchResult {
  const eligible = results.flatMap(result => {
    if (!isPositiveNumber(result.id) || (result.media_type !== 'movie' && result.media_type !== 'tv')) return [];
    return [{ id: result.id, type: result.media_type } satisfies TmdbMatch];
  }).filter((candidate, index, all) => all.findIndex(item => item.id === candidate.id && item.type === candidate.type) === index);
  const hint = tmdbTypeHintOf(record);

  if (hint) {
    const matches = eligible.filter(candidate => candidate.type === hint);
    if (matches.length === 1) return { ok: true, match: matches[0] };
    if (matches.length > 1) return { ok: false, reason: 'TMDB 返回多个匹配结果，请选择正确条目', candidates: matches };
    return { ok: false, reason: `TMDB 没有返回匹配的${hint === 'movie' ? '电影' : '剧集'}结果` };
  }

  if (eligible.length > 1) return { ok: false, reason: '媒体类型不明确，请选择正确的 TMDB 条目', candidates: eligible };
  if (eligible.length === 0) return { ok: false, reason: 'TMDB 没有返回可用结果' };
  return { ok: true, match: eligible[0] };
}

export function buildBatchMetadataPatch(
  record: WatchRecord,
  detail: TmdbMedia,
  tmdbType: TmdbEntityType,
): BatchMetadataPatch {
  const updates: UpdateWatchRecord = {};
  const classification = classifyTmdb(detail, tmdbType === 'tv', mediaTypeOf(record));
  const seasonNumber = tmdbType === 'tv' ? (record.tmdbSeasonNumber ?? seasonNumberOf(record)) : null;
  const targetSeason = seasonNumber == null
    ? undefined
    : detail.seasons?.find(season => season.season_number === seasonNumber);
  const localizedBaseName = detail.name || detail.title;
  const localizedName = seasonNumber == null
    ? localizedBaseName
    : localizedBaseName
      ? `${localizedBaseName} ${targetSeason?.name || `第 ${seasonNumber} 季`}`
      : undefined;
  const originalBaseName = detail.original_name || detail.original_title;
  const originalName = seasonNumber == null
    ? originalBaseName
    : originalBaseName ? `${originalBaseName} Season ${seasonNumber}` : undefined;
  const releaseDate = targetSeason?.air_date || detail.release_date || detail.first_air_date;
  const releaseYear = releaseDate?.split('-')[0];
  const posterPath = targetSeason?.poster_path || detail.poster_path;
  const platform = inferPlatformFromTmdb(
    classification.originCountry,
    detail.networks?.[0]?.name || detail.production_companies?.[0]?.name,
  );

  if (isMissingText(record.chineseName) && localizedName?.trim()) updates.chineseName = localizedName.trim();
  if (isMissingText(record.originalName) && originalName?.trim()) updates.originalName = originalName.trim();
  if (isMissingText(record.releaseYear) && /^\d{4}$/.test(releaseYear ?? '')) updates.releaseYear = releaseYear;
  if (isMissingText(record.posterPath) && posterPath?.trim()) updates.posterPath = posterPath.trim();
  if (isMissingText(record.platform) && platform?.trim()) updates.platform = platform.trim();

  if (isMissingText(record.genres) && !isMissingText(classification.genres)) {
    updates.genres = classification.genres;
  }
  if (isMissingText(record.originCountry) && !isMissingText(classification.originCountry)) {
    updates.originCountry = classification.originCountry;
  }
  if (isMissingText(record.contentTags) && !isMissingText(classification.contentTags)) {
    updates.contentTags = classification.contentTags;
  }
  if (!isPositiveNumber(record.imdbRating) && isPositiveNumber(detail.vote_average)) {
    updates.imdbRating = detail.vote_average;
  }
  if (isMissingText(record.tmdbStatus) && !isMissingText(detail.status)) {
    updates.tmdbStatus = detail.status?.trim();
  }

  if (tmdbType === 'movie') {
    if (!isPositiveNumber(record.movieDuration) && isPositiveNumber(detail.runtime)) {
      updates.movieDuration = Math.round(detail.runtime * 60);
    }
    const identityPlan: TmdbIdentityPlan | undefined = isPositiveNumber(detail.id) ? {
      tmdbMediaKind: 'movie', tmdbId: detail.id, tmdbParentId: null, tmdbSeasonNumber: null, seriesRecordKind: 'single-work',
    } : undefined;
    return finishBatchPatch(record, updates, seasonNumber, identityPlan);
  } else {
    const runtime = positiveEpisodeRuntimeOf(detail);
    if (!isPositiveNumber(record.episodeRuntime) && runtime !== null) {
      updates.episodeRuntime = runtime;
    }

    const episodeCount = seasonNumber == null
      ? detail.number_of_episodes
      : detail.seasons?.find(season => season.season_number === seasonNumber)?.episode_count;
    if (!isPositiveNumber(record.totalEpisodes) && isPositiveNumber(episodeCount)) {
      updates.totalEpisodes = Math.round(episodeCount);
    }
    const identityPlan: TmdbIdentityPlan | undefined = seasonNumber != null && isPositiveNumber(targetSeason?.id) && isPositiveNumber(detail.id)
      ? { tmdbMediaKind: 'tv-season', tmdbId: targetSeason.id, tmdbParentId: detail.id, tmdbSeasonNumber: seasonNumber, seriesRecordKind: 'season' }
      : seasonNumber == null && (record.tmdbMediaKind === 'tv' || record.seriesRecordKind === 'whole-series') && isPositiveNumber(detail.id)
        ? { tmdbMediaKind: 'tv', tmdbId: detail.id, tmdbParentId: null, tmdbSeasonNumber: null, seriesRecordKind: 'whole-series' }
        : undefined;
    return finishBatchPatch(record, updates, seasonNumber, identityPlan);
  }
}

const IDENTITY_FIELDS = ['tmdbMediaKind', 'tmdbId', 'tmdbParentId', 'tmdbSeasonNumber', 'seriesRecordKind'] as const;

function finishBatchPatch(record: WatchRecord, updates: UpdateWatchRecord, seasonNumber: number | null, identityPlan?: TmdbIdentityPlan): BatchMetadataPatch {
  let identityConflict: string | undefined;
  if (identityPlan) {
    for (const field of IDENTITY_FIELDS) {
      const current = record[field];
      const expected = identityPlan[field];
      if (current != null && current !== expected) {
        identityConflict = `${BATCH_METADATA_FIELD_LABELS[field]} 已有值与 TMDB 匹配结果不同`;
        break;
      }
    }
    if (!identityConflict) {
      for (const field of IDENTITY_FIELDS) {
        if (record[field] == null && identityPlan[field] != null) Object.assign(updates, { [field]: identityPlan[field] });
      }
    }
  }
  return { updates, fields: Object.keys(updates) as BatchMetadataField[], seasonNumber, identityPlan: identityConflict ? undefined : identityPlan, identityConflict };
}

export function retainMissingMetadataPatch(
  current: WatchRecord,
  planned: UpdateWatchRecord,
): BatchMetadataPatch {
  const updates: UpdateWatchRecord = {};
  const textFields = [
    'chineseName', 'originalName', 'releaseYear', 'posterPath', 'platform',
    'genres', 'originCountry', 'contentTags', 'tmdbStatus',
  ] as const;
  const numberFields = ['imdbRating', 'movieDuration', 'episodeRuntime', 'totalEpisodes'] as const;
  const identityFields = ['tmdbMediaKind', 'tmdbId', 'tmdbParentId', 'tmdbSeasonNumber', 'seriesRecordKind'] as const;

  for (const field of textFields) {
    const value = planned[field];
    if (isMissingText(current[field]) && typeof value === 'string' && value.trim()) updates[field] = value;
  }
  for (const field of numberFields) {
    const value = planned[field];
    if (!isPositiveNumber(current[field]) && isPositiveNumber(value)) updates[field] = value;
  }
  for (const field of identityFields) {
    const value = planned[field];
    if (current[field] == null && value != null) Object.assign(updates, { [field]: value });
  }

  return {
    updates,
    fields: Object.keys(updates) as BatchMetadataField[],
    seasonNumber: current.tmdbSeasonNumber ?? seasonNumberOf(current),
  };
}

export function selectBatchMetadataPatch(
  patch: BatchMetadataPatch,
  allowedFields: ReadonlySet<BatchMetadataField>,
): BatchMetadataPatch {
  const updates: UpdateWatchRecord = {};
  for (const field of patch.fields) {
    if (allowedFields.has(field)) Object.assign(updates, { [field]: patch.updates[field] });
  }
  const identitySelected = patch.fields.some(field => allowedFields.has(field) && IDENTITY_FIELDS.includes(field as typeof IDENTITY_FIELDS[number]));
  return { updates, fields: Object.keys(updates) as BatchMetadataField[], seasonNumber: patch.seasonNumber, identityPlan: identitySelected ? patch.identityPlan : undefined, identityConflict: patch.identityConflict };
}

export function parseBatchMetadataNoDataState(raw: string | null | undefined): BatchMetadataNoDataState {
  if (!raw) return structuredClone(EMPTY_NO_DATA_STATE);
  try {
    const value: unknown = JSON.parse(raw);
    if (!value || typeof value !== 'object' || (value as { version?: unknown }).version !== 1) {
      return structuredClone(EMPTY_NO_DATA_STATE);
    }
    const rawRecords = (value as { records?: unknown }).records;
    if (!rawRecords || typeof rawRecords !== 'object' || Array.isArray(rawRecords)) {
      return structuredClone(EMPTY_NO_DATA_STATE);
    }
    const records: Record<string, BatchMetadataNoDataEntry> = {};
    for (const [id, entry] of Object.entries(rawRecords)) {
      if (!entry || typeof entry !== 'object') continue;
      const candidate = entry as Partial<BatchMetadataNoDataEntry>;
      if (typeof candidate.imdbId !== 'string' || !Array.isArray(candidate.fields)) continue;
      const fields = candidate.fields.filter((field): field is BatchMetadataField => (
        typeof field === 'string' && Object.hasOwn(BATCH_METADATA_FIELD_LABELS, field)
      ));
      records[id] = {
        imdbId: candidate.imdbId,
        fields: [...new Set(fields)],
        checkedAt: typeof candidate.checkedAt === 'string' ? candidate.checkedAt : '',
      };
    }
    return { version: 1, records };
  } catch {
    return structuredClone(EMPTY_NO_DATA_STATE);
  }
}

export function noDataFieldsForRecord(
  state: BatchMetadataNoDataState,
  record: WatchRecord,
): Set<BatchMetadataField> {
  const entry = state.records[record.id];
  if (!entry || entry.imdbId !== record.imdbId?.trim()) return new Set();
  return new Set(entry.fields);
}

export function recordNoDataFields(
  state: BatchMetadataNoDataState,
  record: WatchRecord,
  fields: readonly BatchMetadataField[],
  checkedAt = new Date().toISOString(),
): BatchMetadataNoDataState {
  if (!record.imdbId?.trim() || fields.length === 0) return state;
  const existing = noDataFieldsForRecord(state, record);
  for (const field of fields) existing.add(field);
  return {
    version: 1,
    records: {
      ...state.records,
      [record.id]: { imdbId: record.imdbId.trim(), fields: [...existing], checkedAt },
    },
  };
}

export function pruneBatchMetadataNoDataState(
  state: BatchMetadataNoDataState,
  records: readonly WatchRecord[],
): BatchMetadataNoDataState {
  const active = new Map(records.map(record => [record.id, record.imdbId?.trim()]));
  return {
    version: 1,
    records: Object.fromEntries(Object.entries(state.records).filter(([id, entry]) => active.get(id) === entry.imdbId)),
  };
}

export function remoteIdentityKey(match: TmdbMatch, seasonNumber: number | null): string {
  return `${match.type}:${match.id}:${seasonNumber == null ? 'series' : `season-${seasonNumber}`}`;
}
