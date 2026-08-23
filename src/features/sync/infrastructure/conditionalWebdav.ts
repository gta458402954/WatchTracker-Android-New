import { entityTag, entityTagKind, firstUsableEntityTag, normalizedEntityTag } from '../domain/entityTags.ts';
import type { WebDAVCreds, WebDavResponse, WebDavTransport } from './webdavTransport.ts';

export interface ConditionalValidator { etag: string; header: 'if-match' | 'dav-if'; }

function successful(status: number) { return status >= 200 && status < 300; }

/** Stable content fingerprint shared by sync and legacy import services. */
export async function contentFingerprint(value: unknown): Promise<string> {
  const bytes = new TextEncoder().encode(JSON.stringify(value));
  const digest = await crypto.subtle.digest('SHA-256', bytes);
  return [...new Uint8Array(digest)].map(byte => byte.toString(16).padStart(2, '0')).join('');
}

function davEtagFromPropfind(text: string | null): string | null {
  if (!text) return null;
  const document = new DOMParser().parseFromString(text, 'application/xml');
  if (document.querySelector('parsererror')) return null;
  return firstUsableEntityTag(
    Array.from(document.getElementsByTagNameNS('*', 'getetag'), element => element.textContent),
  );
}

/** Resolves a safe strong/weak validator without deciding upload or merge policy. */
export async function conditionalValidatorForResource(
  response: WebDavResponse,
  creds: WebDAVCreds,
  proxy: string | null,
  resource: string,
  transport: WebDavTransport,
): Promise<ConditionalValidator> {
  const rawResponseEtag = response.etag?.trim() ?? null;
  const responseEtag = normalizedEntityTag(rawResponseEtag);
  if (rawResponseEtag && entityTagKind(rawResponseEtag) === 'strong') {
    return { etag: rawResponseEtag, header: 'if-match' };
  }
  const properties = await transport.request('PROPFIND', creds, proxy, resource);
  const propertyEtag = successful(properties.status) ? davEtagFromPropfind(properties.text) : null;
  if (propertyEtag) return { etag: propertyEtag, header: 'dav-if' };
  if (responseEtag) return { etag: responseEtag, header: 'dav-if' };
  throw new Error('conditional_write_unsupported');
}

export function assertEntityTag(value: string | null): asserts value is string {
  if (!entityTag(value)) throw new Error('conditional_write_unsupported');
}
