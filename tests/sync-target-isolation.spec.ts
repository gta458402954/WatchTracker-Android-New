import { expect, test } from '@playwright/test';
import { mockSnapshot, setupMockIpc } from './fixtures/mockIpc';

const settings = {
  webdav_creds: 'encrypted:user:password',
  webdav_url: 'https://old.example.test/dav/',
};

async function openSyncSettings(page: import('@playwright/test').Page) {
  await page.goto('/');
  await page.getByRole('button', { name: '设置' }).click();
  await page.getByRole('button', { name: '☁️ 云端同步', exact: true }).click();
  await expect(page.getByText(/已保存目标/)).toBeVisible();
  await page.getByRole('button', { name: '切换或更新凭据' }).click();
}

test('@sync-target-display shows a friendly safe address without overflowing actions', async ({ page }) => {
  await setupMockIpc(page, {
    settings: {
      webdav_creds: 'encrypted:gtazhuce@qq.com:password',
      webdav_url: 'https://dav.jianguoyun.com/dav/%E5%BD%B1%E8%A7%86%E8%BF%BD%E8%B8%AA/?token=hidden',
    },
  });
  await page.setViewportSize({ width: 760, height: 760 });
  await page.goto('/');
  await page.getByRole('button', { name: '设置' }).click();
  await page.getByRole('button', { name: '☁️ 云端同步', exact: true }).click();

  await expect(page.getByText('WebDAV 已连接')).toBeVisible();
  await expect(page.getByText(/坚果云 · \/影视追踪/).first()).toBeVisible();
  await expect(page.getByText('自动同步：已开启')).toBeVisible();
  await expect(page.getByRole('button', { name: '切换或更新凭据' })).toBeVisible();
  await expect(page.getByRole('button', { name: '断开连接' })).toBeVisible();
  await expect(page.getByText(/断开只移除当前连接/)).toBeVisible();
  await expect(page.getByText(/token=hidden/)).toHaveCount(0);

  const horizontalOverflow = await page.locator('body').evaluate(element => element.scrollWidth > element.clientWidth);
  expect(horizontalOverflow).toBe(false);
});

test('@sync-target-isolation cancelling a read-only probe never activates or writes the new target', async ({ page }) => {
  await setupMockIpc(page, { settings });
  await openSyncSettings(page);
  await page.getByPlaceholder('WebDAV 服务器地址').fill('https://new.example.test/dav/');
  await page.getByPlaceholder('用户名').fill('new-user');
  await page.getByPlaceholder('WebDAV 密码 / 应用密码').fill('new-password');
  page.once('dialog', dialog => dialog.dismiss());
  await page.getByRole('button', { name: '只读检查并更新目标' }).click();
  await expect(page.getByText(/已取消切换/)).toBeVisible();

  const snapshot = await mockSnapshot(page);
  const probeCalls = snapshot.calls.filter(call => call.command === 'probe_webdav_request');
  expect(probeCalls.length).toBeGreaterThan(0);
  expect(probeCalls.every(call => call.args.request.method === 'GET')).toBe(true);
  expect(snapshot.calls.some(call => call.command === 'activate_sync_target')).toBe(false);
  expect(snapshot.settings.webdav_url).toBe('https://old.example.test/dav/');
});

test('@sync-target-isolation password rotation keeps the same target and proceeds without a switch prompt', async ({ page }) => {
  await setupMockIpc(page, { settings });
  await openSyncSettings(page);
  await page.getByPlaceholder('WebDAV 密码 / 应用密码').fill('rotated-password');
  let dialogCount = 0;
  page.on('dialog', async dialog => { dialogCount += 1; await dialog.accept(); });
  await page.getByRole('button', { name: '只读检查并更新目标' }).click();
  await expect(page.getByText('✅ 目标已激活并完成首次同步')).toBeVisible();

  const snapshot = await mockSnapshot(page);
  const activation = snapshot.calls.findIndex(call => call.command === 'activate_sync_target');
  expect(activation).toBeGreaterThan(-1);
  expect(snapshot.calls.slice(0, activation).some(call => call.command === 'webdav_request')).toBe(false);
  expect(dialogCount).toBe(0);
});

test('@sync-target-isolation reports HTTP 409 as an inaccessible target directory', async ({ page }) => {
  await setupMockIpc(page, { settings, webdavFailureStatus: 409, webdavFailureCount: 1 });
  await openSyncSettings(page);
  await page.getByPlaceholder('WebDAV 服务器地址').fill('https://new.example.test/dav/');
  await page.getByPlaceholder('用户名').fill('new-user');
  await page.getByPlaceholder('WebDAV 密码 / 应用密码').fill('new-password');
  await page.getByRole('button', { name: '只读检查并更新目标' }).click();
  await expect(page.getByText('WebDAV 目标目录不存在或无法访问，请确认目录已创建后重试。', { exact: true })).toBeVisible();
  const snapshot = await mockSnapshot(page);
  expect(snapshot.calls.some(call => call.command === 'activate_sync_target')).toBe(false);
});

test('@credential-boundary saved requests never expose credentials to the frontend', async ({ page }) => {
  await setupMockIpc(page, { settings });
  await page.goto('/');
  await page.getByRole('button', { name: '设置' }).click();
  await page.getByRole('button', { name: '☁️ 云端同步', exact: true }).click();
  await page.getByRole('button', { name: '立即同步到云端' }).click();

  const snapshot = await mockSnapshot(page);
  const storedCalls = snapshot.calls.filter(call => call.command === 'webdav_request');
  expect(storedCalls.length).toBeGreaterThan(0);
  for (const call of storedCalls) {
    expect(JSON.stringify(call.args)).not.toContain('password');
    expect(JSON.stringify(call.args)).not.toContain('username');
  }
  const connection = snapshot.calls.find(call => call.command === 'get_active_sync_connection');
  expect(JSON.stringify(connection)).not.toContain('password');
});
