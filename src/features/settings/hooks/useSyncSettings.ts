import { useState } from 'react';
import type { WatchRecord } from '../../../shared/types';
import { clearCreds, clearResolvedSyncConflicts, getCreds, importLegacyChangesToConflictCenter, loadFromWebDAV, normalizeSyncTargetUrl, probeSyncTarget, saveCreds, syncFailureMessage, syncToWebDAV, type SyncConflict } from '../../../shared/lib/webdav';
import { getSyncSnapshot, getSyncTargets, resolveSyncConflict, setSettingAsync, type SyncTargetRegistry } from '../../../shared/lib/database';

interface Options {
  records: WatchRecord[]; onSync?: () => Promise<{ ok: boolean; error?: string; conflictCount?: number }>;
  onImport: (records: WatchRecord[]) => void | Promise<void>; onDatabaseRestored: () => Promise<WatchRecord[]>;
  onNotify?: (tone: 'info' | 'success' | 'warning' | 'error', message: string) => void;
  showFailure: (scope: string, action: string, error: unknown, setStatus?: (message: string) => void) => void;
  showSuccess: (message: string) => void;
  username: string; setUsername: (value: string) => void; password: string; setPassword: (value: string) => void;
  webdavUrl: string; setWebdavUrl: (value: string) => void; saved: boolean; setSaved: (value: boolean) => void;
  editingTarget: boolean; setEditingTarget: (value: boolean) => void; targetRegistry: SyncTargetRegistry | null; setTargetRegistry: (value: SyncTargetRegistry) => void;
  syncStatus: string; setSyncStatus: (value: string) => void; syncConflicts: SyncConflict[]; setSyncConflicts: (value: SyncConflict[]) => void;
  syncInterval: number; onSyncIntervalChange: (value: number) => void; pullIntervalMinutes: number; onPullIntervalChange: (value: number) => void;
}

export function useSyncSettings(options: Options) {
  const { records, onSync, onImport, onDatabaseRestored, onNotify, showFailure, showSuccess,
    username, setUsername, password, setPassword, webdavUrl, setSaved, setEditingTarget,
    targetRegistry, setTargetRegistry, setSyncStatus, setSyncConflicts, syncInterval, onSyncIntervalChange, pullIntervalMinutes, onPullIntervalChange } = options;
  const [localInterval, setLocalInterval] = useState(syncInterval);
  const [localPullInterval, setLocalPullInterval] = useState(pullIntervalMinutes);
  const [importStatus, setImportStatus] = useState('');

  async function handleSave() {
    if (!username.trim() || !password.trim() || !webdavUrl.trim()) return;
    try {
      const normalizedUrl = normalizeSyncTargetUrl(webdavUrl); const current = await getCreds();
      const activeDescriptor = targetRegistry?.targets.find(target => target.id === targetRegistry.activeTargetId);
      const identityChanged = !current || activeDescriptor?.username !== username.trim() || activeDescriptor?.normalizedUrl !== normalizedUrl;
      if (identityChanged) {
        setSyncStatus('正在只读检查目标...'); const probe = await probeSyncTarget({ username: username.trim(), password, url: normalizedUrl });
        const description = probe.kind === 'empty' ? '目标中暂无同步文件，将在激活后以安全条件创建并上传本机数据。' : `目标包含 ${probe.recordCount} 条记录${probe.revision == null ? '' : `，修订 ${probe.revision}`}。激活后将先拉取、合并，再按需上传。`;
        if (!confirm(`${description}\n\n确认切换到此 WebDAV 目标吗？旧目标的待上传数据和冲突会保留。`)) { setSyncStatus('已取消切换，当前目标未改变。'); return; }
      }
      await saveCreds({ username: username.trim(), password: password.trim(), url: normalizedUrl }); setSaved(true); setEditingTarget(false); setPassword('');
      setTargetRegistry(await getSyncTargets()); setSyncStatus('目标已激活，正在执行首次拉取与合并...'); const result = onSync ? await onSync() : await syncToWebDAV(records);
      if (!result.ok) { const safeMessage = syncFailureMessage(result.error); setSyncStatus(`⚠️ 目标已保存；首次同步未完成。${safeMessage || '请稍后重试。'}`); onNotify?.('warning', safeMessage || '目标已保存，但首次同步未完成。'); }
      else { setSyncStatus(result.conflictCount ? `⚠️ 目标已激活，有 ${result.conflictCount} 项冲突等待选择` : '✅ 目标已激活并完成首次同步'); showSuccess('WebDAV 目标已激活并完成首次同步。'); }
    } catch (error) {
      const safeMessage = syncFailureMessage(error instanceof Error ? error.message : String(error));
      if (safeMessage) { setSyncStatus(`⚠️ ${safeMessage}`); onNotify?.('warning', safeMessage); }
      else showFailure('Settings.SaveWebDav', '保存 WebDAV 凭据', error, setSyncStatus);
    }
    setTimeout(() => setSyncStatus(''), 3000);
  }
  async function handleClear() {
    try { await clearCreds(); setUsername(''); setPassword(''); setSaved(false); setEditingTarget(false); setTargetRegistry(await getSyncTargets()); setSyncStatus('🧹 凭据已清除'); showSuccess('WebDAV 凭据已清除。'); }
    catch (error) { showFailure('Settings.ClearWebDav', '清除 WebDAV 凭据', error, setSyncStatus); }
    setTimeout(() => setSyncStatus(''), 3000);
  }
  async function handleSync() {
    setSyncStatus('正在同步...');
    try { const result = onSync ? await onSync() : await syncToWebDAV(records); if (result.ok) { setSyncConflicts(await clearResolvedSyncConflicts(records)); setSyncStatus(result.conflictCount ? `⚠️ 云端核对完成，有 ${result.conflictCount} 项冲突等待选择` : '✅ 同步成功'); showSuccess('WebDAV 同步完成。'); } else { const safeMessage = syncFailureMessage(result.error); if (safeMessage) { setSyncStatus(`⚠️ ${safeMessage}`); onNotify?.('warning', safeMessage); } else showFailure('Settings.SyncWebDav', 'WebDAV 同步', result.error, setSyncStatus); } }
    catch (error) { showFailure('Settings.SyncWebDav', 'WebDAV 同步', error, setSyncStatus); }
    setTimeout(() => setSyncStatus(''), 3000);
  }
  async function handleResolveConflict(conflict: SyncConflict, resolution: 'local' | 'remote' | 'keep' | 'delete') {
    const title = conflict.local?.chineseName || conflict.remote?.chineseName || '未命名条目'; const action = resolution === 'local' ? '采用本机版本' : resolution === 'remote' ? '采用云端版本' : resolution === 'keep' ? '保留条目' : '确认删除';
    if (!confirm(`确定对「${title}」${action}吗？选择结果将在下次同步时发布。`)) return;
    try { const snapshot = await getSyncSnapshot(); await resolveSyncConflict(conflict.id, resolution, snapshot.targetId, snapshot.targetEpoch); await onDatabaseRestored(); setSyncConflicts(await clearResolvedSyncConflicts([])); showSuccess('同步冲突已解决，选择结果会在下次同步时发布。'); }
    catch (error) { showFailure('Settings.ResolveConflict', '解决同步冲突', error); }
  }
  async function handleImportLegacyChanges() {
    if (!confirm('此操作只会把旧版 records.json 的差异加入冲突中心，不会直接覆盖本机或 v3 云端数据。继续吗？')) return;
    setSyncStatus('正在读取旧版云端数据...');
    try { const result = await importLegacyChangesToConflictCenter(); if (!result.ok) { showFailure('Settings.ImportLegacySync', '读取旧版云端数据', result.error, setSyncStatus); return; } setSyncConflicts(await clearResolvedSyncConflicts([])); setSyncStatus(result.conflictCount ? `⚠️ 已加入 ${result.conflictCount} 项旧版差异，请在冲突中心选择。` : '✅ 旧版云端数据与 v3 无差异。'); }
    catch (error) { showFailure('Settings.ImportLegacySync', '读取旧版云端数据', error, setSyncStatus); }
  }
  async function handleImport() {
    if (!confirm('导入将替换未锁定的本地数据；已锁定记录会保留。确定吗？')) return;
    setImportStatus('正在从云端下载数据...');
    try { const response = await loadFromWebDAV(); if (response.ok && Array.isArray(response.data)) { await onImport(response.data as WatchRecord[]); setImportStatus('✅ 导入成功'); showSuccess('云端记录已导入。'); } else showFailure('Settings.ImportWebDav', '从云端导入', response.error, setImportStatus); }
    catch (error) { showFailure('Settings.ImportWebDav', '从云端导入', error, setImportStatus); }
    setTimeout(() => setImportStatus(''), 3000);
  }
  async function handleSaveInterval() { try { await setSettingAsync('sync_interval', localInterval.toString()); onSyncIntervalChange(localInterval); setSyncStatus('✅ 自动同步频率已更新'); showSuccess('自动同步频率已更新。'); } catch (error) { showFailure('Settings.SaveSyncInterval', '保存同步频率', error, setSyncStatus); } setTimeout(() => setSyncStatus(''), 3000); }
  async function handleSavePullInterval() { try { await setSettingAsync('sync_pull_interval_minutes', localPullInterval.toString()); onPullIntervalChange(localPullInterval); setSyncStatus('✅ 主动拉取周期已更新'); showSuccess('主动拉取周期已更新。'); } catch (error) { showFailure('Settings.SavePullInterval', '保存主动拉取周期', error, setSyncStatus); } setTimeout(() => setSyncStatus(''), 3000); }
  return { importStatus, localInterval, setLocalInterval, localPullInterval, setLocalPullInterval, handleSave, handleClear, handleSync, handleResolveConflict, handleImportLegacyChanges, handleImport, handleSaveInterval, handleSavePullInterval };
}
