import type { WatchRecord } from '../types';
import { mediaTypeOf, regionCodesOf } from './classification.ts';

export interface DisplayTitles {
  primary: string;
  secondary: string | null;
}

function stripFirstSeasonSuffix(value: string, chinese: boolean): string {
  const marker = chinese ? /第\s*(?:1|一)\s*季|第一季/i : /\bSeason\s*1\b/i;
  const match = value.match(marker);
  if (!match || match.index == null) return value.trim();
  const before = value.slice(0, match.index).trim();
  const after = value.slice(match.index + match[0].length);
  const delimiter = after.match(/^\s*[:：|\-–—]\s*/);
  if (after.trim() && !delimiter) return value.trim();
  const suffix = delimiter ? after.slice(delimiter[0].length).trim() : '';
  if (!before) return suffix || value.trim();
  if (!suffix) return before;
  return chinese ? `${before}：${suffix}` : `${before}: ${suffix}`;
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
