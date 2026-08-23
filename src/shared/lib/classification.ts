import { MEDIA_TYPE_VALUES, type MediaType, type WatchRecord } from '../types/watchRecord.generated.ts';
import {
  type CountryCode,
  PREFERRED_COUNTRY_CODES,
  UNKNOWN_REGION_CODE,
  countryCodeOfLabel,
  countryLabelOf,
} from './countryNames.ts';

export const MEDIA_TYPES: readonly MediaType[] = MEDIA_TYPE_VALUES;

export const REGION_TAGS = ['美国', '韩国', '日本', '英国', '中国大陆', '中国香港', '中国台湾'] as const;
export type RegionTag = (typeof REGION_TAGS)[number];

const SPECIAL_MEDIA_TYPES: readonly MediaType[] = ['纪录片', '综艺', '动画'];

export interface TmdbGenre {
  name?: string;
}

export interface TmdbProductionCountry {
  iso_3166_1?: string;
}

export interface TmdbSeason {
  id?: number;
  name?: string;
  season_number?: number;
  episode_count?: number;
  air_date?: string | null;
  poster_path?: string | null;
}

export interface TmdbMedia {
  id?: number;
  title?: string;
  name?: string;
  original_title?: string;
  original_name?: string;
  original_language?: string;
  release_date?: string;
  first_air_date?: string;
  imdb_id?: string;
  vote_average?: number;
  poster_path?: string | null;
  media_type?: 'movie' | 'tv' | 'tv_season' | 'tv_episode';
  show_id?: number;
  season_number?: number;
  episode_number?: number;
  episode_count?: number;
  air_date?: string | null;
  networks?: Array<{ name?: string }>;
  production_companies?: Array<{ name?: string }>;
  external_ids?: { imdb_id?: string };
  runtime?: number;
  episode_run_time?: number[];
  number_of_episodes?: number;
  status?: string;
  genres?: TmdbGenre[];
  origin_country?: string[];
  production_countries?: TmdbProductionCountry[];
  seasons?: TmdbSeason[];
  parts?: TmdbMedia[];
  belongs_to_collection?: { id?: number; name?: string; poster_path?: string | null; backdrop_path?: string | null } | null;
}

export interface TmdbSearchResponse {
  success?: boolean;
  results?: TmdbMedia[];
  data?: TmdbMedia;
  error?: string;
}

export function mediaTypeOf(record: Pick<WatchRecord, 'mediaType'>): MediaType {
  return MEDIA_TYPES.includes(record.mediaType) ? record.mediaType : '电影';
}

export function contentTagsOf(record: Pick<WatchRecord, 'contentTags'>): string[] {
  return record.contentTags?.split(/[,，]/).map(tag => tag.trim()).filter(Boolean) ?? [];
}

const INVALID_COUNTRY_VALUES = new Set(['N/A', 'NA', 'NULL', 'UNKNOWN']);

export function normalizeCountryCodes(originCountry?: string | null): CountryCode[] {
  if (!originCountry) return [];

  const codes: CountryCode[] = [];
  for (const rawValue of originCountry.split(/[,，]/)) {
    const value = rawValue.trim();
    if (!value) continue;

    const upperValue = value.toUpperCase();
    if (INVALID_COUNTRY_VALUES.has(upperValue)) continue;

    const code = countryCodeOfLabel(value) ?? upperValue;
    if (/^[A-Z]{2}$/.test(code)) codes.push(code);
  }

  return [...new Set(codes)];
}

export function hasCountryCode(originCountry: string | null | undefined, countryCode: CountryCode): boolean {
  const normalizedCode = countryCode.trim().toUpperCase();
  return /^[A-Z]{2}$/.test(normalizedCode)
    && normalizeCountryCodes(originCountry).includes(normalizedCode);
}

export function inferPlatformFromTmdb(
  originCountry: string | null | undefined,
  candidate: string | null | undefined,
): string | null {
  if (hasCountryCode(originCountry, 'CN')) return null;

  const platform = candidate?.trim();
  if (!platform) return null;
  if (platform === 'CBS All Access') return 'CBS';
  if (/^Apple\s*Tv/i.test(platform)) return 'Apple TV+';
  return platform;
}

export function positiveEpisodeRuntimeOf(
  detail: Pick<TmdbMedia, 'episode_run_time' | 'runtime'>,
): number | null {
  const runtime = detail.episode_run_time?.find(value => Number.isFinite(value) && value > 0)
    ?? detail.runtime;
  if (typeof runtime !== 'number' || !Number.isFinite(runtime) || runtime <= 0) return null;

  const rounded = Math.round(runtime);
  return rounded > 0 ? rounded : null;
}

export function regionCodesOf(
  record: Pick<WatchRecord, 'originCountry' | 'contentTags'>,
): CountryCode[] {
  const originCodes = normalizeCountryCodes(record.originCountry);
  if (originCodes.length > 0) return originCodes;

  const legacyCodes = contentTagsOf(record)
    .map(countryCodeOfLabel)
    .filter((code): code is CountryCode => code !== undefined);

  return legacyCodes.length > 0 ? [...new Set(legacyCodes)] : [UNKNOWN_REGION_CODE];
}

export function regionCodesForTopFilter(
  record: Pick<WatchRecord, 'originCountry' | 'contentTags'>,
): CountryCode[] {
  return regionCodesOf(record).slice(0, 1);
}

// B-002 will move the UI to ISO-code filters. Until then this wrapper keeps the
// existing fixed Chinese buttons working without maintaining a second parser.
export function regionsOf(record: Pick<WatchRecord, 'originCountry' | 'contentTags'>): RegionTag[] {
  return regionCodesOf(record)
    .map(countryLabelOf)
    .filter((label): label is RegionTag => REGION_TAGS.includes(label as RegionTag));
}

export function hasRegion(
  record: Pick<WatchRecord, 'originCountry' | 'contentTags'>,
  region: RegionTag,
): boolean {
  return regionsOf(record).includes(region);
}

export interface RegionOption {
  code: CountryCode;
  label: string;
  count: number;
}

export function compareRegionOptions(a: RegionOption, b: RegionOption): number {
  if (a.code === UNKNOWN_REGION_CODE) return b.code === UNKNOWN_REGION_CODE ? 0 : 1;
  if (b.code === UNKNOWN_REGION_CODE) return -1;

  const preferredCodes = PREFERRED_COUNTRY_CODES as readonly string[];
  const preferredA = preferredCodes.indexOf(a.code);
  const preferredB = preferredCodes.indexOf(b.code);
  if (preferredA !== -1 && preferredB !== -1) return preferredA - preferredB;
  if (preferredA !== -1) return -1;
  if (preferredB !== -1) return 1;
  if (b.count !== a.count) return b.count - a.count;

  const labelOrder = a.label.localeCompare(b.label, 'zh-CN');
  return labelOrder !== 0 ? labelOrder : a.code.localeCompare(b.code, 'en');
}

export function aggregateRegions(
  records: ReadonlyArray<Pick<WatchRecord, 'originCountry' | 'contentTags'>>,
): RegionOption[] {
  const counts = new Map<CountryCode, number>();
  for (const record of records) {
    for (const code of regionCodesForTopFilter(record)) {
      counts.set(code, (counts.get(code) ?? 0) + 1);
    }
  }

  return [...counts.entries()]
    .map(([code, count]) => ({ code, label: countryLabelOf(code), count }))
    .sort(compareRegionOptions);
}

export function mergeContentTags(existing: string | null | undefined, tmdbRegions: string): string {
  const customTags = (existing ?? '').split(/[,，]/).map(tag => tag.trim()).filter(tag =>
    tag && tag !== '纪录片' && countryCodeOfLabel(tag) === undefined,
  );
  const regions = tmdbRegions.split(/[,，]/).map(tag => tag.trim()).filter(Boolean);
  return [...new Set([...customTags, ...regions])].join(',');
}

export function classifyTmdb(
  detail: TmdbMedia,
  isTV: boolean,
  preferredType?: MediaType | null,
): { mediaType: MediaType; contentTags: string; originCountry: string | null; genres: string | null } {
  const rawCountryCodes = isTV
    ? detail.origin_country ?? []
    : detail.production_countries?.map(country => country.iso_3166_1 ?? '') ?? [];
  const countryCodes = normalizeCountryCodes(rawCountryCodes.join(','));
  const regions = countryCodes
    .map(code => countryLabelOf(code))
    .filter((label, index) => label !== countryCodes[index]);
  const genreNames = detail.genres?.map(genre => genre.name?.trim()).filter((name): name is string => Boolean(name)) ?? [];
  const isDocumentary = genreNames.some(name => name === 'Documentary' || name === '纪录片');
  const isReality = genreNames.some(name => name === 'Reality' || name === '真人秀');

  let mediaType: MediaType;
  if (isDocumentary) {
    mediaType = '纪录片';
  } else if (preferredType && SPECIAL_MEDIA_TYPES.includes(preferredType)) {
    mediaType = preferredType;
  } else if (isTV && isReality) {
    mediaType = '综艺';
  } else {
    mediaType = isTV ? '剧集' : '电影';
  }

  return {
    mediaType,
    contentTags: [...new Set(regions)].join(','),
    originCountry: countryCodes.join(', ') || null,
    genres: genreNames.join(',') || null,
  };
}

export function errorMessage(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}
