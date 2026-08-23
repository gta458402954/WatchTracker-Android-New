import type { SyncRuntimeState } from './database';
import type { RegionOption } from './classification';

export interface SyncPresentationInput {
  hasCredentials: boolean;
  syncing: boolean;
  message: string;
  runtime: SyncRuntimeState | null;
  paused: boolean;
}

export interface SyncPresentation {
  label: string;
  description: string;
  className: string;
  pendingCount: number;
}

export function syncPresentation(input: SyncPresentationInput): SyncPresentation {
  const pendingCount = (input.runtime?.stagedCount ?? 0)
    + (input.runtime?.outbox.pending ? 1 : 0)
    + (input.runtime?.publishPending ? 1 : 0);
  if (!input.hasCredentials) return { label: '未配置', description: '尚未配置 WebDAV，同步不会运行。', className: 'border-gray-200 bg-gray-50 text-gray-500', pendingCount };
  if (input.syncing) return { label: '同步中', description: '正在安全核对并合并本机与云端数据。', className: 'border-blue-200 bg-blue-50 text-blue-700', pendingCount };
  if ((input.runtime?.conflictCount ?? 0) > 0) return { label: `${input.runtime?.conflictCount} 项冲突`, description: '存在需要明确选择本机或云端版本的冲突。', className: 'border-red-200 bg-red-50 text-red-700', pendingCount };
  if (input.runtime?.scheduler.lastErrorCode || input.message.startsWith('❌')) return { label: '同步失败', description: '最近一次同步失败，本地数据仍保持安全。', className: 'border-red-200 bg-red-50 text-red-700', pendingCount };
  if (input.paused) return { label: '已暂停', description: '自动同步已暂停，待发布修改会继续保留。', className: 'border-amber-200 bg-amber-50 text-amber-700', pendingCount };
  if (pendingCount > 0 || input.runtime?.scheduler.nextAttemptAt) return { label: '待同步', description: '存在待发布修改或已安排下一次重试。', className: 'border-amber-200 bg-amber-50 text-amber-700', pendingCount };
  return { label: '已同步', description: '自动同步已开启，当前没有待发布修改。', className: 'border-green-200 bg-green-50 text-green-700', pendingCount };
}

export function partitionRegionOptions(options: RegionOption[], activeRegions: string[], directLimit: number): { direct: RegionOption[]; overflow: RegionOption[] } {
  if (directLimit <= 0) return { direct: [], overflow: [...options] };
  const active = new Set(activeRegions);
  const promoted = options.filter(option => active.has(option.code));
  const rest = options.filter(option => !active.has(option.code));
  const direct = [...promoted, ...rest].slice(0, Math.max(directLimit, promoted.length));
  const directCodes = new Set(direct.map(option => option.code));
  return { direct, overflow: options.filter(option => !directCodes.has(option.code)) };
}
