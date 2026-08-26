import { expect, test } from '@playwright/test';
import type { EpisodeCompletion, WatchRecord } from '../src/shared/types';
import { mockSnapshot, setupMockIpc } from './fixtures/mockIpc';

test.use({ userAgent: 'Mozilla/5.0 (Linux; Android 16; Pixel) AppleWebKit/537.36 Chrome/140 Mobile' });

function record(id: string, overrides: Partial<WatchRecord> = {}): WatchRecord {
  return {
    id, originalName: `${id} original`, chineseName: id, progress: '', totalEpisodes: null,
    episodeTrackingEnabled: false, nextEpisode: null, movieProgress: null, movieDuration: null,
    releaseYear: '2026', posterPath: null, status: '未看', platform: '', rating: null,
    startDate: '', endDate: '', notes: '', createdAt: '2026-08-26T00:00:00.000Z',
    updatedAt: null, imdbId: null, isLocked: false, mediaType: '电影', contentTags: null,
    originCountry: null, rev: 1, revActor: 'seed', ...overrides,
  };
}

function completion(recordId: string, episodeNumber: number, completedAt: string | null = null): EpisodeCompletion {
  return { id: `${recordId}-${episodeNumber}`, recordId, episodeNumber, completedAt, createdAt: '2026-08-20T00:00:00.000Z', updatedAt: '2026-08-20T00:00:00.000Z', rev: 1, revActor: 'seed' };
}

test('M1.3 hides invalid controls and explicitly enables tracking without converting legacy progress', async ({ page }) => {
  await setupMockIpc(page, { records: [
    record('电影有总集数', { totalEpisodes: 9 }),
    record('剧集缺总数', { mediaType: '剧集' }),
    record('可跟踪纪录片', { mediaType: '纪录片', totalEpisodes: 3, progress: '旧进度 E01' }),
  ] });
  await page.goto('/');
  await expect(page.getByText('未启用逐集跟踪')).toHaveCount(1);
  await page.getByRole('button', { name: '打开 可跟踪纪录片' }).click();
  await expect(page.getByRole('heading', { name: '逐集跟踪' })).toBeVisible();
  await expect(page.getByText('旧进度“旧进度 E01”会原样保留，不会转换为逐集历史。')).toBeVisible();
  await page.getByRole('combobox', { name: '初始下一集' }).selectOption('2');
  await page.getByRole('button', { name: '启用逐集跟踪' }).click();
  await expect(page.getByText('逐集跟踪已启用，下一集为第 2 集。', { exact: true })).toBeVisible();
  const snapshot = await mockSnapshot(page);
  expect(snapshot.records.find(item => item.id === '可跟踪纪录片')).toMatchObject({ progress: '旧进度 E01', episodeTrackingEnabled: true, nextEpisode: 2, status: '在看', rev: 2 });
  expect(snapshot.calls.find(call => call.command === 'enable_episode_tracking')?.args).toMatchObject({ recordId: '可跟踪纪录片', initialNextEpisode: 2, expectedRev: 1 });
});

test('M1.3 completes, jumps, retreats with retained history, and finishes atomically', async ({ page }) => {
  await setupMockIpc(page, { records: [record('动作剧', { mediaType: '剧集', totalEpisodes: 4 })] });
  await page.goto('/'); await page.getByRole('button', { name: '打开 动作剧' }).click();
  await page.getByRole('combobox', { name: '初始下一集' }).selectOption('2'); await page.getByRole('button', { name: '启用逐集跟踪' }).click();
  await page.getByRole('button', { name: '完成第 2 集' }).click(); await expect(page.getByText('共 4 集 · 下一集：第 3 集')).toBeVisible();
  await page.getByRole('combobox', { name: '调整下一集' }).selectOption('4'); await page.getByRole('button', { name: '跳至第 4 集' }).click();
  await page.getByRole('combobox', { name: '调整下一集' }).selectOption('2'); await page.getByRole('button', { name: '回退至第 2 集' }).click();
  let snapshot = await mockSnapshot(page); const beforeFinish = snapshot.episodeCompletions.map(item => item.episodeNumber);
  expect(beforeFinish).toEqual([2, 3]);
  await page.getByRole('button', { name: '将全部集数标为完结' }).click();
  await expect(page.getByText('已原子完成剩余集数并标记完结。', { exact: true })).toBeVisible();
  snapshot = await mockSnapshot(page);
  expect(snapshot.records[0]).toMatchObject({ status: '已看', nextEpisode: null, episodeTrackingEnabled: true });
  expect(snapshot.records[0].endDate).toBeTruthy();
  expect(snapshot.episodeCompletions.map(item => item.episodeNumber)).toEqual([2, 3, 4]);
  expect(snapshot.calls.filter(call => call.command === 'update_record')).toHaveLength(0);
  expect(snapshot.calls.filter(call => call.command === 'set_next_episode').every(call => typeof call.args.expectedRev === 'number')).toBe(true);
});

test('M1.3 list quick action completes the final episode through the episode command', async ({ page }) => {
  await setupMockIpc(page, { records: [record('快捷剧', { mediaType: '剧集', totalEpisodes: 2, episodeTrackingEnabled: true, nextEpisode: 2, status: '在看', rev: 7 })] });
  await page.goto('/'); await page.getByRole('button', { name: '完成第 2 集' }).click();
  await expect(page.getByText('第 2 集已完成，这条记录已完结。', { exact: true })).toBeVisible();
  const snapshot = await mockSnapshot(page);
  expect(snapshot.records[0]).toMatchObject({ status: '已看', nextEpisode: null, rev: 8 });
  expect(snapshot.calls.find(call => call.command === 'set_next_episode')?.args).toMatchObject({ recordId: '快捷剧', nextEpisode: null, expectedRev: 7 });
  expect(snapshot.calls.some(call => call.command === 'update_record')).toBe(false);
});

test('M1.3 reloads stale progress without optimistic success and keeps the retry UI', async ({ page }) => {
  await setupMockIpc(page, { records: [record('并发剧', { mediaType: '剧集', totalEpisodes: 3, episodeTrackingEnabled: true, nextEpisode: 1, status: '在看' })] });
  await page.goto('/');
  await expect(page.getByRole('button', { name: '完成第 1 集' })).toBeVisible();
  await page.evaluate(() => { window.__WATCHTRACKER_TEST__.records[0].rev = 2; });
  await page.getByRole('button', { name: '完成第 1 集' }).click();
  await expect(page.getByText('逐集进度已被其他操作更新，已重新载入，请确认后重试。', { exact: true })).toBeVisible();
  await expect(page.getByRole('button', { name: '完成第 1 集' })).toBeVisible();
  const snapshot = await mockSnapshot(page);
  expect(snapshot.records[0]).toMatchObject({ nextEpisode: 1, rev: 2 });
  expect(snapshot.episodeCompletions).toEqual([]);
});

test('M1.3 treats locked records as read-only in list and detail', async ({ page }) => {
  await setupMockIpc(page, { records: [record('锁定剧', { mediaType: '剧集', totalEpisodes: 3, episodeTrackingEnabled: true, nextEpisode: 1, status: '在看' })] });
  await page.goto('/');
  await expect(page.getByRole('button', { name: '完成第 1 集' })).toBeVisible();
  await page.evaluate(() => { window.__WATCHTRACKER_TEST__.records[0].isLocked = true; });
  await page.getByRole('button', { name: '完成第 1 集' }).click();
  await expect(page.getByText('记录已锁定，逐集进度保持不变。', { exact: true })).toBeVisible();
  await expect(page.getByRole('button', { name: '完成第 1 集' })).toHaveCount(0);
  await page.getByRole('button', { name: '打开 锁定剧' }).click();
  await expect(page.getByText('🔒 记录已锁定，逐集进度和历史均为只读。')).toBeVisible();
  await expect(page.getByRole('button', { name: '完成第 1 集' })).toBeDisabled();
  await expect(page.getByRole('combobox', { name: '调整下一集' })).toBeDisabled();
  expect((await mockSnapshot(page)).episodeCompletions).toEqual([]);
});

test('M1.3 reloads a missing record and reports that no progress was written', async ({ page }) => {
  await setupMockIpc(page, { records: [record('消失剧', { mediaType: '动画', totalEpisodes: 2, episodeTrackingEnabled: true, nextEpisode: 1, status: '在看' })] });
  await page.goto('/'); await expect(page.getByRole('button', { name: '完成第 1 集' })).toBeVisible(); await page.evaluate(() => { window.__WATCHTRACKER_TEST__.records = []; });
  await page.getByRole('button', { name: '完成第 1 集' }).click();
  await expect(page.getByText('记录已不存在，已重新载入片库。', { exact: true })).toBeVisible();
  await expect(page.locator('[data-empty-state="library"]')).toBeVisible();
  expect((await mockSnapshot(page)).episodeCompletions).toEqual([]);
});

test('M1.3 resumes a completed tracked record only after new episodes are added', async ({ page }) => {
  const id = '增集剧';
  await setupMockIpc(page, {
    records: [record(id, { mediaType: '剧集', totalEpisodes: 4, episodeTrackingEnabled: true, nextEpisode: null, status: '已看', endDate: '2026-08-20', rev: 5 })],
    episodeCompletions: [completion(id, 1, '2026-08-19T10:00:00.000Z'), completion(id, 2, '2026-08-20T10:00:00.000Z')],
  });
  await page.goto('/'); await page.getByRole('button', { name: `打开 ${id}` }).click();
  await expect(page.getByText('发现新增 2 集，可以从第 3 集继续追更。')).toBeVisible();
  await page.getByRole('button', { name: '继续追更第 3 集' }).click();
  await expect(page.getByText('已从第 3 集继续追更。', { exact: true })).toBeVisible();
  const snapshot = await mockSnapshot(page);
  expect(snapshot.records[0]).toMatchObject({ status: '在看', nextEpisode: 3, endDate: '2026-08-20', rev: 6 });
  expect(snapshot.episodeCompletions).toHaveLength(2);
});

test('M1.3 episode controls keep 48px touch targets and accessible names', async ({ page }) => {
  await page.setViewportSize({ width: 360, height: 740 });
  await setupMockIpc(page, { records: [record('触控剧', { mediaType: '综艺', totalEpisodes: 3, episodeTrackingEnabled: true, nextEpisode: 2, status: '在看' })] });
  await page.goto('/'); await page.getByRole('button', { name: '打开 触控剧' }).click();
  for (const control of await page.locator('.mobile-episode-panel button, .mobile-episode-panel select').all()) {
    const box = await control.boundingBox(); expect(box?.height ?? 0).toBeGreaterThanOrEqual(48);
    expect((await control.getAttribute('aria-label')) || (await control.textContent())?.trim()).toBeTruthy();
  }
});
