import type { WatchRecord } from '../../../shared/types';
import type { TmdbSeason } from '../../../shared/lib/classification';

const ARABIC_SEASON_PATTERNS = [
  /第\s*(\d+)\s*季/i,
  /\bSeason\s*(\d+)\b/i,
  /\bS(\d{1,3})(?:E\d+)?\b/i,
];
const CHINESE_SEASON_PATTERN = /第\s*([零〇一二两三四五六七八九十百]+)\s*季/i;

function chineseNumber(value: string): number | null {
  const digits: Record<string, number> = { 零: 0, 〇: 0, 一: 1, 二: 2, 两: 2, 三: 3, 四: 4, 五: 5, 六: 6, 七: 7, 八: 8, 九: 9 };
  if ([...value].every(character => character in digits)) {
    const number = Number([...value].map(character => digits[character]).join(''));
    return Number.isInteger(number) && number >= 0 && number <= 99 ? number : null;
  }
  const hundred = value.indexOf('百');
  const ten = value.indexOf('十');
  let number = 0;
  if (hundred >= 0) number += (hundred === 0 ? 1 : digits[value[hundred - 1]]) * 100;
  if (ten >= 0) number += (ten === 0 || (hundred >= 0 && ten === hundred + 1) ? 1 : digits[value[ten - 1]]) * 10;
  const last = value.at(-1)!;
  if (last !== '百' && last !== '十') number += digits[last] ?? 0;
  return Number.isInteger(number) && number >= 0 && number <= 99 ? number : null;
}

function seasonOfText(value: string | null | undefined): number | null {
  if (!value) return null;
  for (const pattern of ARABIC_SEASON_PATTERNS) {
    const match = value.match(pattern);
    if (match) return Number(match[1]);
  }
  const chinese = value.match(CHINESE_SEASON_PATTERN);
  return chinese ? chineseNumber(chinese[1]) : null;
}

export interface RecordSeasonIdentity {
  seasonNumber: number | null;
  conflicted: boolean;
}

export function recordSeasonIdentity(
  record: Pick<WatchRecord, 'chineseName' | 'originalName' | 'progress'>,
): RecordSeasonIdentity {
  const values = [record.chineseName, record.originalName, record.progress]
    .map(seasonOfText)
    .filter((value): value is number => value !== null);
  const unique = [...new Set(values)];
  return { seasonNumber: unique.length === 1 ? unique[0] : null, conflicted: unique.length > 1 };
}

export function seriesBaseName(value: string | null | undefined): string {
  return (value ?? '')
    .replace(/第\s*\d+\s*季/ig, '')
    .replace(CHINESE_SEASON_PATTERN, '')
    .replace(/\bSeason\s*\d+\b/ig, '')
    .replace(/\bS\d{1,3}(?:E\d+)?\b/ig, '')
    .trim();
}

export function seasonNumberOf(record: Pick<WatchRecord, 'chineseName' | 'originalName' | 'progress'>): number | null {
  return recordSeasonIdentity(record).seasonNumber;
}

export function tvSourceKey(parentId: number): string {
  return `tmdb:tv-show:${parentId}`;
}

export function chronologicalRecords(records: WatchRecord[]): WatchRecord[] {
  return [...records].sort((left, right) => {
    const leftYear = /^\d{4}/.test(left.releaseYear ?? '') ? Number(left.releaseYear?.slice(0, 4)) : Number.MAX_SAFE_INTEGER;
    const rightYear = /^\d{4}/.test(right.releaseYear ?? '') ? Number(right.releaseYear?.slice(0, 4)) : Number.MAX_SAFE_INTEGER;
    if (leftYear !== rightYear) return leftYear - rightYear;
    const leftSeason = seasonNumberOf(left);
    const rightSeason = seasonNumberOf(right);
    const seasonRank = (value: number | null) => value === 0 ? Number.MAX_SAFE_INTEGER : value ?? Number.MAX_SAFE_INTEGER - 1;
    if (seasonRank(leftSeason) !== seasonRank(rightSeason)) return seasonRank(leftSeason) - seasonRank(rightSeason);
    return `${left.chineseName}\0${left.originalName}\0${left.id}`.localeCompare(`${right.chineseName}\0${right.originalName}\0${right.id}`, 'zh-CN');
  });
}

export function locallyKnownSeries(records: WatchRecord[]): Array<{ key: string; name: string; recordIds: string[]; seasons: number[]; tmdbParentId: number | null }> {
  const groups = new Map<string, { name: string; recordIds: string[]; seasons: number[] }>();
  for (const record of records) {
    const identityResult = recordSeasonIdentity(record);
    if (identityResult.conflicted) continue;
    const season = identityResult.seasonNumber;
    if (season == null) continue;
    const chineseBase = seriesBaseName(record.chineseName);
    const originalBase = seriesBaseName(record.originalName);
    const base = chineseBase || originalBase;
    if (!base) continue;
    const identity = record.tmdbParentId ? tvSourceKey(record.tmdbParentId) : null;
    const key = identity || record.imdbId?.trim().toLowerCase() || `title:${base.toLowerCase()}\0${originalBase.toLowerCase()}`;
    const group = groups.get(key) ?? { name: base, recordIds: [], seasons: [] };
    group.recordIds.push(record.id);
    if (!group.seasons.includes(season)) group.seasons.push(season);
    groups.set(key, group);
  }
  return [...groups.entries()].map(([key, value]) => ({
    key,
    ...value,
    seasons: value.seasons.sort((a, b) => a - b),
    tmdbParentId: key.startsWith('tmdb:tv-show:') ? Number(key.slice('tmdb:tv-show:'.length)) : null,
  }));
}

export function seasonIsAired(season: TmdbSeason, now = new Date()): boolean {
  if ((season.season_number ?? 0) <= 0 || !season.air_date) return false;
  const date = new Date(`${season.air_date}T00:00:00Z`);
  return !Number.isNaN(date.valueOf()) && date <= now;
}

export function defaultMissingSeasonNumbers(seasons: TmdbSeason[], existing: Set<number>, now = new Date()): number[] {
  return seasons.filter(season => seasonIsAired(season, now) && !existing.has(season.season_number ?? 0)).map(season => season.season_number as number);
}

interface CacheEntry<T> { value: T; expiresAt: number }
const CACHE_PREFIX = 'watchtracker.tmdb.identity.v1:';

export function readIdentityCache<T>(key: string, now = Date.now()): T | undefined {
  try {
    const raw = localStorage.getItem(`${CACHE_PREFIX}${key}`);
    if (!raw) return undefined;
    const entry = JSON.parse(raw) as CacheEntry<T>;
    if (!Number.isFinite(entry.expiresAt) || entry.expiresAt <= now) {
      localStorage.removeItem(`${CACHE_PREFIX}${key}`);
      return undefined;
    }
    return entry.value;
  } catch { return undefined; }
}

export function writeIdentityCache<T>(key: string, value: T, found: boolean, now = Date.now(), ttlDays?: number): void {
  const days = ttlDays ?? (found ? 30 : 7);
  try { localStorage.setItem(`${CACHE_PREFIX}${key}`, JSON.stringify({ value, expiresAt: now + days * 86_400_000 } satisfies CacheEntry<T>)); } catch { /* derived cache is best effort */ }
}
