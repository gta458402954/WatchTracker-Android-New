import { expect, test } from '@playwright/test';
import type { WatchRecord } from '../src/shared/types';
import { mockSnapshot, setupMockIpc } from './fixtures/mockIpc';

test.use({ userAgent: 'Mozilla/5.0 (Linux; Android 16; Pixel) AppleWebKit/537.36 Chrome/140 Mobile' });
function record(id: string, overrides: Partial<WatchRecord> = {}): WatchRecord {
  return { id, originalName: `${id} original`, chineseName: id, progress: '', totalEpisodes: null, movieProgress: null, movieDuration: null, releaseYear: '2026', posterPath: null, status: '未看', platform: '', rating: null, startDate: '', endDate: '', notes: '', createdAt: `2026-08-${id.length.toString().padStart(2, '0')}T00:00:00.000Z`, imdbId: null, mediaType: '电影', contentTags: null, originCountry: null, rev: 1, ...overrides };
}

test('M1.2 distinguishes library, search, and filter empty states', async ({ page }) => {
  await setupMockIpc(page, { records: [record('测试电影', { platform: 'Netflix' })] }); await page.goto('/');
  await expect(page.getByRole('heading', { name: '我的片库' })).toBeVisible();
  await page.getByRole('textbox', { name: '搜索片库' }).fill('不存在'); await expect(page.locator('[data-empty-state="search"]')).toBeVisible();
  await page.getByRole('button', { name: '清空搜索' }).click(); await page.getByRole('button', { name: '打开筛选' }).click(); await page.locator('fieldset').filter({ hasText: '状态' }).locator('button.mobile-chip').filter({ hasText: '已看' }).click(); await page.getByRole('button', { name: '应用筛选' }).click(); await expect(page.locator('[data-empty-state="filter"]')).toBeVisible();
});

test('M1.2 persists sort/basic filters but not search', async ({ page }) => {
  await setupMockIpc(page, { records: [record('电影一')] }); await page.goto('/');
  await page.getByRole('textbox', { name: '搜索片库' }).fill('临时搜索'); await page.getByRole('combobox', { name: '排序' }).selectOption('rating'); await page.getByRole('button', { name: '打开筛选' }).click(); await page.getByRole('button', { name: '已锁定' }).click(); await page.getByRole('button', { name: '应用筛选' }).click();
  await expect(page.getByRole('textbox', { name: '搜索片库' })).toHaveValue('临时搜索');
  const calls = (await mockSnapshot(page)).calls.filter(call => call.command === 'set_setting'); expect(calls.some(call => String(call.args.key) === 'mobile_library_preferences_v1' && String(call.args.value).includes('rating'))).toBeTruthy(); expect(calls.some(call => String(call.args.value).includes('临时搜索'))).toBeFalsy();
});

test('M1.2 legacy poster preference falls back to the only list view without losing other preferences', async ({ page }) => {
  const legacy = JSON.stringify({ version: 1, viewMode: 'poster', sortBy: 'rating', mediaTypes: ['剧集'], statuses: ['在看'], lock: 'locked' });
  await setupMockIpc(page, {
    records: [record('旧偏好记录', { mediaType: '剧集', status: '在看', isLocked: true, rating: 9 }), record('其他记录')],
    settings: { mobile_library_preferences_v1: legacy },
  });
  await page.goto('/');
  await expect(page.locator('.mobile-record-list')).toBeVisible();
  await expect(page.locator('.mobile-poster-grid')).toHaveCount(0);
  await expect(page.getByRole('button', { name: '打开 旧偏好记录' })).toBeVisible();
  await expect(page.getByRole('button', { name: '打开 其他记录' })).toHaveCount(0);
  await expect(page.getByRole('combobox', { name: '排序' })).toHaveValue('rating');
  await expect(page.getByRole('button', { name: '移除类型：剧集' })).toBeVisible();
  await expect(page.getByRole('button', { name: '移除状态：在看' })).toBeVisible();
  await expect(page.getByRole('button', { name: '移除锁定筛选' })).toBeVisible();
  await expect(page.getByText('片库偏好版本不兼容，已使用默认设置。', { exact: true })).toHaveCount(0);
});

test('M1.2 detail, status, lock, unlock, and edit routes stay mobile-only', async ({ page }) => {
  await setupMockIpc(page, { records: [record('详情记录')] }); await page.goto('/'); await page.getByRole('button', { name: '打开 详情记录' }).click(); await expect(page).toHaveURL(/#detail\//); await expect(page.locator('#mobile-detail-title')).toBeVisible();
  await page.getByRole('button', { name: '标为已看' }).click(); await expect(page.getByText('状态已更新为“已看”。', { exact: true })).toBeVisible(); await page.getByRole('button', { name: '锁定' }).click(); await expect(page.getByText('记录已锁定。', { exact: true })).toBeVisible(); await expect(page.getByRole('button', { name: '编辑记录' })).toBeDisabled(); await page.getByRole('button', { name: '解锁' }).click(); await expect(page.getByText('记录已解锁。', { exact: true })).toBeVisible();
  await page.getByRole('button', { name: '编辑记录' }).click(); await expect(page).toHaveURL(/#form\/edit\//); await expect(page.getByRole('dialog', { name: '编辑记录' })).toBeVisible(); await page.getByRole('button', { name: '取消' }).click(); await expect(page).toHaveURL(/#detail\//); await page.getByRole('button', { name: '返回片库' }).click(); await expect(page).toHaveURL('#library');
});

test('M1.2 direct edit hash restores the record id and form values', async ({ page }) => {
  await setupMockIpc(page, { records: [record('冷启动编辑', { platform: '本地平台' })] }); await page.goto('/'); await page.goto('/#form/edit/%E5%86%B7%E5%90%AF%E5%8A%A8%E7%BC%96%E8%BE%91');
  await expect(page.getByRole('dialog', { name: '编辑记录' })).toBeVisible();
  await expect(page.getByPlaceholder('请输入中文名称')).toHaveValue('冷启动编辑');
  await expect(page.getByLabel('平台')).toHaveValue('本地平台');
  await page.goBack(); await expect(page).toHaveURL(/#library|#detail\//);
});

test('M1.2 detail round trip preserves query and the real mobile-content scroll position', async ({ page }) => {
  await setupMockIpc(page, { records: Array.from({ length: 20 }, (_, index) => record(`滚动记录${index}`, { originalName: `滚动原名${index}` })) }); await page.goto('/');
  await page.getByRole('textbox', { name: '搜索片库' }).fill('滚动'); await page.waitForTimeout(300);
  const before = await page.evaluate(() => { const el = document.querySelector<HTMLElement>('.mobile-content'); if (!el) return -1; el.style.height = '120px'; el.style.maxHeight = '120px'; el.style.overflowY = 'scroll'; el.scrollTop = 80; return el.scrollTop; });
  await page.getByRole('button', { name: '打开 滚动记录0' }).click(); await page.getByRole('button', { name: '返回片库' }).click();
  await expect(page.getByRole('textbox', { name: '搜索片库' })).toHaveValue('滚动');
  const after = await page.evaluate(() => document.querySelector<HTMLElement>('.mobile-content')?.scrollTop ?? -1);
  expect(before).toBeGreaterThan(0); expect(after).toBeGreaterThan(0);
});

test('M1.2 filter sheet focuses first control, traps both Tab directions, and restores focus', async ({ page }) => {
  await setupMockIpc(page); await page.goto('/'); await page.getByRole('button', { name: '打开筛选' }).click(); const dialog = page.getByRole('dialog', { name: '筛选片库' }); await expect(dialog).toBeVisible(); const first = dialog.getByRole('button', { name: '关闭筛选' }); const last = dialog.getByRole('button', { name: '应用筛选' }); await expect(first).toBeFocused(); await page.keyboard.press('Shift+Tab'); await expect(last).toBeFocused(); await page.keyboard.press('Tab'); await expect(first).toBeFocused(); await page.keyboard.press('Escape'); await expect(dialog).toHaveCount(0); await expect(page.getByRole('button', { name: '打开筛选' })).toBeFocused();
  await page.getByRole('button', { name: '添加记录' }).first().click(); await page.getByPlaceholder('请输入中文名称').fill('草稿'); let message = ''; page.once('dialog', dialog => { message = dialog.message(); void dialog.dismiss(); }); await page.getByRole('button', { name: '取消' }).click(); expect(message).toContain('未保存'); await expect(page.getByRole('dialog', { name: '添加新记录' })).toBeVisible();
});

test('M1.2 narrow layout keeps touch targets at least 48px', async ({ page }) => {
  await page.setViewportSize({ width: 360, height: 740 }); await setupMockIpc(page, { records: [record('窄屏记录')] }); await page.goto('/'); const buttons = await page.locator('.mobile-library button').all(); for (const button of buttons) { const box = await button.boundingBox(); expect(box?.height ?? 0).toBeGreaterThanOrEqual(48); }
  const horizontalOverflow = await page.evaluate(() => Math.max(document.body.scrollWidth, document.documentElement.scrollWidth) - window.innerWidth);
  expect(horizontalOverflow).toBeLessThanOrEqual(0);
});

test('M1.2 library refinement compresses the header and keeps search/count semantics', async ({ page }) => {
  await setupMockIpc(page, { records: [record('在看记录', { status: '在看' }), record('已看记录')] });
  await page.goto('/');
  await expect(page.locator('.mobile-topbar')).toHaveCount(0);
  await expect(page.getByText('WatchTracker', { exact: true })).toHaveCount(0);
  await expect(page.getByText('离线优先', { exact: true })).toHaveCount(0);
  await expect(page.getByText('OFFLINE LIBRARY', { exact: true })).toHaveCount(0);
  await expect(page.getByRole('heading', { name: '我的片库' })).toBeVisible();
  await expect(page.locator('.mobile-section-heading')).toContainText('2');
  const search = page.getByRole('textbox', { name: '搜索片库' });
  await search.fill('在看');
  await expect(page.locator('.mobile-section-heading')).toContainText('1 / 2');
  await expect(page.getByRole('button', { name: '清空搜索' })).toBeVisible();
  await page.getByRole('button', { name: '清空搜索' }).click();
  await expect(search).toHaveValue('');
  await expect(page.locator('.mobile-section-heading')).toContainText('2');
  await expect(page.locator('.mobile-search-row')).toHaveCount(1);
  await expect(page.locator('.mobile-toolbar-row')).toHaveCount(1);
  const sort = page.getByRole('combobox', { name: '排序' });
  await expect(sort.locator('option')).toHaveText(['最新添加', '完成时间', '上映年份', '评分']);
  await sort.selectOption('rating');
  await expect(sort).toHaveValue('rating');
  await page.getByRole('button', { name: '打开筛选' }).click();
  await page.locator('fieldset').filter({ hasText: '状态' }).getByRole('button', { name: '在看' }).click();
  await page.getByRole('button', { name: '应用筛选' }).click();
  await expect(page.getByRole('button', { name: '移除状态：在看' })).toHaveText('在看 ×');
  await expect(page.locator('.mobile-section-heading')).toContainText('1 / 2');
});

test('M1.2 record actions use a focused bottom sheet and preserve locked safety', async ({ page }) => {
  await setupMockIpc(page, { records: [record('普通记录'), record('锁定记录', { isLocked: true })] });
  await page.goto('/');
  const ordinaryCard = page.locator('[data-record-id="普通记录"]');
  await expect(ordinaryCard.getByRole('button', { name: '编辑', exact: true })).toHaveCount(0);
  await expect(ordinaryCard.getByRole('button', { name: '锁定', exact: true })).toHaveCount(0);
  await expect(ordinaryCard.getByRole('button', { name: '删除', exact: true })).toHaveCount(0);
  const trigger = page.getByRole('button', { name: '更多操作：普通记录' });
  await trigger.click();
  const sheet = page.getByRole('dialog', { name: '普通记录' });
  await expect(sheet).toBeVisible();
  await expect(sheet.getByRole('button', { name: '编辑' })).toBeFocused();
  for (const button of await sheet.getByRole('button').all()) { const box = await button.boundingBox(); expect(box?.height ?? 0).toBeGreaterThanOrEqual(48); }
  await page.keyboard.press('Escape');
  await expect(sheet).toHaveCount(0);
  await expect(trigger).toBeFocused();

  await trigger.click();
  await page.getByRole('button', { name: '取消' }).click();
  await expect(sheet).toHaveCount(0);
  await trigger.click();
  await page.mouse.click(4, 4);
  await expect(sheet).toHaveCount(0);

  const lockedTrigger = page.getByRole('button', { name: '更多操作：锁定记录' });
  await lockedTrigger.click();
  const lockedSheet = page.getByRole('dialog', { name: '锁定记录' });
  await expect(lockedSheet.getByRole('button', { name: '解锁' })).toBeVisible();
  await expect(lockedSheet.getByRole('button', { name: '编辑' })).toHaveCount(0);
  await expect(lockedSheet.getByRole('button', { name: '删除' })).toHaveCount(0);
  await page.evaluate(() => window.__WATCHTRACKER_ANDROID_BACK__?.());
  await expect(lockedSheet).toHaveCount(0);
  await expect(page).toHaveURL('#library');
  await expect(lockedTrigger).toBeFocused();
});

test('M1.2 list cards retain poster fallback and primary quick action as the only mobile library view', async ({ page }) => {
  await setupMockIpc(page, { records: [record('列表电影'), record('列表剧集', { mediaType: '剧集', totalEpisodes: 4, episodeTrackingEnabled: true, nextEpisode: 2, status: '在看' })] });
  await page.goto('/');
  await expect(page.locator('.mobile-record-thumbnail').first()).toBeVisible();
  await expect(page.locator('.mobile-poster-fallback').first()).toContainText('无图');
  await expect(page.getByRole('button', { name: '完成第 2 集' })).toBeVisible();
  await expect(page.getByRole('button', { name: '列表', exact: true })).toHaveCount(0);
  await expect(page.getByRole('button', { name: '海报', exact: true })).toHaveCount(0);
  await expect(page.locator('.mobile-poster-grid')).toHaveCount(0);
  await expect(page.locator('.mobile-record-list')).toBeVisible();
});
