export interface WebDavTargetDisplay {
  provider: string | null;
  host: string;
  path: string;
  summary: string;
  safeUrl: string;
}

function withoutSensitiveUrlParts(raw: string): string {
  return raw.trim()
    .replace(/([a-z][a-z\d+.-]*:\/\/)[^/@\s]+@/i, '$1')
    .split(/[?#]/, 1)[0]
    .replace(/\/+$/, '');
}

function decodedPath(pathname: string): string {
  try {
    return decodeURIComponent(pathname);
  } catch {
    return pathname;
  }
}

function cleanPath(pathname: string): string {
  const trimmed = pathname.replace(/\/+$/, '');
  return trimmed || '/';
}

/** Formats a WebDAV target for display without exposing credentials, queries, or fragments. */
export function formatWebDavTargetUrl(raw: string): WebDavTargetDisplay {
  try {
    const url = new URL(raw.trim());
    const isJianguo = url.hostname.toLowerCase() === 'dav.jianguoyun.com';
    const provider = isJianguo ? '坚果云' : null;
    const fullPath = cleanPath(decodedPath(url.pathname));
    const path = isJianguo
      ? cleanPath(fullPath.replace(/^\/dav(?=\/|$)/i, ''))
      : fullPath;
    const host = url.host;
    const label = provider ?? host;
    const safeUrl = `${url.protocol}//${host}${fullPath}`;
    return {
      provider,
      host,
      path,
      summary: path === '/' ? label : `${label} · ${path}`,
      safeUrl,
    };
  } catch {
    const fallback = withoutSensitiveUrlParts(raw) || '无效的 WebDAV 地址';
    return { provider: null, host: '', path: '', summary: fallback, safeUrl: fallback };
  }
}
