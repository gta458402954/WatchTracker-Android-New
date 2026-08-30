export interface LocalImportPreview {
  stageToken: string;
  stageSha256: string;
  currentLibraryFingerprint: string;
  fileName: string;
  sizeBytes: number;
  formatVersion: number;
  exportedAt: string;
  counts: {
    records: number;
    episodeCompletions: number;
    collections: number;
    collectionMembers: number;
  };
  records: {
    added: number;
    updated: number;
    removed: number;
    unchanged: number;
    lockedPreserved: number;
    finalCount: number;
  };
}

export interface LocalImportResult {
  recoveryPointId: string;
  recordCount: number;
  episodeCompletionCount: number;
  collectionCount: number;
  collectionMemberCount: number;
  lockedPreservedCount: number;
}

const IMPORT_MESSAGES: Record<string, string> = {
  invalid_json: '不是有效的 JSON 文件。',
  invalid_backup_format: '文件不是有效的 WatchTracker V4 备份。',
  missing_format_version: '备份缺少格式版本。',
  unsupported_backup_version: '此备份版本不受支持。',
  future_backup_version: '备份来自更新版本，请先升级 WatchTracker。',
  unknown_backup_field: '备份包含不受支持的额外数据。',
  invalid_backup_metadata: '备份的导出时间或元数据无效。',
  invalid_records: '备份中的影视记录格式无效。',
  invalid_episode_history: '备份中的逐集历史不完整或损坏。',
  invalid_collections: '备份中的收藏数据无效。',
  import_preview_stale: '本地片库在预览后发生变化，请重新检查后再导入。',
  import_stage_changed: '导入文件在预览后发生变化，请重新检查导入文件。',
  import_file_too_large: '备份文件过大。',
  import_stage_missing: '导入文件已失效，请重新选择。',
  document_import_unavailable: '当前环境无法打开 Android 文件选择器。',
  document_picker_unavailable: '无法打开系统文件选择器。',
  document_read_failed: '无法读取所选备份文件。',
};

export function localImportErrorCode(error: unknown): string {
  const message = error instanceof Error ? error.message : String(error ?? '');
  return Object.keys(IMPORT_MESSAGES).find(code => message.includes(code)) ?? 'import_failed';
}

export function localImportErrorMessage(error: unknown): string {
  return IMPORT_MESSAGES[localImportErrorCode(error)] ?? '无法安全验证此备份。';
}

export function formatImportBytes(value: number): string {
  if (value < 1024) return `${value} B`;
  if (value < 1024 * 1024) return `${(value / 1024).toFixed(1)} KiB`;
  return `${(value / (1024 * 1024)).toFixed(1)} MiB`;
}
