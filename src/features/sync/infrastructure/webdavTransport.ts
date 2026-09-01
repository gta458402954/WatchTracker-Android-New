import { invoke } from '@tauri-apps/api/core';

export type WebDavMethod = 'MKCOL' | 'PUT' | 'GET' | 'PROPFIND';

export interface WebDAVCreds {
  username: string;
  password?: string;
  url?: string;
  targetId?: string;
  targetEpoch?: number;
  credentialAvailable?: boolean;
}

export interface WebDavResponse {
  status: number;
  body: unknown | null;
  etag: string | null;
  text: string | null;
  /** Raw Content-Range metadata returned by a ranged GET probe. */
  contentRange?: string | null;
  /** Number of bytes observed from a ranged GET probe (2 means at least 2). */
  rangeBodyLength?: number | null;
}

export interface WebDavTransport {
  request(
    method: WebDavMethod,
    creds: WebDAVCreds,
    proxy: string | null,
    resource: string,
    body?: string | null,
    ifMatch?: string | null,
    ifNoneMatch?: string | null,
    ifDavEtag?: string | null,
    range?: string | null,
  ): Promise<WebDavResponse>;
}

type InvokeCommand = <T>(command: string, args?: Record<string, unknown>) => Promise<T>;

/** Adapts the two existing Rust WebDAV commands without applying sync policy. */
export function createWebDavTransport(invokeCommand: InvokeCommand = invoke): WebDavTransport {
  return {
    async request(method, creds, proxy, resource, body = null, ifMatch = null, ifNoneMatch = null, ifDavEtag = null, range = null) {
      const baseUrl = creds.url?.endsWith('/') ? creds.url : `${creds.url}/`;
      const url = method === 'MKCOL' ? baseUrl : `${baseUrl}${resource}`;
      if (creds.password !== undefined) {
        return invokeCommand<WebDavResponse>('probe_webdav_request', { request: {
          method, url, username: creds.username, password: creds.password, proxy, body,
          ifMatch, ifNoneMatch, ifDavEtag, range,
        } });
      }
      if (!creds.targetId || creds.targetEpoch === undefined) throw new Error('credential_missing');
      return invokeCommand<WebDavResponse>('webdav_request', { request: {
        targetId: creds.targetId, targetEpoch: creds.targetEpoch, method, url, proxy, body,
        ifMatch, ifNoneMatch, ifDavEtag, range,
      } });
    },
  };
}

export const webdavTransport = createWebDavTransport();
