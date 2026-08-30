import { expect, test } from '@playwright/test';
import type { CollectionMember, EpisodeCompletion, WatchCollection, WatchRecord } from '../src/shared/types';
import { mockSnapshot, setupMockIpc } from './fixtures/mockIpc';

test.use({ userAgent: 'Mozilla/5.0 (Linux; Android 16; Pixel) AppleWebKit/537.36 Chrome/140 Mobile' });

const oldRecord: WatchRecord = {
  id: 'old-record', originalName: 'Old', chineseName: '旧记录', progress: '', totalEpisodes: null,
  episodeTrackingEnabled: false, nextEpisode: null, movieProgress: null, movieDuration: null,
  releaseYear: '2024', posterPath: null, status: '未看', platform: '', rating: null,
  startDate: '', endDate: '', notes: '', createdAt: '2026-01-01T00:00:00.000Z',
  updatedAt: null, imdbId: null, mediaType: '电影', rev: 1, revActor: 'local',
};
const importedRecord: WatchRecord = {
  ...oldRecord, id: 'imported-record', originalName: 'Imported', chineseName: '导入记录',
  totalEpisodes: 2, episodeTrackingEnabled: true, nextEpisode: 2, mediaType: '剧集', rev: 3,
};
const completion: EpisodeCompletion = {
  id: 'completion', recordId: importedRecord.id, episodeNumber: 1,
  completedAt: '2026-08-30T00:00:00.000Z', createdAt: '2026-08-30T00:00:00.000Z',
  updatedAt: '2026-08-30T00:00:00.000Z', rev: 1, revActor: 'backup',
};
const collection: WatchCollection = {
  id: 'collection', name: '导入收藏', normalizedName: '导入收藏', description: null,
  sourceKind: 'manual', sourceKey: null, collectionKind: 'manual', orderMode: 'manual',
  createdAt: '2026-08-30T00:00:00.000Z', updatedAt: '2026-08-30T00:00:00.000Z',
  rev: 1, revActor: 'backup',
};
const member: CollectionMember = {
  id: 'member', collectionId: collection.id, recordId: importedRecord.id, position: 0,
  sourceKind: 'manual', createdAt: '2026-08-30T00:00:00.000Z',
  updatedAt: '2026-08-30T00:00:00.000Z', rev: 1, revActor: 'backup',
};
const preview = {
  stageSha256: 'stage-sha', currentLibraryFingerprint: 'library-sha', sizeBytes: 4096,
  formatVersion: 4, exportedAt: '2026-08-30T00:00:00.000Z',
  counts: { records: 2, episodeCompletions: 1, collections: 1, collectionMembers: 1 },
  records: { added: 1, updated: 1, removed: 1, unchanged: 2, lockedPreserved: 1, finalCount: 3 },
};

async function openSettings(page: import('@playwright/test').Page) {
  await page.getByRole('button', { name: '设置' }).click();
  await expect(page.getByRole('heading', { name: '设置', exact: true })).toBeVisible();
}

function validOptions() {
  return {
    records: [oldRecord],
    documentImportPreview: preview,
    documentImportRecords: [importedRecord],
    documentImportEpisodeCompletions: [completion],
    documentImportCollections: [collection],
    documentImportCollectionMembers: [member],
  };
}

test('M2.2 shows a read-only four-entity preview and commits through the token API', async ({ page }) => {
  await page.setViewportSize({ width: 360, height: 740 });
  await setupMockIpc(page, validOptions());
  await page.goto('/'); await openSettings(page);
  await expect(page.getByRole('button', { name: '导入数据' })).toBeVisible();
  await page.getByRole('button', { name: '导入数据' }).click();

  const card = page.getByRole('region', { name: '导入预览' });
  await expect(card).toBeVisible();
  await expect(card).toContainText('WatchTracker-backup-test.json');
  await expect(card).toContainText('V4');
  for (const text of ['记录', '逐集历史', '收藏集', '收藏成员', '新增', '更新', '删除', '不变', '锁定保护']) {
    await expect(card).toContainText(text);
  }
  expect(await page.evaluate(() => document.documentElement.scrollWidth <= window.innerWidth)).toBe(true);

  const before = await mockSnapshot(page);
  expect(before.records).toEqual([oldRecord]);
  expect(before.calls.some(call => call.command === 'commit_local_import')).toBe(false);

  await page.getByRole('button', { name: '确认导入并替换' }).click();
  await expect(page.getByTestId('mobile-import-status')).toContainText('数据已导入');
  const after = await mockSnapshot(page);
  expect(after.records).toEqual([importedRecord]);
  expect(after.episodeCompletions).toEqual([completion]);
  expect(after.collections).toEqual([collection]);
  expect(after.collectionMembers).toEqual([member]);
  expect(after.recoveryPoints[0]?.reason).toBe('import');
  expect(after.calls.some(call => call.command === 'get_all_records'
    && after.calls.indexOf(call) > after.calls.findIndex(item => item.command === 'commit_local_import'))).toBe(true);
});

test('M2.2 picker cancellation is quiet and creates no preview or error', async ({ page }) => {
  await setupMockIpc(page, { records: [oldRecord], documentImportResult: 'cancelled' });
  await page.goto('/'); await openSettings(page);
  await page.getByRole('button', { name: '导入数据' }).click();
  await expect(page.getByTestId('mobile-import-status')).toHaveText('已取消导入。');
  await expect(page.getByTestId('mobile-import-error')).toHaveCount(0);
  await expect(page.getByRole('button', { name: '确认导入并替换' })).toHaveCount(0);
  expect((await mockSnapshot(page)).records).toEqual([oldRecord]);
});

for (const [name, code, message] of [
  ['invalid JSON', 'invalid_json', '不是有效的 JSON 文件'],
  ['future version', 'future_backup_version', '备份来自更新版本'],
] as const) {
  test(`M2.2 rejects ${name} without exposing confirmation`, async ({ page }) => {
    await setupMockIpc(page, { records: [oldRecord], documentImportPreviewError: code });
    await page.goto('/'); await openSettings(page);
    await page.getByRole('button', { name: '导入数据' }).click();
    await expect(page.getByTestId('mobile-import-error')).toContainText(message);
    await expect(page.getByRole('button', { name: '确认导入并替换' })).toHaveCount(0);
    expect((await mockSnapshot(page)).records).toEqual([oldRecord]);
  });
}

test('M2.2 rejects stale confirmation, keeps the library and refreshes the preview', async ({ page }) => {
  await setupMockIpc(page, { ...validOptions(), documentImportCommitError: 'import_preview_stale' });
  await page.goto('/'); await openSettings(page);
  await page.getByRole('button', { name: '导入数据' }).click();
  await page.getByRole('button', { name: '确认导入并替换' }).click();
  await expect(page.getByTestId('mobile-import-error')).toContainText('本地片库在预览后发生变化');
  await expect(page.getByRole('region', { name: '导入预览' })).toBeVisible();
  expect((await mockSnapshot(page)).records).toEqual([oldRecord]);
});

test('M2.2 local import remains available without WebDAV and while sync is paused or errored', async ({ page }) => {
  await setupMockIpc(page, {
    ...validOptions(),
    settings: {
      sync_scheduler_v1: JSON.stringify({
        version: 1, paused: true, consecutiveFailures: 2, nextAttemptAt: null,
        lastAttemptAt: null, lastSuccessAt: null, lastErrorCode: 'network_unavailable', lastRemoteCheckAt: null,
      }),
    },
  });
  await page.goto('/'); await openSettings(page);
  await expect(page.getByRole('button', { name: '导入数据' })).toBeEnabled();
  await page.getByRole('button', { name: '导入数据' }).click();
  await expect(page.getByRole('region', { name: '导入预览' })).toBeVisible();
});

test('M2.2 cancellation from preview discards its staged document', async ({ page }) => {
  await setupMockIpc(page, validOptions());
  await page.goto('/'); await openSettings(page);
  await page.getByRole('button', { name: '导入数据' }).click();
  await page.getByRole('button', { name: '取消导入' }).click();
  await expect(page.getByRole('region', { name: '导入预览' })).toHaveCount(0);
  expect((await mockSnapshot(page)).calls.some(call => call.command === 'discard_local_import_stage')).toBe(true);
});
