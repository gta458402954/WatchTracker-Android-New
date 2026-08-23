import { useEffect, useRef, useState } from 'react';
import type { WatchRecord } from '../../../shared/types';
import { clearResolvedSyncConflicts, getCreds, type SyncConflict } from '../../../shared/lib/webdav';
import { getSettingAsync, getSyncTargets, getTmdbCredentialStatus, type SyncTargetRegistry } from '../../../shared/lib/database';
import { BATCH_METADATA_STATE_KEY, parseBatchMetadataNoDataState, pruneBatchMetadataNoDataState, type BatchMetadataNoDataState } from '../../../shared/lib/batchMetadata';
import type { NoticeTone } from '../../../shared/lib/feedback';

interface UseSettingsBootstrapOptions {
  records: WatchRecord[];
  onNotify?: (tone: NoticeTone, message: string) => void;
  showFailure: (scope: string, action: string, error: unknown, setStatus?: (message: string) => void) => void;
}

export function useSettingsBootstrap({ records, onNotify, showFailure }: UseSettingsBootstrapOptions) {
  const [username, setUsername] = useState('');
  const [password, setPassword] = useState('');
  const [webdavUrl, setWebdavUrl] = useState('https://dav.jianguoyun.com/dav/影视追踪/');
  const [saved, setSaved] = useState(false);
  const [editingTarget, setEditingTarget] = useState(false);
  const [targetRegistry, setTargetRegistry] = useState<SyncTargetRegistry | null>(null);
  const [syncStatus, setSyncStatus] = useState('');
  const [proxy, setProxy] = useState('');
  const [tmdbKey, setTmdbKey] = useState('');
  const [tmdbSaved, setTmdbSaved] = useState(false);
  const [batchNoDataState, setBatchNoDataState] = useState<BatchMetadataNoDataState>({ version: 1, records: {} });
  const [syncConflicts, setSyncConflicts] = useState<SyncConflict[]>([]);
  const recordsRef = useRef(records);

  useEffect(() => { recordsRef.current = records; }, [records]);
  useEffect(() => {
    async function loadInitial() {
      try {
        const creds = await getCreds();
        setSaved(!!creds);
        if (creds?.url) { setWebdavUrl(creds.url); setUsername(creds.username); }
        try { setTargetRegistry(await getSyncTargets()); } catch (error) {
          if (String(error).includes('target_migration_required')) setSyncStatus('⚠️ 旧版 WebDAV 凭据无法安全迁移，请重新输入账号后连接；本地数据仍可正常使用。');
          else throw error;
        }
        const tmdbStatus = await getTmdbCredentialStatus();
        setTmdbSaved(tmdbStatus.available);
        if (tmdbStatus.state === 'reentry-required') onNotify?.('warning', 'TMDB 密钥需要在当前 Windows 用户下重新输入。');
        const savedProxy = await getSettingAsync('network_proxy');
        if (savedProxy) setProxy(savedProxy);
        setBatchNoDataState(pruneBatchMetadataNoDataState(parseBatchMetadataNoDataState(await getSettingAsync(BATCH_METADATA_STATE_KEY)), recordsRef.current));
      } catch (error) { showFailure('Settings.Initialize', '读取设置', error, setSyncStatus); }
    }
    void loadInitial();
  }, [onNotify, showFailure]);
  useEffect(() => {
    let cancelled = false;
    clearResolvedSyncConflicts(records)
      .then(conflicts => { if (!cancelled) setSyncConflicts(conflicts); })
      .catch(error => showFailure('Settings.LoadConflicts', '读取同步冲突', error));
    return () => { cancelled = true; };
  }, [records, showFailure]);

  return { username, setUsername, password, setPassword, webdavUrl, setWebdavUrl, saved, setSaved, editingTarget, setEditingTarget, targetRegistry, setTargetRegistry, syncStatus, setSyncStatus, proxy, setProxy, tmdbKey, setTmdbKey, tmdbSaved, setTmdbSaved, batchNoDataState, setBatchNoDataState, syncConflicts, setSyncConflicts };
}
