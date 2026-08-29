import { expect, test } from '@playwright/test';
import type { CollectionMember, EpisodeCompletion, WatchCollection, WatchRecord } from '../src/shared/types';
import { mockSnapshot, setupMockIpc } from './fixtures/mockIpc';

test.use({ userAgent: 'Mozilla/5.0 (Linux; Android 16; Pixel) AppleWebKit/537.36 Chrome/140 Mobile' });

const record: WatchRecord = {
  id: 'export-record', originalName: 'Export Original', chineseName: '导出记录', progress: '',
  totalEpisodes: 2, episodeTrackingEnabled: true, nextEpisode: 2, movieProgress: null,
  movieDuration: null, releaseYear: '2026', posterPath: null, status: '在看', platform: '',
  rating: null, startDate: '2026-08-29', endDate: '', notes: '',
  createdAt: '2026-08-29T00:00:00.000Z', updatedAt: '2026-08-29T01:00:00.000Z',
  imdbId: null, mediaType: '剧集', rev: 2, revActor: 'device',
};
const completion: EpisodeCompletion = {
  id: 'export-completion', recordId: record.id, episodeNumber: 1,
  completedAt: '2026-08-29T01:00:00.000Z', createdAt: '2026-08-29T01:00:00.000Z',
  updatedAt: '2026-08-29T01:00:00.000Z', rev: 1, revActor: 'device',
};
const collection: WatchCollection = {
  id: 'export-collection', name: '导出收藏', normalizedName: '导出收藏', description: null,
  sourceKind: 'manual', sourceKey: null, collectionKind: 'manual', orderMode: 'manual',
  createdAt: '2026-08-29T00:00:00.000Z', updatedAt: '2026-08-29T00:00:00.000Z',
  rev: 1, revActor: 'device',
};
const member: CollectionMember = {
  id: 'export-member', collectionId: collection.id, recordId: record.id, position: 0,
  sourceKind: 'manual', createdAt: '2026-08-29T00:00:00.000Z',
  updatedAt: '2026-08-29T00:00:00.000Z', rev: 1, revActor: 'device',
};

async function openSettings(page: import('@playwright/test').Page) {
  await page.getByRole('button', { name: '设置' }).click();
  await expect(page.getByRole('heading', { name: '设置', exact: true })).toBeVisible();
}

test('M2.1 exports the desktop-compatible V4 whitelist without WebDAV or local mutation', async ({ page }) => {
  await page.setViewportSize({ width: 360, height: 740 });
  await setupMockIpc(page, {
    records: [record], episodeCompletions: [completion], collections: [collection],
    collectionMembers: [member], documentExportDelayMs: 120,
  });
  await page.goto('/');
  await openSettings(page);

  await expect(page.getByRole('heading', { name: '备份与恢复' })).toBeVisible();
  await expect(page.getByText(/不会包含 WebDAV 密码、TMDB API Key/)).toBeVisible();
  expect(await page.evaluate(() => document.documentElement.scrollWidth <= window.innerWidth)).toBe(true);
  const button = page.getByRole('button', { name: '导出数据' });
  await expect(button).toBeEnabled();
  await button.click();
  await expect(page.getByRole('button', { name: '正在准备导出…' })).toBeDisabled();
  await expect(page.getByText('数据已导出。', { exact: true })).toBeVisible();

  const snapshot = await mockSnapshot(page);
  expect(snapshot.records).toHaveLength(1);
  expect(snapshot.exportedFileName).toMatch(/^WatchTracker-backup-\d{4}-\d{2}-\d{2}-\d{6}\.json$/);
  const payload = JSON.parse(snapshot.lastExportJson || '{}');
  expect(payload).toMatchObject({
    formatVersion: 4,
    records: [record],
    episodeCompletions: [completion],
    collections: [collection],
    collectionMembers: [member],
  });
  expect(Object.keys(payload)).toEqual([
    'formatVersion', 'exportedAt', 'records', 'episodeCompletions', 'collections', 'collectionMembers',
  ]);
  const writeCommands = new Set([
    'insert_record', 'update_record', 'delete_record', 'replace_all_records', 'replace_library',
    'replace_library_v3', 'create_recovery_point', 'set_auto_sync_paused', 'commit_sync_result',
  ]);
  expect(snapshot.calls.filter(call => writeCommands.has(call.command))).toEqual([]);
});

test('M2.1 treats picker cancellation as a quiet non-error result', async ({ page }) => {
  await setupMockIpc(page, { records: [record], documentExportResult: 'cancelled' });
  await page.goto('/'); await openSettings(page);
  await page.getByRole('button', { name: '导出数据' }).click();

  await expect(page.getByTestId('mobile-export-status')).toHaveText('已取消导出。');
  await expect(page.getByText('数据导出失败。', { exact: true })).toHaveCount(0);
  expect((await mockSnapshot(page)).records).toHaveLength(1);
});

test('M2.1 reports write failure and remains available while sync is paused and errored', async ({ page }) => {
  const scheduler = {
    version: 1, paused: true, consecutiveFailures: 3, nextAttemptAt: null,
    lastAttemptAt: '2026-08-29T01:00:00.000Z', lastSuccessAt: null,
    lastErrorCode: 'network_unavailable', lastRemoteCheckAt: '2026-08-29T01:00:00.000Z',
  };
  await setupMockIpc(page, {
    records: [record],
    settings: {
      webdav_creds: 'androidkeystore:v1',
      webdav_url: 'https://example.test/dav/',
      sync_scheduler_v1: JSON.stringify(scheduler),
    },
    documentExportResult: 'error',
  });
  await page.goto('/'); await openSettings(page);
  await expect(page.getByRole('button', { name: '导出数据' })).toBeEnabled();
  await expect(page.getByRole('alert')).toContainText('同步错误');
  await page.getByRole('button', { name: '导出数据' }).click();

  await expect(page.getByRole('alert').filter({ hasText: '数据导出失败。' })).toBeVisible();
  expect((await mockSnapshot(page)).records).toHaveLength(1);
});
