import { expect, test, type Page } from '@playwright/test';
import type { WatchRecord } from '../src/shared/types';
import type { SyncPayloadV3 } from '../src/shared/lib/syncMerge';
import { mockSnapshot, setupMockIpc } from './fixtures/mockIpc';

function record(id: string, overrides: Partial<WatchRecord> = {}): WatchRecord {
  return {
    id,
    originalName: `${id} original`,
    chineseName: id,
    progress: '',
    totalEpisodes: null,
    movieProgress: null,
    movieDuration: null,
    releaseYear: '2026',
    posterPath: null,
    status: '未看',
    platform: '',
    rating: null,
    startDate: '',
    endDate: '',
    notes: '',
    createdAt: '2026-01-01T00:00:00.000Z',
    updatedAt: '2026-01-01T00:00:00.000Z',
    imdbId: null,
    mediaType: '电影',
    contentTags: null,
    originCountry: null,
    rev: 1,
    revActor: 'fixture',
    ...overrides,
  };
}

function payload(records: WatchRecord[]): SyncPayloadV3 {
  return {
    schemaVersion: 3,
    documentId: 'sync-reliability-document',
    revision: 1,
    commitId: 'sync-reliability-commit',
    parentCommitId: null,
    writerId: 'remote-device',
    committedAt: '2026-08-02T00:00:00.000Z',
    records,
    tombstones: [],
  };
}

const credentials = {
  webdav_creds: 'encrypted:user:password',
  webdav_url: 'https://example.test/watchtracker/',
};

async function webdavCallCount(page: Page): Promise<number> {
  return (await mockSnapshot(page)).calls.filter(call => call.command === 'webdav_request').length;
}

test('@expected-sync-v3 durable outbox survives reload and resumes without losing the edit', async ({ page }) => {
  const original = record('崩溃恢复条目');
  await setupMockIpc(page, {
    records: [original],
    settings: {
      ...credentials,
      sync_interval: '300',
      sync_scheduler_v1: JSON.stringify({
        version: 1, paused: true, consecutiveFailures: 0, nextAttemptAt: null,
        lastAttemptAt: null, lastSuccessAt: null, lastErrorCode: null, lastRemoteCheckAt: null,
      }),
    },
  });
  await page.goto('/');
  await expect(page.getByText('崩溃恢复条目').first()).toBeVisible();

  await page.evaluate(async () => {
    await window.__TAURI_INTERNALS__.invoke('update_record', {
      id: '崩溃恢复条目', updates: { notes: '退出前已经提交到 SQLite' },
    });
  });
  expect(JSON.parse((await mockSnapshot(page)).settings.sync_outbox_v1 as string).pending).toBe(true);

  await page.reload();
  await expect(page.getByRole('button', { name: /云端同步：已暂停/ })).toBeVisible();
  expect(await webdavCallCount(page)).toBe(0);
  await page.getByRole('button', { name: /云端同步：已暂停/ }).click();
  await page.getByRole('button', { name: '恢复自动同步' }).click();

  await expect.poll(async () => JSON.parse((await mockSnapshot(page)).settings.sync_outbox_v1 as string).pending).toBe(false);
  const snapshot = await mockSnapshot(page);
  expect(snapshot.records[0].notes).toBe('退出前已经提交到 SQLite');
  expect(snapshot.calls.some(call => call.command === 'webdav_request' && call.args.method === 'PUT')).toBe(true);
});

test('@expected-sync-v3 focus actively pulls a remote-only record while the local outbox is clean', async ({ page }) => {
  const local = record('本地已有');
  const remoteOnly = record('其他设备新增');
  const baseline = payload([local]);
  await setupMockIpc(page, {
    records: [local],
    webdavV3Remote: payload([local, remoteOnly]),
    webdavV3Etag: '"focus-etag"',
    settings: {
      ...credentials,
      sync_v3_baseline: JSON.stringify(baseline),
      sync_outbox_v1: JSON.stringify({
        version: 1, pending: false, dirtyGeneration: 0, reasons: [], firstQueuedAt: null, lastQueuedAt: null,
      }),
    },
  });
  await page.goto('/');
  await expect(page.getByText('本地已有').first()).toBeVisible();
  await page.evaluate(() => window.dispatchEvent(new Event('focus')));
  await expect(page.getByText('其他设备新增').first()).toBeVisible();

  const snapshot = await mockSnapshot(page);
  expect(snapshot.calls.some(call => call.command === 'commit_sync_result')).toBe(true);
  expect(JSON.parse(snapshot.settings.sync_outbox_v1 as string).pending).toBe(false);
});

test('@expected-sync-v3 online recovery actively pulls after connectivity returns', async ({ page }) => {
  const local = record('联网前本地条目');
  const remoteOnly = record('联网恢复新增');
  const baseline = payload([local]);
  await setupMockIpc(page, {
    records: [local],
    webdavV3Remote: payload([local, remoteOnly]),
    webdavV3Etag: '"online-etag"',
    settings: {
      ...credentials,
      sync_v3_baseline: JSON.stringify(baseline),
      sync_outbox_v1: JSON.stringify({
        version: 1, pending: false, dirtyGeneration: 0, reasons: [], firstQueuedAt: null, lastQueuedAt: null,
      }),
    },
  });
  await page.goto('/');
  await expect(page.getByText('联网前本地条目').first()).toBeVisible();
  await page.evaluate(() => window.dispatchEvent(new Event('online')));
  await expect(page.getByText('联网恢复新增').first()).toBeVisible();

  const snapshot = await mockSnapshot(page);
  expect(snapshot.calls.some(call => call.command === 'commit_sync_result')).toBe(true);
  expect(JSON.parse(snapshot.settings.sync_outbox_v1 as string).pending).toBe(false);
});

test('@expected-sync-v3 transient startup failure persists backoff and avoids a request loop', async ({ page }) => {
  await setupMockIpc(page, {
    records: [record('等待重试')],
    webdavFailureStatus: 503,
    webdavFailureCount: 1,
    settings: {
      ...credentials,
      sync_outbox_v1: JSON.stringify({
        version: 1, pending: true, dirtyGeneration: 0, reasons: ['record-update'],
        firstQueuedAt: '2026-08-02T00:00:00.000Z', lastQueuedAt: '2026-08-02T00:00:00.000Z',
      }),
    },
  });
  await page.goto('/');
  await expect.poll(async () => JSON.parse((await mockSnapshot(page)).settings.sync_scheduler_v1 as string).lastErrorCode).toBe('http_503');
  const runtime = JSON.parse((await mockSnapshot(page)).settings.sync_scheduler_v1 as string);
  expect(runtime.nextAttemptAt).not.toBeNull();
  const count = await webdavCallCount(page);
  await page.waitForTimeout(700);
  expect(await webdavCallCount(page)).toBe(count);
});

test('@expected-sync-v3 authentication blocker keeps the outbox and does not auto-retry', async ({ page }) => {
  await setupMockIpc(page, {
    records: [record('认证阻断')],
    webdavFailureStatus: 401,
    webdavFailureCount: 1,
    settings: {
      ...credentials,
      sync_outbox_v1: JSON.stringify({
        version: 1, pending: true, dirtyGeneration: 0, reasons: ['record-update'],
        firstQueuedAt: '2026-08-02T00:00:00.000Z', lastQueuedAt: '2026-08-02T00:00:00.000Z',
      }),
    },
  });
  await page.goto('/');
  await expect.poll(async () => JSON.parse((await mockSnapshot(page)).settings.sync_scheduler_v1 as string).lastErrorCode).toBe('http_401');
  const snapshot = await mockSnapshot(page);
  expect(JSON.parse(snapshot.settings.sync_outbox_v1 as string).pending).toBe(true);
  expect(JSON.parse(snapshot.settings.sync_scheduler_v1 as string).nextAttemptAt).toBeNull();
  const count = await webdavCallCount(page);
  await page.waitForTimeout(700);
  expect(await webdavCallCount(page)).toBe(count);
});

test('@expected-sync-v3 startup retries a persisted conditional-write blocker through DAV getetag', async ({ page }) => {
  const remote = payload([record('升级后恢复同步')]);
  const pending = record('升级后恢复同步', {
    notes: '本地待上传修改',
    updatedAt: '2026-08-02T00:01:00.000Z',
    rev: 2,
    revActor: 'local-device',
  });
  await setupMockIpc(page, {
    records: [pending],
    webdavV3Remote: remote,
    webdavV3Etag: '"upgrade-etag"',
    omitGetEtag: true,
    settings: {
      ...credentials,
      sync_v3_baseline: JSON.stringify(remote),
      sync_outbox_v1: JSON.stringify({
        version: 1, pending: true, dirtyGeneration: 97, reasons: ['record-update'],
        firstQueuedAt: '2026-08-02T00:00:00.000Z', lastQueuedAt: '2026-08-02T00:00:00.000Z',
      }),
      sync_scheduler_v1: JSON.stringify({
        version: 1, paused: false, consecutiveFailures: 2, nextAttemptAt: null,
        lastAttemptAt: '2026-08-02T00:00:00.000Z', lastSuccessAt: null,
        lastErrorCode: 'conditional_write_unsupported', lastRemoteCheckAt: null,
      }),
    },
  });

  await page.goto('/');
  await expect.poll(async () => JSON.parse((await mockSnapshot(page)).settings.sync_outbox_v1 as string).pending).toBe(false);

  const snapshot = await mockSnapshot(page);
  const runtime = JSON.parse(snapshot.settings.sync_scheduler_v1 as string);
  expect(runtime.lastErrorCode).toBeNull();
  expect(snapshot.calls.some(call => call.command === 'webdav_request' && call.args.method === 'PROPFIND')).toBe(true);
  expect(snapshot.calls.some(call => call.command === 'webdav_request' && call.args.method === 'PUT')).toBe(true);
});
