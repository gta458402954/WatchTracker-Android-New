export type RuntimeKind = 'android' | 'tauri-desktop' | 'web';

export interface RuntimeProbe {
  userAgent?: string | null;
  hasTauri?: boolean;
}

/**
 * Keep platform selection in one small, pure module. A narrow Android check
 * avoids sending a desktop-width heuristic into the product boundary and
 * keeps the browser/Playwright compatibility shell deterministic.
 */
export function detectRuntime({ userAgent = '', hasTauri = false }: RuntimeProbe): RuntimeKind {
  if (hasTauri && /Android/i.test(userAgent ?? '')) return 'android';
  if (hasTauri) return 'tauri-desktop';
  return 'web';
}

export function hasTauriRuntime(scope: typeof globalThis = globalThis): boolean {
  const candidate = scope as typeof globalThis & {
    __TAURI_INTERNALS__?: unknown;
    __TAURI__?: unknown;
  };
  return Boolean(candidate.__TAURI_INTERNALS__ || candidate.__TAURI__);
}

export function currentRuntime(): RuntimeKind {
  const userAgent = typeof navigator === 'undefined' ? '' : navigator.userAgent;
  return detectRuntime({ userAgent, hasTauri: hasTauriRuntime() });
}

export function isAndroidRuntime(): boolean {
  return currentRuntime() === 'android';
}
