import type { Status, WatchRecord } from '../types/index.ts';

export const STATUSES: Status[] = ['在看', '未看', '已看'];

export const STATUS_CONFIG: Record<Status, { label: string; color: string; bg: string }> = {
  '在看': { label: '在看', color: 'text-blue-700', bg: 'bg-blue-100 border-blue-200' },
  '未看': { label: '未看', color: 'text-gray-500', bg: 'bg-gray-100 border-gray-200' },
  '已看': { label: '已看', color: 'text-green-700', bg: 'bg-green-100 border-green-200' },
};



export const PLATFORMS = [
  'Netflix', 'HBO', 'Amazon', 'Apple TV+', 'Disney+', 'Hulu',
  'CBS', 'AMC', 'BBC', 'NHK',
  '爱奇艺', '优酷', '腾讯视频', 'B站', '芒果TV', '寰亚',
  '其他',
];

export function getEmptyRecord(): Omit<WatchRecord, 'id' | 'createdAt'> {
  return {
    originalName: '',
    chineseName: '',
    progress: '',
    totalEpisodes: null,
    episodeTrackingEnabled: false,
    nextEpisode: null,
    movieProgress: null,
    movieDuration: null,
    releaseYear: null,
    posterPath: null,
    status: '未看',
    platform: '',
    rating: null,
    startDate: '',
    endDate: '',
    notes: '',
    imdbId: null,
    genres: null,
    originCountry: null,
    imdbRating: null,
    tmdbStatus: null,
    interestLevel: null,
    episodeRuntime: null,
    mediaType: '电影',
      contentTags: '',
      tmdbMediaKind: null,
      tmdbId: null,
      tmdbParentId: null,
      tmdbSeasonNumber: null,
      seriesRecordKind: null,
  };
}


// 电影时间格式化：将秒数转为 "1h 23m 45s" 格式
export function formatMovieTime(seconds: number | null): string {
  if (seconds === null) return '';
  const h = Math.floor(seconds / 3600);
  const m = Math.floor((seconds % 3600) / 60);
  const s = seconds % 60;
  if (h > 0) {
    return `${h}h ${m.toString().padStart(2, '0')}m ${s.toString().padStart(2, '0')}s`;
  }
  if (m > 0) {
    return `${m}m ${s.toString().padStart(2, '0')}s`;
  }
  return `${s}s`;
}

// 解析时间字符串为秒数，支持 "1:23:45", "1h23m45s", "1h30m", "45:30", "30m45s", "45s" 等格式
export function parseTimeToSeconds(input: string): number | null {
  if (!input || !input.trim()) return null;
  const t = input.trim();

  // "HH:MM:SS" 或 "H:MM:SS" 格式
  const hmsMatch = t.match(/^(?:(\d+):)?(\d+):(\d+)$/);
  if (hmsMatch) {
    const h = parseInt(hmsMatch[1] || '0', 10);
    const m = parseInt(hmsMatch[2], 10);
    const s = parseInt(hmsMatch[3], 10);
    return h * 3600 + m * 60 + s;
  }

  // "Xh Ym Zs" 或 "Xh Ym" 或 "Xm Ys" 或 "Xh" 或 "Xm" 或 "Xs"
  let total = 0;
  const hMatch = t.match(/(\d+)\s*h/i);
  if (hMatch) total += parseInt(hMatch[1], 10) * 3600;
  const mMatch = t.match(/(\d+)\s*m/i);
  if (mMatch) total += parseInt(mMatch[1], 10) * 60;
  const sMatch = t.match(/(\d+)\s*s/i);
  if (sMatch) total += parseInt(sMatch[1], 10);

  if (total > 0 || t === '0' || t === '00') return total;

  return null;
}

// 电影进度显示：在卡片上显示 "45:30 / 2:30:00" 格式
export function formatMovieProgress(current: number | null, total: number | null): string {
  if (!current) return '';
  if (!total) return formatMovieTime(current);

  const fmt = (s: number) => {
    const h = Math.floor(s / 3600);
    const m = Math.floor((s % 3600) / 60);
    const sec = s % 60;
    if (h > 0) {
      return `${h}:${m.toString().padStart(2, '0')}:${sec.toString().padStart(2, '0')}`;
    }
    return `${m}:${sec.toString().padStart(2, '0')}`;
  };

  return `${fmt(current)} / ${fmt(total)}`;
}
