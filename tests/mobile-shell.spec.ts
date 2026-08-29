import { expect, test } from '@playwright/test';
import type { WatchRecord } from '../src/shared/types';
import { mockSnapshot, setupMockIpc } from './fixtures/mockIpc';

test.use({ userAgent: 'Mozilla/5.0 (Linux; Android 16; Pixel) AppleWebKit/537.36 Chrome/140 Mobile' });

function record(id: string, overrides: Partial<WatchRecord> = {}): WatchRecord {
  return {
    id, originalName: `${id} original`, chineseName: id, progress: '', totalEpisodes: null,
    movieProgress: null, movieDuration: null, releaseYear: '2026', posterPath: null,
    status: '未看', platform: '', rating: null, startDate: '', endDate: '', notes: '',
    createdAt: '2026-08-09T00:00:00.000Z', imdbId: null, mediaType: '电影', contentTags: null, originCountry: 'CN',
    ...overrides,
  };
}

test('Android runtime renders the mobile shell and local CRUD form', async ({ page }) => {
  await setupMockIpc(page);
  await page.goto('/');

  await expect(page.getByRole('navigation', { name: '主导航' })).toBeVisible();
  await expect(page.getByRole('button', { name: '添加记录' })).toBeVisible();
  await expect(page.getByRole('heading', { name: '片库还是空的' })).toBeVisible();

  await page.getByRole('button', { name: '添加记录' }).first().click();
  const dialog = page.getByRole('dialog', { name: '添加新记录' });
  await expect(dialog).toBeVisible();
  await expect(page.getByRole('button', { name: '自动填充' })).toHaveCount(0);
  await dialog.getByPlaceholder('请输入中文名称').fill('移动端测试记录');
  await dialog.getByRole('button', { name: '添加记录', exact: true }).click();
  await expect(page.getByText('移动端测试记录')).toBeVisible();

  await page.getByRole('button', { name: '设置' }).click();
  await expect(page.getByRole('heading', { name: '设置', exact: true })).toBeVisible();
  await expect(page.getByRole('heading', { name: '备份与恢复' })).toBeVisible();
  await expect(page.getByText('WebDAV 密码由 Android Keystore 保护，不会回显到页面。')).toBeVisible();
  expect(await page.evaluate(() => window.__WATCHTRACKER_ANDROID_BACK__?.() ?? 'exit')).toBe('history');
  await page.evaluate(() => history.back());
  await expect(page.getByRole('heading', { name: '我的片库' })).toBeVisible();
});

test('unfinished mobile entries remain placeholders while settings is functional', async ({ page }) => {
  await setupMockIpc(page);
  await page.goto('/');
  for (const label of ['发现', '收藏', '统计']) {
    await page.getByRole('button', { name: label }).click();
    await expect(page.getByRole('heading', { name: label })).toBeVisible();
    await expect(page.getByText('此功能正在开发中')).toBeVisible();
  }
  await page.getByRole('button', { name: '片库' }).click();
  await expect(page.getByRole('heading', { name: '片库还是空的' })).toBeVisible();
  const commands = (await mockSnapshot(page)).calls.map(call => call.command);
  expect(commands.filter(command => /insert|update|delete/.test(command))).toEqual([]);
});

test('selecting library from settings replaces the tab entry instead of returning to settings', async ({ page }) => {
  await setupMockIpc(page);
  await page.goto('/');
  await page.getByRole('button', { name: '设置' }).click();
  await page.getByRole('button', { name: '片库' }).click();
  await expect(page.getByRole('heading', { name: '片库还是空的' })).toBeVisible();
  const result = await page.evaluate(() => {
    const length = history.length;
    const consumed = window.__WATCHTRACKER_ANDROID_BACK__?.() ?? null;
    return { hash: location.hash, state: history.state, length, afterLength: history.length, consumed };
  });
  expect(result).toMatchObject({ hash: '#library', state: { mobileRoute: 'library' }, consumed: 'exit' });
  expect(result.afterLength).toBe(result.length);
  await expect(page.getByRole('heading', { name: '片库还是空的' })).toBeVisible();
});

test('system back closes the form before asking Android to exit the root', async ({ page }) => {
  await setupMockIpc(page);
  await page.goto('/');
  await page.getByRole('button', { name: '添加记录' }).first().click();
  await expect(page.getByRole('dialog', { name: '添加新记录' })).toBeVisible();
  const consumed = await page.evaluate(() => window.__WATCHTRACKER_ANDROID_BACK__?.() ?? 'exit');
  expect(consumed).toBe('history');
  await page.evaluate(() => history.back());
  await expect(page.getByRole('dialog', { name: '添加新记录' })).toHaveCount(0);
  await expect(page.getByRole('heading', { name: '片库还是空的' })).toBeVisible();
});

test('mobile initialization can recover after a failed local read', async ({ page }) => {
  await setupMockIpc(page, { failRecordLoads: true });
  await page.goto('/');
  const alert = page.getByRole('alert');
  await expect(alert).toContainText('无法读取本地数据');
  await page.evaluate(() => { window.__WATCHTRACKER_TEST__.failRecordLoads = false; });
  await alert.getByRole('button', { name: '重试' }).click();
  await expect(page.getByRole('heading', { name: '片库还是空的' })).toBeVisible();
});

test('locked mobile records stay visible but expose no edit/delete action', async ({ page }) => {
  await setupMockIpc(page, { records: [record('锁定记录', { isLocked: true })] });
  await page.goto('/');
  const card = page.locator('.mobile-record-card');
  await expect(card).toContainText('🔒 已锁定');
  await expect(card.getByRole('button', { name: '编辑' })).toHaveCount(0);
  await expect(card.getByRole('button', { name: '删除' })).toHaveCount(0);
  const commands = (await mockSnapshot(page)).calls.map(call => call.command);
  expect(commands.filter(command => command === 'update_record' || command === 'delete_record')).toEqual([]);
});

test('mobile record form keeps key interactions at least 48 CSS pixels', async ({ page }) => {
  await setupMockIpc(page);
  await page.goto('/');
  await page.getByRole('button', { name: '添加记录' }).first().click();
  const dialog = page.getByRole('dialog', { name: '添加新记录' });
  await expect(dialog).toBeVisible();
  for (const button of await dialog.locator('button').all()) {
    const box = await button.boundingBox();
    expect(box?.width ?? 0).toBeGreaterThanOrEqual(48);
    expect(box?.height ?? 0).toBeGreaterThanOrEqual(48);
  }
  for (const input of await dialog.locator('input, select, textarea').all()) {
    expect((await input.boundingBox())?.height ?? 0).toBeGreaterThanOrEqual(48);
  }
  await expect(dialog.getByRole('button', { name: '期待值 5 分' })).toBeVisible();
});
