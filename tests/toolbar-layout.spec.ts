import { expect, test } from '@playwright/test';
import type { WatchRecord } from '../src/shared/types';
import { mockSnapshot, setupMockIpc } from './fixtures/mockIpc';

function record(id: string, originCountry: string): WatchRecord {
  return {
    id, originalName: id, chineseName: id, progress: '', totalEpisodes: null,
    movieProgress: null, movieDuration: null, releaseYear: '2026', posterPath: null,
    status: '未看', platform: '', rating: null, startDate: '', endDate: '', notes: '',
    createdAt: '2026-08-09T00:00:00.000Z', imdbId: null, mediaType: '电影',
    contentTags: null, originCountry,
  };
}

const pausedSyncSettings = {
  webdav_creds: 'encrypted:user:password',
  webdav_url: 'https://example.test/watchtracker/',
  sync_scheduler_v1: JSON.stringify({
    version: 1, paused: true, consecutiveFailures: 0, nextAttemptAt: null,
    lastAttemptAt: null, lastSuccessAt: '2026-08-09T08:00:00.000Z',
    lastErrorCode: null, lastRemoteCheckAt: null,
  }),
};

test('@toolbar keeps the confirmed eight-part desktop structure on one line', async ({ page }) => {
  await page.setViewportSize({ width: 1200, height: 800 });
  await setupMockIpc(page, { records: [record('布局记录', 'CN')], settings: pausedSyncSettings });
  await page.goto('/');

  const toolbar = page.getByLabel('顶部工具栏');
  await expect(toolbar.getByText('影视追踪', { exact: true })).toBeVisible();
  await expect(toolbar.getByRole('button', { name: /视图：/ })).toBeVisible();
  await expect(toolbar.getByPlaceholder('搜索电影、剧集...')).toBeVisible();
  await expect(toolbar.getByRole('button', { name: /高级筛选/ })).toBeVisible();
  await expect(toolbar.getByRole('combobox', { name: '排序方式' })).toBeVisible();
  await expect(toolbar.getByRole('button', { name: /云端同步：已暂停/ })).toBeVisible();
  await expect(toolbar.getByRole('button', { name: '设置' })).toBeVisible();
  await expect(toolbar.getByRole('button', { name: '更多操作' })).toBeVisible();

  const verticalCenters = await toolbar.locator(':scope > *').evaluateAll(elements =>
    elements.map(element => {
      const rect = element.getBoundingClientRect();
      return rect.top + rect.height / 2;
    }),
  );
  expect(Math.max(...verticalCenters) - Math.min(...verticalCenters)).toBeLessThanOrEqual(1);
  expect(await page.locator('body').evaluate(element => element.scrollWidth > element.clientWidth)).toBe(false);
});

test('@toolbar popovers are inert until an explicit action and moved actions remain reachable', async ({ page }) => {
  await setupMockIpc(page, { records: [record('菜单记录', 'CN')], settings: pausedSyncSettings });
  await page.goto('/');
  const before = await mockSnapshot(page);
  const writeCommands = new Set(['insert_record', 'update_record', 'delete_record', 'replace_all_records', 'replace_library', 'set_auto_sync_paused']);
  const businessCallsBefore = before.calls.filter(call => call.command === 'webdav_request' || writeCommands.has(call.command)).length;

  await page.getByRole('button', { name: /云端同步：已暂停/ }).click();
  await expect(page.getByRole('dialog', { name: '同步状态' })).toBeVisible();
  await page.keyboard.press('Escape');
  await page.getByRole('button', { name: '更多操作' }).click();
  await expect(page.getByRole('menuitem', { name: /添加记录/ })).toBeVisible();
  await expect(page.getByRole('menuitem', { name: /切换至海报墙/ })).toBeVisible();
  await expect(page.getByRole('menuitem', { name: '数据看板' })).toBeVisible();
  await page.keyboard.press('Escape');

  const after = await mockSnapshot(page);
  expect(after.calls.filter(call => call.command === 'webdav_request' || writeCommands.has(call.command))).toHaveLength(businessCallsBefore);

  await page.keyboard.press('Control+n');
  await expect(page.getByRole('dialog', { name: '添加新记录' })).toBeVisible();
});

test('@toolbar remains page-safe at 360px while controls scroll inside their rows', async ({ page }) => {
  await page.setViewportSize({ width: 360, height: 740 });
  const codes = ['CN', 'US', 'JP', 'KR', 'GB', 'FR', 'DE', 'XX'];
  await setupMockIpc(page, { records: codes.map(code => record(`记录-${code}`, code)), settings: pausedSyncSettings });
  await page.goto('/');

  await expect(page.getByLabel('顶部工具栏').getByText('影视追踪', { exact: true })).toBeVisible();
  await expect(page.getByLabel('地区筛选').getByRole('button', { name: /更多地区/ })).toBeVisible();
  expect(await page.locator('body').evaluate(element => element.scrollWidth > element.clientWidth)).toBe(false);
  expect(await page.getByLabel('顶部工具栏').evaluate(element => element.scrollWidth > element.clientWidth)).toBe(true);
});
