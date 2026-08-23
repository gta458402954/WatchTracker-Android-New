import { expect, test } from '@playwright/test';
import { setupMockIpc, mockSnapshot } from './fixtures/mockIpc';

test('@arch002 settings bootstrap runs once and tab navigation is read-only', async ({ page }) => {
  await setupMockIpc(page);
  await page.goto('/');
  const before = (await mockSnapshot(page)).calls;
  await page.getByRole('button', { name: '设置' }).click();
  const dialog = page.getByRole('dialog', { name: /设置中心/ });
  await expect(dialog).toBeVisible();
  await expect.poll(async () => (await mockSnapshot(page)).calls.length).toBeGreaterThan(before.length);
  const afterOpen = (await mockSnapshot(page)).calls;
  const bootstrapCommands = ['get_creds', 'get_sync_targets', 'get_tmdb_credential_status', 'get_setting', 'get_sync_conflicts'];
  const counts = Object.fromEntries(bootstrapCommands.map(command => [command, afterOpen.filter(call => call.command === command).length]));
  for (const tab of ['同步', '类型与标签', '系统工具', '关于', '基础配置']) {
    await dialog.getByRole('button', { name: new RegExp(tab) }).click();
    await page.waitForTimeout(30);
  }
  const afterTabs = (await mockSnapshot(page)).calls;
  for (const command of bootstrapCommands) {
    expect(afterTabs.filter(call => call.command === command).length, `${command} repeated after tab switch`).toBe(counts[command]);
  }
  const businessWrites = afterTabs.slice(before.length).filter(call => /^(update_|insert_|delete_|replace_|set_|save_|clear_|resolve_|vacuum|complete_|apply_|create_|add_)/.test(call.command));
  expect(businessWrites).toEqual([]);
});

test('@arch002 record form remains page-safe at 360px', async ({ page }) => {
  await setupMockIpc(page);
  await page.setViewportSize({ width: 360, height: 740 });
  await page.goto('/');
  await page.getByRole('button', { name: '更多操作' }).click();
  await page.getByRole('menuitem', { name: /添加记录/ }).click();
  const dialog = page.getByRole('dialog', { name: '添加新记录' });
  await expect(dialog).toBeVisible();
  expect(await page.locator('body').evaluate(element => element.scrollWidth > element.clientWidth)).toBe(false);
});
