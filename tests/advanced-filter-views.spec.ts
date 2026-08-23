import { expect, test } from '@playwright/test';
import type { WatchRecord } from '../src/shared/types';
import { mockSnapshot, setupMockIpc } from './fixtures/mockIpc';

function record(id: string, overrides: Partial<WatchRecord> = {}): WatchRecord {
  return {
    id, originalName: `${id} original`, chineseName: id, progress: '', totalEpisodes: null,
    movieProgress: null, movieDuration: null, releaseYear: '2025', posterPath: null,
    status: '未看', platform: '', rating: null, startDate: '', endDate: '', notes: '',
    createdAt: `2026-01-0${id.length}T00:00:00.000Z`, imdbId: null, mediaType: '电影',
    contentTags: null, originCountry: null, ...overrides,
  };
}

const records = [
  record('国产悬疑剧', { mediaType: '剧集', status: '在看', originCountry: 'CN,US', platform: '腾讯视频', genres: '剧情,悬疑', contentTags: '刑侦', rating: 9, imdbRating: 8.2, isLocked: true }),
  record('国产综艺', { mediaType: '综艺', status: '在看', originCountry: 'CN', platform: '腾讯视频', genres: '真人秀', rating: 7 }),
  record('英国电影', { status: '已看', originCountry: 'UK', platform: 'Netflix', genres: '剧情', rating: 8.5 }),
];

test('@advanced-filter combines fields, summarizes them, and never writes records', async ({ page }) => {
  await setupMockIpc(page, { records });
  await page.goto('/');
  await page.getByRole('button', { name: /^剧集 1$/ }).click();
  await page.getByRole('button', { name: /^在看 1$/ }).click();
  await page.getByRole('button', { name: /^中国大陆 1$/ }).click();
  await page.getByRole('button', { name: '高级筛选' }).click();
  const dialog = page.getByRole('dialog', { name: '高级筛选' });
  await expect(dialog).toBeVisible();
  await expect(dialog.getByRole('group', { name: '媒体类型' })).toHaveCount(0);
  await expect(dialog.getByRole('group', { name: '观看状态' })).toHaveCount(0);
  await expect(dialog.getByRole('group', { name: /地区/ })).toHaveCount(0);
  await expect(dialog.getByRole('group', { name: '锁定状态' })).toHaveCount(0);
  await dialog.getByRole('button', { name: /^腾讯视频 / }).click();
  await dialog.getByLabel('个人评分最小值').fill('8');
  await dialog.getByRole('button', { name: '完成' }).click();

  await expect(page.getByText('国产悬疑剧', { exact: true })).toBeVisible();
  await expect(page.getByText('国产综艺', { exact: true })).toHaveCount(0);
  await expect(page.getByText('英国电影', { exact: true })).toHaveCount(0);
  const summary = page.getByLabel('当前高级筛选条件');
  await expect(summary).toContainText('平台：腾讯视频');
  await expect(summary).toContainText('个人评分 ≥ 8');
  await expect(summary).not.toContainText('剧集');
  await expect(summary).not.toContainText('在看');
  await expect(summary).not.toContainText('中国大陆');
  await expect(page.getByRole('button', { name: '高级筛选 2' })).toBeVisible();

  const snapshot = await mockSnapshot(page);
  expect(snapshot.calls.some(call => ['insert_record', 'update_record', 'delete_record'].includes(call.command))).toBe(false);
  expect(snapshot.settings.sync_outbox_v1).toBeUndefined();
});

test('@saved-views saves locally, marks changes dirty, updates, and configures startup', async ({ page }) => {
  await setupMockIpc(page, { records });
  await page.goto('/');
  await page.getByRole('button', { name: /^剧集 1$/ }).click();
  await page.getByRole('button', { name: '视图：临时筛选' }).click();
  await page.getByRole('button', { name: '另存当前条件为视图' }).click();
  await page.getByLabel('视图名称').fill('正在追的剧');
  await page.getByRole('button', { name: '保存', exact: true }).click();

  await expect(page.getByRole('button', { name: '视图：正在追的剧' })).toBeVisible();
  await page.getByRole('button', { name: /^电影 1$/ }).click();
  await page.getByRole('button', { name: '视图：正在追的剧 · 已修改' }).click();
  await page.getByRole('button', { name: '更新当前视图' }).click();
  await page.getByRole('button', { name: '设为启动视图' }).click();
  await expect(page.getByRole('button', { name: '★ 正在追的剧' })).toBeVisible();

  const snapshot = await mockSnapshot(page);
  const stored = JSON.parse(snapshot.settings.watchlist_saved_views_v1 ?? '{}');
  expect(stored.schemaVersion).toBe(1);
  expect(stored.views).toHaveLength(1);
  expect(stored.views[0].query.mediaTypes).toEqual(['电影']);
  expect(snapshot.settings.watchlist_startup_view_id_v1).toBe(stored.views[0].id);
  expect(snapshot.calls.some(call => ['insert_record', 'update_record', 'delete_record'].includes(call.command))).toBe(false);
});

test('@saved-views applies only an explicit valid startup view', async ({ page }) => {
  const view = {
    id: 'startup-view', name: '英国已看',
    query: {
      schemaVersion: 1, searchText: '', mediaTypes: [], statuses: ['已看'], regions: ['GB'],
      platforms: [], genres: [], contentTags: [], lock: 'all',
      releaseYear: { min: null, max: null }, rating: { min: null, max: null }, imdbRating: { min: null, max: null },
    },
    sortBy: 'rating', viewMode: 'list',
    createdAt: '2026-08-09T00:00:00.000Z', updatedAt: '2026-08-09T00:00:00.000Z',
  };
  await setupMockIpc(page, { records, settings: {
    watchlist_saved_views_v1: JSON.stringify({ schemaVersion: 1, views: [view] }),
    watchlist_startup_view_id_v1: view.id,
  } });
  await page.goto('/');
  await expect(page.getByRole('button', { name: '视图：英国已看' })).toBeVisible();
  await page.getByRole('button', { name: '视图：英国已看' }).click();
  await expect(page.getByRole('button', { name: '★ 英国已看' })).toHaveAttribute('aria-pressed', 'true');
  await expect(page.getByText('英国电影', { exact: true })).toBeVisible();
  await expect(page.getByText('国产悬疑剧', { exact: true })).toHaveCount(0);
  await expect(page.locator('select').first()).toHaveValue('rating');
});

test('@saved-views keeps a failed save visible and the advanced dialog accessible on narrow screens', async ({ page }) => {
  await setupMockIpc(page, { records, failSettingWrites: true });
  await page.setViewportSize({ width: 360, height: 720 });
  await page.goto('/');
  await page.getByRole('button', { name: '视图：全部记录' }).click();
  await expect(page.getByRole('dialog', { name: '保存视图' }).getByRole('button', { name: '全部记录' })).toBeFocused();
  await page.getByRole('button', { name: '另存当前条件为视图' }).click();
  await page.getByLabel('视图名称').fill('不会保存');
  await page.getByRole('button', { name: '保存', exact: true }).click();
  await expect(page.getByLabel('视图名称')).toBeVisible();
  await expect(page.getByText('Injected setting write failure')).toHaveCount(0);

  await page.keyboard.press('Escape');
  await page.getByRole('button', { name: '高级筛选' }).click();
  const dialog = page.getByRole('dialog', { name: '高级筛选' });
  await expect(dialog).toBeVisible();
  await expect(dialog.getByRole('button', { name: '关闭高级筛选' })).toBeFocused();
  await expect.poll(() => page.locator('body').evaluate(body => body.style.overflow)).toBe('hidden');
  expect(await dialog.evaluate(element => element.scrollWidth <= element.clientWidth)).toBe(true);
  await page.keyboard.press('Escape');
  await expect(dialog).toHaveCount(0);
  await expect(page.getByRole('button', { name: '高级筛选' })).toBeFocused();
  expect(await page.evaluate(() => document.documentElement.scrollWidth <= window.innerWidth)).toBe(true);
});
