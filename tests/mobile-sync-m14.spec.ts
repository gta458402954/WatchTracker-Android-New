import { expect, test } from '@playwright/test';
import type { EpisodeCompletion, WatchRecord } from '../src/shared/types';
import type { SyncPayloadV3 } from '../src/shared/lib/syncMerge';
import { mockSnapshot, setupMockIpc } from './fixtures/mockIpc';

test.use({ userAgent: 'Mozilla/5.0 (Linux; Android 16; Pixel) AppleWebKit/537.36 Chrome/140 Mobile' });

function record(id: string, overrides: Partial<WatchRecord> = {}): WatchRecord {
  return {
    id, originalName: `${id} original`, chineseName: id, progress: '', totalEpisodes: null,
    episodeTrackingEnabled: false, nextEpisode: null, movieProgress: null, movieDuration: null,
    releaseYear: '2026', posterPath: null, status: '未看', platform: '', rating: null,
    startDate: '', endDate: '', notes: '', createdAt: '2026-08-27T00:00:00.000Z',
    updatedAt: '2026-08-27T00:00:00.000Z', imdbId: null, isLocked: false,
    mediaType: '电影', contentTags: null, originCountry: null, rev: 1, revActor: 'seed',
    ...overrides,
  };
}

function payload(records: WatchRecord[], completions: EpisodeCompletion[] = []): SyncPayloadV3 {
  return {
    schemaVersion: completions.length || records.some(item => item.episodeTrackingEnabled) ? 4 : 3,
    documentId: 'm14-document', revision: 3, commitId: 'm14-seed', parentCommitId: null,
    writerId: 'desktop-device', committedAt: '2026-08-27T00:00:00.000Z',
    records, tombstones: [], ...(completions.length || records.some(item => item.episodeTrackingEnabled) ? { episodeCompletions: completions } : {}),
  };
}

const configured = {
  webdav_creds: 'androidkeystore:v1',
  webdav_url: 'http://127.0.0.1:18144/dav/',
};

async function openSettings(page: import('@playwright/test').Page) {
  await page.getByRole('button', { name: '设置' }).click();
  await expect(page.getByRole('heading', { name: '同步设置' })).toBeVisible();
}

test('M1.4 cold-starts the local library without credentials and renders real sync settings', async ({ page }) => {
  await setupMockIpc(page, { records: [record('离线记录')] });
  await page.goto('/');
  await expect(page.getByRole('button', { name: '打开 离线记录' })).toBeVisible();
  await openSettings(page);
  await expect(page.getByTestId('mobile-sync-runtime')).toContainText('未配置');
  await expect(page.getByText('此功能正在开发中')).toHaveCount(0);
});

test('M1.4 probes before activation and never retains the password in DOM or diagnostics', async ({ page }) => {
  const secret = 'm14-password-never-render';
  await setupMockIpc(page);
  await page.goto('/'); await openSettings(page);
  await page.getByLabel('WebDAV URL').fill('http://alice:url-secret@127.0.0.1:18144/dav/?token=query-secret#fragment-secret');
  await page.getByLabel('用户名').fill('mobile-user');
  await page.getByLabel('密码').fill(secret);
  await page.getByRole('button', { name: '测试连接' }).click();
  await expect(page.getByRole('region', { name: '目标检查结果' })).toContainText(/发现旧版目标|暂无同步文件/);
  await page.getByRole('button', { name: '确认激活并首次同步' }).click();
  await expect(page.getByText(/目标已激活并完成首次同步|首次同步完成/)).toBeVisible();
  await expect(page.getByLabel('密码')).toHaveValue('');
  expect(await page.locator('body').textContent()).not.toContain(secret);
  expect(await page.locator('body').textContent()).not.toContain('query-secret');
  const snapshot = await mockSnapshot(page);
  expect(snapshot.settings.webdav_creds).toBe('androidkeystore:v1');
  expect(JSON.stringify(snapshot)).not.toContain(secret);
  expect(JSON.stringify(snapshot)).not.toContain('url-secret');
  expect(snapshot.calls.some(call => call.command === 'probe_webdav_request')).toBe(true);
  expect(snapshot.calls.some(call => call.command === 'activate_sync_target')).toBe(true);
  expect(snapshot.calls.filter(call => call.command === 'webdav_request').every(call => !('password' in call.args))).toBe(true);
});

test('M1.4 refreshes target context before persisting an initial sync failure', async ({ page }) => {
  await setupMockIpc(page, { webdavSyncFailureCount: 1, webdavFailureStatus: 503 });
  await page.goto('/'); await openSettings(page);
  await page.getByLabel('WebDAV URL').fill('http://127.0.0.1:18144/failed-first-sync/');
  await page.getByLabel('用户名').fill('new-target-user');
  await page.getByLabel('密码').fill('new-target-password');
  await page.getByRole('button', { name: '测试连接' }).click();
  await expect(page.getByRole('region', { name: '目标检查结果' })).toBeVisible();
  await page.getByRole('button', { name: '确认激活并首次同步' }).click();

  await expect(page.locator('.mobile-sync-message')).toContainText('目标已保存，但首次同步失败');
  await expect(page.getByText(/旧目标保持不变/)).toHaveCount(0);
  await expect(page.getByLabel('密码')).toHaveValue('');
  const connection = await page.evaluate(() => window.__TAURI_INTERNALS__.invoke('get_active_sync_connection')) as {
    targetId: string; targetEpoch: number; credentialAvailable: boolean;
  };
  expect(connection).toMatchObject({ credentialAvailable: true, targetEpoch: 1 });
  const snapshot = await mockSnapshot(page);
  const scheduler = JSON.parse(snapshot.settings.sync_scheduler_v1 || '{}');
  expect(scheduler).toMatchObject({ consecutiveFailures: 1, lastErrorCode: 'http_503' });
  const failureCall = snapshot.calls.find(call => call.command === 'record_sync_failure');
  expect(failureCall?.args).toMatchObject({ targetId: connection.targetId, targetEpoch: connection.targetEpoch, code: 'http_503' });
});

test('M1.4 manual sync button performs a real remote check and reports its result', async ({ page }) => {
  const local = record('手动同步记录');
  const remote = payload([local]);
  const pausedScheduler = {
    version: 1, paused: true, consecutiveFailures: 0, nextAttemptAt: null,
    lastAttemptAt: null, lastSuccessAt: null, lastErrorCode: null, lastRemoteCheckAt: null,
  };
  await setupMockIpc(page, {
    records: [local],
    settings: { ...configured, sync_v3_baseline: JSON.stringify(remote), sync_scheduler_v1: JSON.stringify(pausedScheduler) },
    webdavV3Remote: remote,
  });
  await page.goto('/'); await openSettings(page);
  const before = (await mockSnapshot(page)).calls.filter(call => call.command === 'webdav_request').length;
  await page.getByRole('button', { name: '立即同步' }).click();
  await expect(page.getByText('同步成功。', { exact: true })).toBeVisible();
  const snapshot = await mockSnapshot(page);
  const requests = snapshot.calls.filter(call => call.command === 'webdav_request');
  expect(requests.length).toBeGreaterThan(before);
  expect(requests.some(call => call.args.method === 'GET' && String(call.args.url).endsWith('records-v3.json'))).toBe(true);
});

test('M1.4 episode progress enters outbox and online trigger publishes record plus completion', async ({ page }) => {
  const series = record('跨端剧集', { mediaType: '剧集', totalEpisodes: 3, episodeTrackingEnabled: true, nextEpisode: 1, status: '在看' });
  const remote = payload([series]);
  await setupMockIpc(page, { records: [series], settings: { ...configured, sync_v3_baseline: JSON.stringify(remote), sync_v4_upgrade_confirmed: '1' }, webdavV3Remote: remote });
  await page.goto('/');
  await page.getByRole('button', { name: '完成第 1 集' }).click();
  await expect(page.getByText('第 1 集已完成，下一集为第 2 集。', { exact: true })).toBeVisible();
  await expect.poll(async () => JSON.parse((await mockSnapshot(page)).settings.sync_outbox_v1 || '{}').pending).toBe(true);
  await page.evaluate(() => window.dispatchEvent(new Event('online')));
  await expect.poll(async () => (await mockSnapshot(page)).webdavV3Remote?.episodeCompletions?.length ?? 0).toBe(1);
  const snapshot = await mockSnapshot(page);
  expect(snapshot.webdavV3Remote?.records[0]).toMatchObject({ nextEpisode: 2, rev: 2 });
  expect(snapshot.webdavV3Remote?.episodeCompletions?.[0]).toMatchObject({ recordId: series.id, episodeNumber: 1 });
  expect(JSON.parse(snapshot.settings.sync_outbox_v1 || '{}').pending).toBe(false);
});

test('M1.4 status, ordinary edit, and delete all notify the shared sync coordinator', async ({ page }) => {
  const local = record('共享调度记录');
  const remote = payload([local]);
  const pausedScheduler = {
    version: 1, paused: true, consecutiveFailures: 0, nextAttemptAt: null,
    lastAttemptAt: null, lastSuccessAt: null, lastErrorCode: null, lastRemoteCheckAt: null,
  };
  await setupMockIpc(page, {
    records: [local],
    settings: { ...configured, sync_v3_baseline: JSON.stringify(remote), sync_scheduler_v1: JSON.stringify(pausedScheduler) },
    webdavV3Remote: remote,
  });
  await page.goto('/');
  const runtimeReads = async () => (await mockSnapshot(page)).calls.filter(call => call.command === 'get_sync_runtime_state').length;

  let before = await runtimeReads();
  await page.getByRole('button', { name: '标为已看' }).click();
  await expect(page.getByText('状态已更新为“已看”。', { exact: true })).toBeVisible();
  await expect.poll(runtimeReads).toBeGreaterThan(before);

  before = await runtimeReads();
  await page.getByRole('button', { name: '编辑' }).click();
  await page.getByLabel('备注').fill('mobile ordinary edit');
  await page.getByRole('button', { name: '保存修改' }).click();
  await expect(page.getByText('记录已更新。', { exact: true })).toBeVisible();
  await expect.poll(runtimeReads).toBeGreaterThan(before);

  before = await runtimeReads();
  page.once('dialog', dialog => void dialog.accept());
  await page.getByRole('button', { name: '删除' }).click();
  await expect(page.getByText('记录已删除。', { exact: true })).toBeVisible();
  await expect.poll(runtimeReads).toBeGreaterThan(before);

  const snapshot = await mockSnapshot(page);
  const commands = snapshot.calls.map(call => call.command);
  expect(commands.filter(command => command === 'update_record')).toHaveLength(2);
  expect(commands.filter(command => command === 'delete_record')).toHaveLength(1);
  expect(JSON.parse(snapshot.settings.sync_outbox_v1 || '{}')).toMatchObject({ pending: true });
  const syncSnapshot = await page.evaluate(() => window.__TAURI_INTERNALS__.invoke('get_sync_snapshot')) as {
    tombstones: Array<{ id: string }>;
  };
  expect(syncSnapshot.tombstones).toContainEqual(expect.objectContaining({ id: local.id }));
});

test('M1.4 failed sync keeps outbox and local CRUD available with persistent error state', async ({ page }) => {
  const local = record('断网仍可编辑'); const remote = payload([local]);
  await setupMockIpc(page, { records: [local], settings: { ...configured, sync_v3_baseline: JSON.stringify(remote) }, webdavV3Remote: remote, webdavFailureCount: 20 });
  await page.goto('/');
  await page.getByRole('button', { name: '打开 断网仍可编辑' }).click();
  await page.getByRole('button', { name: '标为已看' }).click();
  await expect(page.getByText('状态已更新为“已看”。', { exact: true })).toBeVisible();
  await page.evaluate(() => window.dispatchEvent(new Event('online')));
  await expect.poll(async () => JSON.parse((await mockSnapshot(page)).settings.sync_outbox_v1 || '{}').pending).toBe(true);
  await page.getByRole('button', { name: '返回片库' }).click(); await openSettings(page);
  await expect(page.getByTestId('mobile-sync-runtime')).toContainText('待上传是');
  await expect(page.getByRole('alert')).toContainText('本地修改已保留');
});

test('M1.4 exposes pause/resume and persistent conflict resolution', async ({ page }) => {
  const base = record('冲突记录');
  const local = { ...base, notes: 'mobile', rev: 2, revActor: 'mobile' };
  const remote = { ...base, notes: 'desktop', rev: 2, revActor: 'desktop' };
  const conflict = { id: base.id, kind: 'edit-edit' as const, fields: ['notes'], base, local, remote, localDeleted: false, remoteDeleted: false, detectedAt: '2026-08-27T01:00:00.000Z' };
  await setupMockIpc(page, { records: [local], settings: { ...configured, sync_v3_conflicts: JSON.stringify([conflict]) }, webdavV3Remote: payload([remote]) });
  await page.goto('/'); await openSettings(page);
  await expect(page.getByTestId('mobile-conflict-center')).toContainText('冲突记录');
  await page.getByRole('button', { name: '暂停自动同步' }).click();
  await expect(page.getByText('自动同步已暂停。')).toBeVisible();
  await page.getByRole('button', { name: '恢复自动同步' }).click();
  await expect(page.getByText('自动同步已恢复。')).toBeVisible();
  page.once('dialog', dialog => void dialog.accept());
  await page.getByRole('button', { name: '采用本机' }).click();
  await expect(page.getByTestId('mobile-conflict-center')).toContainText('暂无冲突');
  expect(JSON.parse((await mockSnapshot(page)).settings.sync_outbox_v1 || '{}').pending).toBe(true);
});

test('M1.4 reports reentry-required without blocking the local library', async ({ page }) => {
  await setupMockIpc(page, { records: [record('换机保留记录')], settings: configured, webdavCredentialState: 'reentry-required' });
  await page.goto('/'); await expect(page.getByRole('button', { name: '打开 换机保留记录' })).toBeVisible();
  await openSettings(page);
  await expect(page.getByTestId('mobile-sync-runtime')).toContainText('需要重新输入凭据');
  await expect(page.getByRole('button', { name: '立即同步' })).toHaveCount(0);
  await expect(page.getByRole('button', { name: '清除凭据' })).toBeVisible();
  page.once('dialog', dialog => void dialog.accept());
  await page.getByRole('button', { name: '清除凭据' }).click();
  await expect(page.getByText('凭据已清除；本地数据未删除。')).toBeVisible();
  await page.getByRole('button', { name: '片库' }).click();
  await expect(page.getByRole('button', { name: '打开 换机保留记录' })).toBeVisible();
});

test('M1.4 lets an active target with a missing credential be explicitly cleared', async ({ page }) => {
  await setupMockIpc(page, { records: [record('缺失凭据仍保留')], settings: configured, webdavCredentialState: 'missing' });
  await page.goto('/'); await openSettings(page);
  await expect(page.getByTestId('mobile-sync-runtime')).toContainText('凭据缺失');
  await expect(page.getByRole('button', { name: '立即同步' })).toHaveCount(0);
  await expect(page.getByRole('button', { name: '清除凭据' })).toBeVisible();
  page.once('dialog', dialog => void dialog.accept());
  await page.getByRole('button', { name: '清除凭据' }).click();
  await expect(page.getByText('凭据已清除；本地数据未删除。')).toBeVisible();
  await page.getByRole('button', { name: '片库' }).click();
  await expect(page.getByRole('button', { name: '打开 缺失凭据仍保留' })).toBeVisible();
});
