/** Viewing dates carry known precision; a year is never a synthesized day. */
export function viewingDatePrecision(value: string | null | undefined): 'year' | 'day' | null {
  if (!value || !/^[0-9]{4}(?:-[0-9]{2}-[0-9]{2})?$/.test(value)) return null;
  const year = Number(value.slice(0, 4));
  if (year < 1) return null;
  if (value.length === 4) return 'year';
  const month = Number(value.slice(5, 7));
  const day = Number(value.slice(8, 10));
  const leap = year % 4 === 0 && (year % 100 !== 0 || year % 400 === 0);
  const days = [31, leap ? 29 : 28, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31];
  return month >= 1 && month <= 12 && day >= 1 && day <= days[month - 1] ? 'day' : null;
}

export function validViewingDateDraft(value: string | null | undefined): boolean {
  return value == null || value === '' || viewingDatePrecision(value) !== null;
}

export function viewingDateText(value: string | null | undefined, event: 'start' | 'end'): string {
  const precision = viewingDatePrecision(value);
  if (!precision) return '没有记录日期';
  const year = value!.slice(0, 4);
  const known = precision === 'year' ? `${year} 年` : `${year} 年 ${Number(value!.slice(5, 7))} 月 ${Number(value!.slice(8, 10))} 日`;
  return `${known}${event === 'start' ? '开始观看' : '看完'}`;
}

/** Descending calendar year; known days first within a year, year-only last.
 * This is a display order, not a claim about the unknown completion day. */
export function compareViewingDatesDesc(a: string | null | undefined, b: string | null | undefined): number {
  const pa = viewingDatePrecision(a); const pb = viewingDatePrecision(b);
  if (!pa || !pb) return pa ? -1 : pb ? 1 : 0;
  const year = Number(b!.slice(0, 4)) - Number(a!.slice(0, 4));
  if (year) return year;
  if (pa !== pb) return pa === 'day' ? -1 : 1;
  return pa === 'day' ? b!.localeCompare(a!) : 0;
}

/** Exact-date windows/charts require an actual day; year-only returns null. */
export function exactViewingDay(value: string | null | undefined): Date | null {
  return viewingDatePrecision(value) === 'day' ? new Date(`${value}T00:00:00`) : null;
}
