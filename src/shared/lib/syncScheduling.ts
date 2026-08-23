export type SyncFailureDisposition = 'retry' | 'stale-local' | 'blocked';

const RETRY_DELAYS_MS = [10_000, 30_000, 120_000, 300_000, 900_000] as const;
export const FOCUS_PULL_COOLDOWN_MS = 30_000;
export const DEFAULT_PULL_INTERVAL_MINUTES = 15;
export const PULL_INTERVAL_OPTIONS = [0, 5, 15, 30, 60] as const;

export function parsePullInterval(value: string | null, fallback = DEFAULT_PULL_INTERVAL_MINUTES): number {
  if (value === null) return fallback;
  if (!/^\d+$/.test(value.trim())) return fallback;
  const parsed = Number.parseInt(value, 10);
  return (PULL_INTERVAL_OPTIONS as readonly number[]).includes(parsed) ? parsed : fallback;
}

export function classifySyncFailure(error?: string): SyncFailureDisposition {
  const value = error ?? '';
  if (value.includes('stale_local_snapshot')) return 'stale-local';
  if (
    value.includes('conditional_write_unsupported')
    || value.includes('conditional_validator_rejected')
    || value.includes('unsupported_remote_schema')
    || value.includes('legacy_remote_changed')
    || value.includes('未配置凭据')
    || /HTTP Error:\s*(401|403)\b/.test(value)
  ) return 'blocked';
  return 'retry';
}

export function retryDelayMs(failureCount: number, jitterFraction = 0): number {
  const index = Math.max(0, Math.min(RETRY_DELAYS_MS.length - 1, Math.trunc(failureCount) - 1));
  const boundedJitter = Math.max(-0.2, Math.min(0.2, jitterFraction));
  return Math.round(RETRY_DELAYS_MS[index] * (1 + boundedJitter));
}

export function nextRetryAt(failureCount: number, nowMs: number, jitterFraction = 0): string {
  return new Date(nowMs + retryDelayMs(failureCount, jitterFraction)).toISOString();
}

export function isDue(iso: string | null | undefined, nowMs: number): boolean {
  if (!iso) return true;
  const value = Date.parse(iso);
  return !Number.isFinite(value) || value <= nowMs;
}

export function focusPullDue(lastRemoteCheckAt: string | null | undefined, nowMs: number): boolean {
  if (!lastRemoteCheckAt) return true;
  const checked = Date.parse(lastRemoteCheckAt);
  return !Number.isFinite(checked) || nowMs - checked >= FOCUS_PULL_COOLDOWN_MS || checked > nowMs;
}

export function periodicPullDue(
  lastRemoteCheckAt: string | null | undefined,
  intervalMinutes: number,
  nowMs: number,
): boolean {
  if (intervalMinutes === 0) return false;
  if (!lastRemoteCheckAt) return true;
  const checked = Date.parse(lastRemoteCheckAt);
  return !Number.isFinite(checked) || nowMs - checked >= intervalMinutes * 60_000 || checked > nowMs;
}
