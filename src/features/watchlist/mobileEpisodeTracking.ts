import type { EpisodeCompletion, WatchRecord } from '../../shared/types/index.ts';

export type MobileEpisodeAction = 'enable' | 'complete' | 'jump' | 'retreat' | 'finish' | 'resume';
export type MobileEpisodeWriteReason = 'missing' | 'stale' | 'locked' | 'domain' | 'error';

function episodeErrorText(error: unknown): string {
  if (error instanceof Error) return `${error.name} ${error.message}`;
  if (typeof error === 'string') return error;
  try { return JSON.stringify(error); } catch { return String(error); }
}

export function mobileEpisodeWriteReason(error: unknown): MobileEpisodeWriteReason {
  const message = episodeErrorText(error);
  if (message.includes('episode_record_missing')) return 'missing';
  if (message.includes('stale_episode_progress')) return 'stale';
  if (message.includes('episode_record_locked')) return 'locked';
  if (/episode_(tracking_unsupported_media|total_missing|record_already_completed|total_mismatch|tracking_already_enabled|out_of_range|tracking_not_enabled)/.test(message)) return 'domain';
  return 'error';
}

export function mobileEpisodeTotal(record: Pick<WatchRecord, 'mediaType' | 'totalEpisodes'>): number | null {
  const total = record.totalEpisodes;
  return record.mediaType !== '电影'
    && typeof total === 'number'
    && Number.isInteger(total)
    && total > 0
    ? total
    : null;
}

export function isMobileEpisodeTrackable(record: Pick<WatchRecord, 'mediaType' | 'totalEpisodes'>): boolean {
  return mobileEpisodeTotal(record) !== null;
}

export function mobileEpisodeOptions(record: Pick<WatchRecord, 'mediaType' | 'totalEpisodes'>): number[] {
  const total = mobileEpisodeTotal(record);
  return total === null ? [] : Array.from({ length: total }, (_, index) => index + 1);
}

export function nextEpisodeAfterCompletion(
  record: Pick<WatchRecord, 'mediaType' | 'totalEpisodes' | 'nextEpisode'>,
): number | null {
  const total = mobileEpisodeTotal(record);
  const current = record.nextEpisode;
  if (total === null || typeof current !== 'number' || current < 1 || current > total) return null;
  return current === total ? null : current + 1;
}

export function episodeAdjustmentAction(
  record: Pick<WatchRecord, 'episodeTrackingEnabled' | 'nextEpisode'>,
  target: number | null,
): MobileEpisodeAction {
  if (!record.episodeTrackingEnabled) return 'enable';
  if (target === null) return 'finish';
  if (typeof record.nextEpisode === 'number' && target < record.nextEpisode) return 'retreat';
  if (record.nextEpisode === null) return 'retreat';
  return 'jump';
}

export function maxEpisodeInHistory(completions: readonly EpisodeCompletion[]): number {
  return completions.reduce((maximum, item) => Math.max(maximum, item.episodeNumber), 0);
}

export function resumableNextEpisode(
  record: Pick<WatchRecord, 'mediaType' | 'totalEpisodes' | 'episodeTrackingEnabled' | 'nextEpisode' | 'status'>,
  completions: readonly EpisodeCompletion[],
): number | null {
  const total = mobileEpisodeTotal(record);
  if (total === null || !record.episodeTrackingEnabled || record.nextEpisode !== null || record.status !== '已看') return null;
  const lastRecorded = maxEpisodeInHistory(completions);
  return lastRecorded > 0 && lastRecorded < total ? lastRecorded + 1 : null;
}

export function mobileEpisodeSummary(
  record: Pick<WatchRecord, 'mediaType' | 'totalEpisodes' | 'episodeTrackingEnabled' | 'nextEpisode' | 'status'>,
): string | null {
  const total = mobileEpisodeTotal(record);
  if (total === null) return null;
  if (!record.episodeTrackingEnabled) return `共 ${total} 集 · 未启用逐集跟踪`;
  const nextEpisode = record.nextEpisode;
  if (nextEpisode === null) return `共 ${total} 集 · 已完成逐集跟踪`;
  if (typeof nextEpisode !== 'number' || !Number.isInteger(nextEpisode) || nextEpisode < 1 || nextEpisode > total) return `共 ${total} 集 · 逐集进度与总集数不一致`;
  return `共 ${total} 集 · 下一集：第 ${nextEpisode} 集`;
}
