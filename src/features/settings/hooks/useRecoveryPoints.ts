import { useCallback, useState } from 'react';
import type { WatchRecord } from '../../../shared/types';
import {
  deleteRecoveryPoint,
  listRecoveryPoints,
  openBackupDirectory,
  restoreRecoveryPoint,
  setRecoveryPointRetained,
  type RecoveryPoint,
  type RecoveryPointList,
} from '../../../shared/lib/database';
import type { NoticeTone } from '../../../shared/lib/feedback';

const RECOVERY_REASON_LABELS: Record<RecoveryPoint['reason'], string> = {
  import: '全量导入前', sync: '同步落盘前', 'batch-metadata': '批量补全前', migration: '数据库迁移前',
  'target-migration': '同步目标迁移前', 'episode-history-migration': '逐集历史迁移前',
  'collections-migration': '收藏集迁移前', 'series-identity-migration': '系列身份迁移前',
  'series-completion': '补充系列条目前', 'pre-restore': '恢复操作前',
};

interface Options {
  records: WatchRecord[];
  onDatabaseRestored: () => Promise<WatchRecord[]>;
  onNotify?: (tone: NoticeTone, message: string) => void;
  showFailure: (scope: string, action: string, error: unknown, setStatus?: (message: string) => void) => void;
  showSuccess: (message: string) => void;
}

export function useRecoveryPoints({ records, onDatabaseRestored, showFailure, showSuccess }: Options) {
  const [recoveryPoints, setRecoveryPoints] = useState<RecoveryPointList | null>(null);
  const [recoveryStatus, setRecoveryStatus] = useState('');
  const [recoveryBusyId, setRecoveryBusyId] = useState<string | null>(null);

  const refreshRecoveryPoints = useCallback(async () => {
    try { setRecoveryPoints(await listRecoveryPoints()); }
    catch (error) { showFailure('Settings.ListRecoveryPoints', '读取自动恢复点', error, setRecoveryStatus); }
  }, [showFailure]);

  const handleToggleRecoveryRetention = useCallback(async (point: RecoveryPoint) => {
    setRecoveryBusyId(point.id);
    try {
      await setRecoveryPointRetained(point.id, !point.retained);
      setRecoveryStatus(point.retained ? '已取消手工保留。' : '已标记为手工保留，不会自动轮转删除。');
      await refreshRecoveryPoints();
    } catch (error) { showFailure('Settings.RetainRecoveryPoint', '更新恢复点保留状态', error, setRecoveryStatus); }
    finally { setRecoveryBusyId(null); }
  }, [refreshRecoveryPoints, showFailure]);

  const handleDeleteRecoveryPoint = useCallback(async (point: RecoveryPoint) => {
    if (!confirm(`确定删除 ${new Date(point.createdAt).toLocaleString('zh-CN')} 的恢复点吗？此文件删除后无法恢复。`)) return;
    setRecoveryBusyId(point.id);
    try {
      await deleteRecoveryPoint(point.id);
      setRecoveryStatus('恢复点已删除。');
      await refreshRecoveryPoints();
    } catch (error) { showFailure('Settings.DeleteRecoveryPoint', '删除恢复点', error, setRecoveryStatus); }
    finally { setRecoveryBusyId(null); }
  }, [refreshRecoveryPoints, showFailure]);

  const handleOpenBackupDirectory = useCallback(async () => {
    try { await openBackupDirectory(); }
    catch (error) { showFailure('Settings.OpenBackupDirectory', '打开备份目录', error, setRecoveryStatus); }
  }, [showFailure]);

  const handleRestoreRecoveryPoint = useCallback(async (point: RecoveryPoint) => {
    const preview = [
      `恢复点：${new Date(point.createdAt).toLocaleString('zh-CN')}（${RECOVERY_REASON_LABELS[point.reason]}）`,
      `数据库版本：当前 V18 → 快照 V${point.databaseVersion}`,
      `记录数量：当前 ${records.length} → 快照 ${point.recordCount}`,
      '', '恢复会替换整个本地数据库；程序会先自动保存当前数据库。确定继续吗？',
    ].join('\n');
    if (!confirm(preview)) return;
    setRecoveryBusyId(point.id);
    setRecoveryStatus('正在验证并恢复数据库...');
    try {
      const result = await restoreRecoveryPoint(point.id);
      await onDatabaseRestored();
      await refreshRecoveryPoints();
      setRecoveryStatus(`✅ 已恢复 ${result.recordCount} 条记录；恢复前状态也已保存。`);
      showSuccess('数据库恢复完成。');
    } catch (error) { showFailure('Settings.RestoreRecoveryPoint', '恢复数据库', error, setRecoveryStatus); }
    finally { setRecoveryBusyId(null); }
  }, [onDatabaseRestored, records.length, refreshRecoveryPoints, showFailure, showSuccess]);

  return { recoveryPoints, recoveryStatus, recoveryBusyId, refreshRecoveryPoints, handleToggleRecoveryRetention, handleDeleteRecoveryPoint, handleRestoreRecoveryPoint, handleOpenBackupDirectory };
}
