export interface SeasonTitleInput {
  seriesName?: string | null;
  originalName?: string | null;
  seasonNumber: number;
  localizedSeasonName?: string | null;
  originalSeasonName?: string | null;
}

export interface SeasonTitles {
  chineseName: string;
  originalName: string;
}

function normalized(value: string | null | undefined): string {
  return value?.trim().replace(/\s+/g, ' ') ?? '';
}

function chineseSeasonNumber(value: string): number | null {
  const digits: Record<string, number> = { 零: 0, 〇: 0, 一: 1, 二: 2, 两: 2, 三: 3, 四: 4, 五: 5, 六: 6, 七: 7, 八: 8, 九: 9 };
  if ([...value].every(character => character in digits)) {
    const number = Number([...value].map(character => digits[character]).join(''));
    return Number.isInteger(number) ? number : null;
  }
  const hundred = value.indexOf('百');
  const ten = value.indexOf('十');
  let number = 0;
  if (hundred >= 0) number += (hundred === 0 ? 1 : digits[value[hundred - 1]]) * 100;
  if (ten >= 0) number += (ten === 0 || (hundred >= 0 && ten === hundred + 1) ? 1 : digits[value[ten - 1]]) * 10;
  const last = value.at(-1)!;
  if (last !== '百' && last !== '十') number += digits[last] ?? Number.NaN;
  return Number.isFinite(number) ? number : null;
}

export function seasonNumberFromText(value: string | null | undefined): number | null {
  const text = normalized(value);
  const arabic = text.match(/第\s*(\d+)\s*季|\bSeason\s*(\d+)\b|\bS(\d{1,3})(?:E\d+)?\b/i);
  const arabicValue = arabic?.slice(1).find(Boolean);
  if (arabicValue != null) return Number.parseInt(arabicValue, 10);
  const chinese = text.match(/第\s*([零〇一二两三四五六七八九十百]+)\s*季/i);
  return chinese ? chineseSeasonNumber(chinese[1]) : null;
}

function isSeasonMarker(value: string, seasonNumber: number): boolean {
  const text = normalized(value);
  if (!text) return true;
  const marker = /^(?:Season\s*\d+|第\s*(?:\d+|[零〇一二两三四五六七八九十百]+)\s*季)$/i;
  return marker.test(text) && seasonNumberFromText(text) === seasonNumber;
}

function isGenericSeasonMarker(value: string): boolean {
  return /^(?:Season\s*\d+|第\s*(?:\d+|[零〇一二两三四五六七八九十百]+)\s*季)$/i.test(normalized(value));
}

function removeSeriesPrefix(value: string, seriesName: string): string {
  if (!seriesName) return value;
  if (value.toLocaleLowerCase() === seriesName.toLocaleLowerCase()) return '';
  if (!value.toLocaleLowerCase().startsWith(seriesName.toLocaleLowerCase())) return value;
  if (!/^[\s:：|\-–—]/.test(value.slice(seriesName.length))) return value;
  return value.slice(seriesName.length).replace(/^\s*[:：|\-–—]\s*/, '').trim();
}

function removeSeasonMarker(value: string, seasonNumber: number): string {
  const marker = /第\s*(?:\d+|[零〇一二两三四五六七八九十百]+)\s*季|Season\s*\d+/ig;
  return value
    .replace(marker, (match, offset: number, source: string) => {
      if (seasonNumberFromText(match) === seasonNumber) return '';
      const before = source.slice(0, offset).trim();
      const after = source.slice(offset + match.length).trim();
      const leadingMarker = !before && (!after || /^[:：|\-–—]/.test(after));
      const trailingMarker = !after && (!before || /[:：|\-–—]$/.test(before));
      return leadingMarker || trailingMarker ? '' : match;
    })
    .replace(/^\s*[:：|\-–—]\s*/, '')
    .replace(/\s*[:：|\-–—]\s*$/, '')
    .trim();
}

function specificSeasonTitle(value: string | null | undefined, seriesName: string, seasonNumber: number): string {
  let title = normalized(value);
  if (!title || isSeasonMarker(title, seasonNumber) || isGenericSeasonMarker(title)) return '';
  title = removeSeriesPrefix(title, seriesName);
  title = removeSeasonMarker(title, seasonNumber);
  return title === seriesName || isSeasonMarker(title, seasonNumber) ? '' : title;
}

function composeSeasonTitle(seriesName: string, specificTitle: string, marker: string, separator: string): string {
  const prefix = [seriesName, marker].filter(Boolean).join(' ').trim();
  return specificTitle ? `${prefix}${separator}${specificTitle}`.trim() : prefix;
}

export function formatSeasonTitles(input: SeasonTitleInput): SeasonTitles {
  const seriesName = normalized(input.seriesName) || normalized(input.originalName);
  const originalName = normalized(input.originalName) || seriesName;
  const localizedSpecific = specificSeasonTitle(input.localizedSeasonName, seriesName, input.seasonNumber);
  const originalSpecific = specificSeasonTitle(input.originalSeasonName, originalName, input.seasonNumber);
  return {
    chineseName: composeSeasonTitle(seriesName, localizedSpecific || originalSpecific, `第 ${input.seasonNumber} 季`, '：'),
    originalName: composeSeasonTitle(originalName, originalSpecific, `Season ${input.seasonNumber}`, ': '),
  };
}
