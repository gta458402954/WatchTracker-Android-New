import {
  activateSyncTarget,
  disconnectSyncTarget,
  getActiveSyncConnection,
} from '../../../shared/lib/database.ts';
import type { WebDAVCreds } from './webdavTransport.ts';

const DEFAULT_WEBDAV_BASE_URL = 'https://dav.jianguoyun.com/dav/%E5%BD%B1%E8%A7%86%E8%BF%BD%E8%B8%AA/';

export function normalizeSyncTargetUrl(raw: string): string {
  const url = new URL(raw.trim());
  if (!['http:', 'https:'].includes(url.protocol)) throw new Error('invalid_sync_target_url');
  url.username = ''; url.password = ''; url.search = ''; url.hash = '';
  return `${url.toString().replace(/\/+$/, '')}/`;
}

export async function saveCreds(creds: WebDAVCreds & { password: string }) {
  await activateSyncTarget({ url: creds.url || DEFAULT_WEBDAV_BASE_URL, username: creds.username, password: creds.password });
}

export async function getCreds(): Promise<WebDAVCreds | null> {
  try {
    const active = await getActiveSyncConnection();
    return active?.credentialAvailable ? { ...active } : null;
  } catch (error) {
    const message = String(error);
    if (['target_migration_required', 'credential_reentry_required', 'credential_missing', 'credential_store_unavailable', 'credential_store_unsupported']
      .some(code => message.includes(code))) return null;
    throw error;
  }
}

export async function clearCreds() { await disconnectSyncTarget(); }
export async function hasCreds(): Promise<boolean> { return !!(await getCreds()); }

export type { WebDAVCreds } from './webdavTransport.ts';
