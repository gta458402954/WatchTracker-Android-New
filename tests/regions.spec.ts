import { expect, test, type Page } from '@playwright/test';
import type { WatchRecord } from '../src/shared/types';
import { replaceMockRecords, setupMockIpc } from './fixtures/mockIpc';

function record(
  id: string,
  originCountry: string | null,
  overrides: Partial<WatchRecord> = {},
): WatchRecord {
  return {
    id,
    originalName: `${id} original`,
    chineseName: id,
    progress: '',
    totalEpisodes: null,
    movieProgress: null,
    movieDuration: null,
    releaseYear: '2025',
    posterPath: null,
    status: '未看',
    platform: '',
    rating: null,
    startDate: '',
    endDate: '',
    notes: '',
    createdAt: `2025-01-${String(Number(id.length % 20) + 1).padStart(2, '0')}T00:00:00.000Z`,
    imdbId: null,
    mediaType: '电影',
    contentTags: null,
    originCountry,
    ...overrides,
  };
}

async function regionButton(page: Page, label: string) {
  return page.getByLabel('地区筛选').getByRole('button', { name: new RegExp(`^${label} \\d+$`) });
}

test('renders dynamic counts in stable order and filters by code, including aliases and sentinels', async ({ page }) => {
  const records = [
    record('大陆记录', 'CN'),
    record('香港英国合拍', 'HK, GB'),
    record('台湾记录', 'TW'),
    record('英国别名记录', 'UK'),
    record('法国德国合拍', 'FR, DE'),
    record('未知记录', null, { contentTags: '悬疑' }),
    record('未映射记录', 'XX'),
  ];
  await setupMockIpc(page, { records });
  await page.goto('/');

  const buttons = page.getByLabel('地区筛选').getByRole('button');
  await expect(buttons).toHaveText([
    '中国大陆 1', '中国香港 1', '中国台湾 1', '英国 1',
    '法国 1', 'XX 1', '未知地区 1',
  ]);
  await expect(page.getByLabel('地区筛选').getByRole('button', { name: /^德国 / })).toHaveCount(0);

  await (await regionButton(page, '中国大陆')).click();
  await expect(page.getByText('大陆记录', { exact: true })).toBeVisible();
  await expect(page.getByText('香港英国合拍', { exact: true })).toHaveCount(0);
  await expect(await regionButton(page, '中国大陆')).toHaveAttribute('aria-pressed', 'true');

  await (await regionButton(page, '中国大陆')).click();
  await expect(page.getByText('香港英国合拍', { exact: true })).toBeVisible();
  await expect(await regionButton(page, '中国大陆')).toHaveAttribute('aria-pressed', 'false');

  for (const [label, visibleNames] of [
    ['中国香港', ['香港英国合拍']],
    ['中国台湾', ['台湾记录']],
    ['英国', ['英国别名记录']],
    ['未知地区', ['未知记录']],
    ['XX', ['未映射记录']],
  ] as const) {
    await (await regionButton(page, label)).click();
    for (const name of visibleNames) await expect(page.getByText(name, { exact: true })).toBeVisible();
    await (await regionButton(page, label)).click();
  }
});

test('media/status scope drives options while search does not, and unavailable saved conditions stay explicit', async ({ page }) => {
  const records = [
    record('CN 未看电影', 'CN', { isLocked: true }),
    record('FR 已看电影', 'FR', { status: '已看' }),
    record('CN 已看剧集', 'CN', { status: '已看', mediaType: '剧集' }),
  ];
  await setupMockIpc(page, { records });
  await page.goto('/');

  const initialOptions = await page.getByLabel('地区筛选').getByRole('button').allTextContents();
  await page.getByPlaceholder('搜索电影、剧集...').fill('不存在');
  await expect(page.getByLabel('地区筛选').getByRole('button')).toHaveText(initialOptions);
  await page.getByPlaceholder('搜索电影、剧集...').fill('');

  await page.getByRole('banner').getByRole('combobox').selectOption('rating');
  await expect(page.getByLabel('地区筛选').getByRole('button')).toHaveText(initialOptions);

  await page.getByTitle('显示全部锁定状态').click();
  await expect(page.getByLabel('地区筛选').getByRole('button')).toHaveText(initialOptions);
  await page.getByTitle('仅显示已锁定').click();
  await expect(page.getByLabel('地区筛选').getByRole('button')).toHaveText(initialOptions);
  await page.getByTitle('仅显示未锁定').click();

  await (await regionButton(page, '法国')).click();
  await expect(page.getByLabel('地区筛选').getByRole('button')).toHaveText(['法国 1', '中国大陆 2']);
  await page.getByRole('button', { name: /^未看 1$/ }).click();
  await expect(page.getByText('CN 未看电影', { exact: true })).toHaveCount(0);
  await expect(page.getByText('当前筛选没有匹配记录')).toBeVisible();
  await expect(page.getByLabel('当前高级筛选条件')).toHaveCount(0);
  await expect(page.getByLabel('地区筛选').getByRole('button', { name: '法国 0' })).toHaveAttribute('aria-pressed', 'true');

  await page.getByRole('button', { name: /^全部 3$/ }).last().click();
  await expect(await regionButton(page, '法国')).toHaveAttribute('aria-pressed', 'true');

  await page.getByRole('button', { name: /^剧集 1$/ }).click();
  await expect(page.getByLabel('地区筛选').getByRole('button')).toHaveText(['法国 0', '中国大陆 1']);
});

test('dynamic options react to add, edit, delete, and controlled record replacement', async ({ page }) => {
  await setupMockIpc(page, { records: [record('旧记录', null, { contentTags: '美国' })] });
  await page.goto('/');
  await expect(await regionButton(page, '美国')).toBeVisible();

  await page.getByTitle('编辑').click();
  await page.getByPlaceholder('如：韩国').fill('日本');
  await page.getByRole('button', { name: '保存修改' }).click();
  await expect(await regionButton(page, '日本')).toBeVisible();
  await expect(page.getByLabel('地区筛选').getByRole('button', { name: /^美国 / })).toHaveCount(0);

  page.once('dialog', dialog => dialog.accept());
  await page.getByTitle('删除').click();
  await expect(page.getByLabel('地区筛选')).toHaveCount(0);

  await page.getByRole('button', { name: '更多操作' }).click();
  await page.getByRole('menuitem', { name: /添加记录/ }).click();
  await page.getByPlaceholder('请输入中文名称').fill('新增韩国记录');
  await page.getByPlaceholder('如：韩国').fill('韩国');
  await page.getByRole('button', { name: '添加记录' }).click();
  await expect(await regionButton(page, '韩国')).toBeVisible();

  await replaceMockRecords(page, [record('受控法国记录', 'FR'), record('受控德国记录', 'DE')]);
  await expect(page.getByText('受控法国记录', { exact: true })).toBeVisible();
  await expect(page.getByLabel('地区筛选').getByRole('button')).toHaveText(['德国 1', '法国 1']);
});

test('hides the region bar for empty data', async ({ page }) => {
  await setupMockIpc(page);
  await page.goto('/');
  await expect(page.getByText('还没有记录，快去添加吧！')).toBeVisible();
  await expect(page.getByLabel('地区筛选')).toHaveCount(0);
});

test('displays BY with its Chinese country name', async ({ page }) => {
  await setupMockIpc(page, { records: [record('白俄罗斯记录', 'BY')] });
  await page.goto('/');

  await expect(await regionButton(page, '白俄罗斯')).toHaveText('白俄罗斯 1');
});

test('many dynamic regions collapse into an accessible overflow menu on narrow screens', async ({ page }) => {
  await page.setViewportSize({ width: 640, height: 900 });
  const codes = ['CN', 'HK', 'TW', 'US', 'JP', 'KR', 'GB', 'FR', 'DE', 'IT', 'ES', 'CA', 'AU', 'BR', 'XX'];
  await setupMockIpc(page, { records: codes.map(code => record(`记录-${code}`, code)) });
  await page.goto('/');

  const group = page.getByLabel('地区筛选');
  const directButtons = group.getByRole('button').filter({ hasNotText: '更多地区' });
  await expect(directButtons).toHaveCount(2);
  for (const button of await directButtons.all()) await expect(button).toHaveAttribute('aria-pressed', 'false');

  await group.getByRole('button', { name: /更多地区/ }).click();
  await expect(page.getByRole('menu', { name: '更多地区' }).getByRole('menuitemcheckbox')).toHaveCount(codes.length - 2);
  await page.getByRole('menuitemcheckbox', { name: /XX/ }).click();
  await expect(group.getByRole('button', { name: /XX/ })).toHaveAttribute('aria-pressed', 'true');
});

test('only the first country of one record enters the top filter', async ({ page }) => {
  await setupMockIpc(page, { records: [record('多国记录', 'US, CN, GB, BY, FR')] });
  await page.goto('/');

  await expect(page.getByLabel('地区筛选').getByRole('button')).toHaveText([
    '美国 1',
  ]);
  await expect(page.getByLabel('地区筛选').getByRole('button', { name: /^中国大陆 / })).toHaveCount(0);
  await expect(page.getByLabel('地区筛选').getByRole('button', { name: /^英国 / })).toHaveCount(0);
  await expect(page.getByLabel('地区筛选').getByRole('button', { name: /^白俄罗斯 / })).toHaveCount(0);
  await expect(page.getByLabel('地区筛选').getByRole('button', { name: /^法国 / })).toHaveCount(0);
});
