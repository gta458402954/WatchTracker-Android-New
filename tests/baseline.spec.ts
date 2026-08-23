import { expect, test } from '@playwright/test';
import { mockSnapshot, setupMockIpc } from './fixtures/mockIpc';

test('ready empty data renders the normal empty state', async ({ page }) => {
  await setupMockIpc(page);
  await page.goto('/');

  await expect(page.getByText('还没有记录，快去添加吧！')).toBeVisible();
  await expect(page.getByRole('alert')).toHaveCount(0);
  await expect(page.getByTestId('build-commit')).toHaveText(/^git (?:[0-9a-f]{7,40}|unknown)$/);
  expect(await page.getByTestId('build-commit').evaluate(
    element => element.nextElementSibling?.textContent,
  )).toBe('影视追踪');
});

test('about settings shows one consistent responsive build identity', async ({ page }) => {
  await setupMockIpc(page);
  await page.setViewportSize({ width: 760, height: 760 });
  await page.goto('/');
  const headerCommit = (await page.getByTestId('build-commit').textContent())?.replace(/^git /, '');
  await page.getByRole('button', { name: '设置' }).click();
  await page.getByRole('button', { name: /关于/ }).click();

  const about = page.getByTestId('about-settings');
  await expect(about.getByText('WatchTracker')).toBeVisible();
  await expect(about.getByText('影视追踪')).toBeVisible();
  await expect(page.getByTestId('about-product-version')).toHaveText(/^v\d+\.\d+\.\d+/);
  await expect(page.getByTestId('about-git-commit')).toHaveText(headerCommit || 'unknown');
  await expect(page.getByTestId('about-commit-time')).not.toBeEmpty();
  await expect(about.getByText(/构建或安装时间/)).toBeVisible();

  const horizontalOverflow = await page.locator('body').evaluate(element => element.scrollWidth > element.clientWidth);
  expect(horizontalOverflow).toBe(false);
});

test('initialization failure shows an error, not empty data, and retry recovers', async ({ page }) => {
  await setupMockIpc(page, { failRecordLoads: true });
  await page.goto('/');

  const alert = page.getByRole('alert');
  await expect(alert).toContainText('无法读取本地数据');
  await expect(alert.getByRole('button', { name: '重试加载' })).toBeVisible();
  await expect(page.getByText('还没有记录，快去添加吧！')).toHaveCount(0);

  await page.evaluate(() => {
    window.__WATCHTRACKER_TEST__.failRecordLoads = false;
  });
  await alert.getByRole('button', { name: '重试加载' }).click();
  await expect(page.getByText('还没有记录，快去添加吧！')).toBeVisible();
  await expect(page.getByRole('alert')).toHaveCount(0);

  const snapshot = await mockSnapshot(page);
  expect(snapshot.calls.filter(({ command }) => command === 'get_all_records').length).toBeGreaterThanOrEqual(2);
});

test('V20 database is explicitly rejected without attempting record or setting reads', async ({ page }) => {
  await setupMockIpc(page, {
    databaseCompatibilityIssue: {
      code: 'unsupported_newer_database', detectedVersion: 20, supportedVersion: 18,
    },
  });
  await page.goto('/');

  await expect(page.getByRole('alert')).toContainText('数据库版本不兼容');
  await expect(page.getByRole('alert')).toContainText('检测到 V20 数据库');
  await expect(page.getByRole('alert')).toContainText('该数据库未被修改');
  const calls = (await mockSnapshot(page)).calls.map(call => call.command);
  expect(calls).toContain('get_database_compatibility');
  expect(calls).not.toContain('get_all_records');
  expect(calls).not.toContain('get_setting');
});

test('failed V19 downgrade reports rollback guidance and blocks reads', async ({ page }) => {
  await setupMockIpc(page, {
    databaseCompatibilityIssue: {
      code: 'v19_downgrade_failed', detectedVersion: 19, supportedVersion: 18,
    },
  });
  await page.goto('/');

  const alert = page.getByRole('alert');
  await expect(alert).toContainText('V19 数据库自动转换失败');
  await expect(alert).toContainText('原迁移事务已回滚');
  const calls = (await mockSnapshot(page)).calls.map(call => call.command);
  expect(calls).not.toContain('get_all_records');
  expect(calls).not.toContain('get_setting');
});

test('successful V19 downgrade notice is shown once and then cleared', async ({ page }) => {
  await setupMockIpc(page, {
    settings: { database_migration_notice: 'v19_to_v18' },
  });
  await page.goto('/');

  await expect(page.getByRole('status')).toContainText('数据库已安全从 V19 转换为 V18');
  await expect.poll(async () => (await mockSnapshot(page)).settings.database_migration_notice).toBe('');
});

test('current IPC DTO supports create, update, and delete without permissive fallbacks', async ({ page }) => {
  await setupMockIpc(page);
  await page.goto('/');
  await expect(page.getByText('还没有记录，快去添加吧！')).toBeVisible();

  await page.getByRole('button', { name: '更多操作' }).click();
  await page.getByRole('menuitem', { name: /添加记录/ }).click();
  await expect(page.getByRole('heading', { name: '添加新记录' })).toBeVisible();
  await page.getByPlaceholder('请输入中文名称').fill('A007 自动化记录');
  await page.getByPlaceholder('英文 / 原名').fill('A007 Automated Record');
  await page.getByRole('button', { name: '添加记录' }).click();
  await expect(page.getByText('A007 自动化记录')).toBeVisible();

  await page.getByTitle('编辑').click();
  await expect(page.getByRole('heading', { name: '编辑记录' })).toBeVisible();
  await page.getByPlaceholder('请输入中文名称').fill('A007 自动化记录（已修改）');
  await page.getByRole('button', { name: '保存修改' }).click();
  await expect(page.getByText('A007 自动化记录（已修改）')).toBeVisible();

  page.once('dialog', (dialog) => dialog.accept());
  await page.getByTitle('删除').click();
  await expect(page.getByText('A007 自动化记录（已修改）')).toHaveCount(0);
  await expect(page.getByText('还没有记录，快去添加吧！')).toBeVisible();

  const snapshot = await mockSnapshot(page);
  expect(snapshot.records).toHaveLength(0);
  expect(snapshot.calls.map(({ command }) => command)).toEqual(expect.arrayContaining([
    'insert_record',
    'update_record',
    'delete_record',
  ]));
});
