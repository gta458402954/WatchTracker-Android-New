import type { WatchRecord } from '../types';
import { isEpisodicDiscoveryRecord } from './discovery.ts';

function readableMinutes(seconds: number): string {
  const totalMinutes = Math.max(1, Math.round(seconds / 60));
  const hours = Math.floor(totalMinutes / 60);
  const minutes = totalMinutes % 60;
  if (!hours) return `${minutes} 分钟`;
  return `${hours} 小时${minutes ? ` ${minutes} 分钟` : ''}`;
}

export function dashboardWatchingProgress(record: WatchRecord): string {
  if (isEpisodicDiscoveryRecord(record)) {
    if (record.episodeTrackingEnabled) {
      return record.nextEpisode == null ? '逐集记录已完结' : `下一集：第 ${record.nextEpisode} 集`;
    }
    return record.progress || '尚未记录进度';
  }

  const current = record.movieProgress;
  const total = record.movieDuration;
  if (typeof current !== 'number' || !Number.isFinite(current) || current <= 0) return '尚未记录进度';
  if (typeof total !== 'number' || !Number.isFinite(total) || total <= 0) {
    return `已观看 ${readableMinutes(current)}`;
  }
  const percentage = Math.min(100, Math.max(0, Math.round((current / total) * 100)));
  return `已观看 ${readableMinutes(current)} / ${readableMinutes(total)} · ${percentage}%`;
}
