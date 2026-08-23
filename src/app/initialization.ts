import { parsePullInterval } from '../shared/lib/syncScheduling.ts';

export interface InitializationDependencies {
  readCredentials: () => Promise<boolean>;
  readSyncInterval: () => Promise<string | null>;
  readPullInterval: () => Promise<string | null>;
  readRecords: () => Promise<unknown>;
}

export interface InitialAppData {
  hasWebDAVCredentials: boolean;
  syncInterval: number;
  pullIntervalMinutes: number;
}

export function parseSyncInterval(value: string | null, fallback = 30): number {
  if (value === null) return fallback;
  const parsed = Number.parseInt(value, 10);
  return Number.isFinite(parsed) && parsed >= 5 && parsed <= 300 ? parsed : fallback;
}

export async function initializeApp(
  dependencies: InitializationDependencies,
): Promise<InitialAppData> {
  // Credentials/settings are optional startup enhancements.  A missing
  // Android secret-store adapter must never prevent the local database from
  // becoming usable offline.
  const readOptional = async <T>(read: () => Promise<T>, fallback: T): Promise<T> => {
    try {
      return await read();
    } catch {
      return fallback;
    }
  };
  const hasWebDAVCredentials = await readOptional(dependencies.readCredentials, false);
  const savedInterval = await readOptional(dependencies.readSyncInterval, null);
  const savedPullInterval = await readOptional(dependencies.readPullInterval, null);
  await dependencies.readRecords();

  return {
    hasWebDAVCredentials,
    syncInterval: parseSyncInterval(savedInterval),
    pullIntervalMinutes: parsePullInterval(savedPullInterval),
  };
}
