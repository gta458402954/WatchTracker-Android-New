export interface SyncErrorResult {
  ok: false;
  error: string;
  staleLocal?: boolean;
}

export function syncFailureMessage(error?: string): string | null {
  switch (error) {
    case 'conditional_write_unsupported':
      return '服务器未提供安全条件写入能力，已禁止上传；本地数据没有被覆盖。';
    case 'conditional_validator_rejected':
      return '服务器持续拒绝同一个安全写入条件，已停止重试；本地数据没有被覆盖。';
    case 'remote_busy':
      return '云端数据持续变化，请稍后重试。';
    case 'stale_local_snapshot':
      return '同步期间出现新的本地修改，本次未覆盖本地数据，请再次同步。';
    case 'stale_sync_target':
      return '同步期间云端目标已切换，旧请求已被拒绝；两个目标的数据均未被覆盖。';
    case 'target_migration_required':
      return '旧版 WebDAV 凭据无法安全迁移，请重新输入账号后再同步。';
    case 'unsupported_remote_schema':
      return '云端数据版本高于当前程序，已停止同步且未写入。';
    case 'legacy_remote_changed':
      return '检测到旧版程序仍在写入 records.json；请升级其他设备后再显式导入旧数据。';
    case 'episode_sync_upgrade_required':
      return '逐集历史尚未获准升级云端同步格式；本地数据已保留。';
    case 'episode_completion_conflict':
      return '两端为同一集记录了不同完成时间，已停止上传以避免覆盖。';
    case 'collections_sync_upgrade_required':
      return '收藏集尚未获准升级云端同步格式；本地数据已保留。';
    default:
      return null;
  }
}

/** Converts internal failures to the stable, safe result codes shown by the facade. */
export function syncError(error: unknown): SyncErrorResult {
  const message = String(error);
  if (message.includes('stale_local_snapshot')) return { ok: false, error: 'stale_local_snapshot', staleLocal: true };
  if (message.includes('unsupported_remote_schema')) return { ok: false, error: 'unsupported_remote_schema' };
  if (message.includes('conditional_write_unsupported')) return { ok: false, error: 'conditional_write_unsupported' };
  if (message.includes('conditional_validator_rejected')) return { ok: false, error: 'conditional_validator_rejected' };
  if (message.includes('Invalid WebDAV entity tag')) return { ok: false, error: 'conditional_write_unsupported' };
  if (message.includes('legacy_remote_changed')) return { ok: false, error: 'legacy_remote_changed' };
  if (message.includes('episode_completion_conflict')) return { ok: false, error: 'episode_completion_conflict' };
  if (message.includes('episode_sync_upgrade_required')) return { ok: false, error: 'episode_sync_upgrade_required' };
  if (message.includes('collections_sync_upgrade_required')) return { ok: false, error: 'collections_sync_upgrade_required' };
  return { ok: false, error: message };
}
