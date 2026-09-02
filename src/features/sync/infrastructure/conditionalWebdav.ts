import { entityTag, entityTagKind, firstUsableEntityTag, normalizedEntityTag } from '../domain/entityTags.ts';
import type { WebDAVCreds, WebDavResponse, WebDavTransport } from './webdavTransport.ts';

export interface ConditionalValidator { etag: string; header: 'if-match' | 'dav-if'; }
export interface BoundResourceRead { response: WebDavResponse; validator: ConditionalValidator | null; }

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

/**
 * Reads only DAV:getetag for a resource. A missing/unsupported/malformed
 * response is deliberately represented as null so callers can choose a safe
 * fallback without coupling this transport helper to sync policy.
 */
export async function probeDavEntityTagForResource(
  creds: WebDAVCreds,
  proxy: string | null,
  resource: string,
  transport: WebDavTransport,
): Promise<string | null> {
  try {
    const properties = await transport.request('PROPFIND', creds, proxy, resource);
    return successful(properties.status) ? davEtagFromPropfind(properties.text) : null;
  } catch {
    return null;
  }
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
    return { etag: responseEtag!, header: 'if-match' };
  }
  const propertyEtag = await probeDavEntityTagForResource(creds, proxy, resource, transport);
  if (responseEtag && propertyEtag === responseEtag) return { etag: responseEtag, header: 'dav-if' };
  if (responseEtag && propertyEtag && propertyEtag !== responseEtag) throw new Error('remote_busy');
  throw new Error('conditional_write_unsupported');
}

/**
 * Binds a full GET body to a validator observed for that exact representation.
 * A strong response ETag is self-binding. Weak/unquoted response ETags require
 * a matching post-read DAV observation. When GET has no ETag, two equal DAV
 * observations separated by a fresh full GET are required. Bodies from an
 * unstable observation are discarded and never returned to merge or upload.
 */
export async function readResourceWithBoundValidator(
  creds: WebDAVCreds,
  proxy: string | null,
  resource: string,
  transport: WebDavTransport,
  initialResponse?: WebDavResponse,
  maxAttempts = 3,
): Promise<BoundResourceRead> {
  let response = initialResponse;
  let previousDavObservation: string | null = null;

  for (let attempt = 0; attempt < maxAttempts; attempt++) {
    if (!response) response = await transport.request('GET', creds, proxy, resource);
    if (response.status !== 200) return { response, validator: null };

    const rawResponseEtag = response.etag?.trim() ?? null;
    const responseEtag = normalizedEntityTag(rawResponseEtag);
    if (rawResponseEtag && responseEtag && entityTagKind(rawResponseEtag) === 'strong') {
      return { response, validator: { etag: responseEtag, header: 'if-match' } };
    }

    const postReadDavEtag = await probeDavEntityTagForResource(
      creds, proxy, resource, transport,
    );
    if (responseEtag) {
      if (postReadDavEtag === responseEtag) {
        return { response, validator: { etag: responseEtag, header: 'dav-if' } };
      }
      if (!postReadDavEtag) return { response, validator: null };
      previousDavObservation = postReadDavEtag;
      response = undefined;
      continue;
    }

    if (!postReadDavEtag) return { response, validator: null };
    if (previousDavObservation === postReadDavEtag) {
      return { response, validator: { etag: postReadDavEtag, header: 'dav-if' } };
    }
    previousDavObservation = postReadDavEtag;
    response = undefined;
  }

  throw new Error('remote_busy');
}

export function assertEntityTag(value: string | null): asserts value is string {
  if (!entityTag(value)) throw new Error('conditional_write_unsupported');
}
