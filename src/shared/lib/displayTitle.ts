import type { WatchRecord } from '../types';
import { mediaTypeOf, regionCodesOf } from './classification.ts';

export interface DisplayTitles {
  primary: string;
  secondary: string | null;
}

const CHINESE_FIRST_SEASON_SUFFIX = /\s*(?:第\s*(?:1|一)\s*季|第一季)\s*$/;
const ENGLISH_FIRST_SEASON_SUFFIX = /\s+Season\s*1\s*$/i;

function stripFirstSeasonSuffix(value: string, chinese: boolean): string {
  const stripped = value.replace(chinese ? CHINESE_FIRST_SEASON_SUFFIX : ENGLISH_FIRST_SEASON_SUFFIX, '').trim();
  return stripped || value.trim();
}

export function displayTitlesOf(
  record: Pick<WatchRecord, 'chineseName' | 'originalName' | 'mediaType' | 'originCountry' | 'contentTags'>,
): DisplayTitles {
  const hideFirstSeason = mediaTypeOf(record) !== '电影' && regionCodesOf(record).includes('CN');
  const chineseName = hideFirstSeason
    ? stripFirstSeasonSuffix(record.chineseName, true)
    : record.chineseName.trim();
  const originalName = hideFirstSeason
    ? stripFirstSeasonSuffix(record.originalName, false)
    : record.originalName.trim();
  const primary = chineseName || originalName || '未命名条目';
  const secondary = originalName && originalName !== primary ? originalName : null;
  return { primary, secondary };
}
