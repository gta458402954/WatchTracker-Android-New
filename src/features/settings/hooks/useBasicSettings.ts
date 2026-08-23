import { clearTmdbCredential, saveTmdbCredential, setSettingAsync } from '../../../shared/lib/database';

interface Options { tmdbKey: string; setTmdbKey: (value: string) => void; setTmdbSaved: (value: boolean) => void; setSyncStatus: (value: string) => void; proxy: string; showFailure: (scope: string, action: string, error: unknown, setStatus?: (message: string) => void) => void; showSuccess: (message: string) => void; }
export function useBasicSettings({ tmdbKey, setTmdbKey, setTmdbSaved, setSyncStatus, proxy, showFailure, showSuccess }: Options) {
  async function handleSaveTmdbKey() { if (!tmdbKey.trim()) return; try { await saveTmdbCredential(tmdbKey.trim()); setTmdbKey(''); setTmdbSaved(true); setSyncStatus('✅ TMDB 密钥已保存'); showSuccess('TMDB 密钥已保存。'); setTimeout(() => setSyncStatus(''), 2000); } catch (error) { showFailure('Settings.SaveTmdbKey', '保存 TMDB 密钥', error, setSyncStatus); } }
  async function handleClearTmdbKey() { if (!confirm('确定清除已保存的 TMDB 密钥吗？')) return; try { await clearTmdbCredential(); setTmdbKey(''); setTmdbSaved(false); setSyncStatus('🧹 TMDB 密钥已清除'); showSuccess('TMDB 密钥已清除。'); setTimeout(() => setSyncStatus(''), 2000); } catch (error) { showFailure('Settings.ClearTmdbKey', '清除 TMDB 密钥', error, setSyncStatus); } }
  async function handleSaveProxy() { try { await setSettingAsync('network_proxy', proxy.trim()); setSyncStatus('✅ 代理设置已更新'); showSuccess('代理设置已更新。'); } catch (error) { showFailure('Settings.SaveProxy', '保存代理设置', error, setSyncStatus); } setTimeout(() => setSyncStatus(''), 2000); }
  return { handleSaveTmdbKey, handleClearTmdbKey, handleSaveProxy };
}
