import { expect, test } from '@playwright/test';
import type { WatchRecord } from '../src/shared/types';
import { mockSnapshot, setupMockIpc } from './fixtures/mockIpc';

function record(id: string, overrides: Partial<WatchRecord> = {}): WatchRecord {
  return {
    id,
    originalName: id,
    chineseName: id,
    progress: '',
    totalEpisodes: null,
    episodeTrackingEnabled: false,
    nextEpisode: null,
    movieProgress: null,
    movieDuration: 90 * 60,
    releaseYear: '2026',
    posterPath: null,
    status: '未看',
    platform: 'Netflix',
    rating: null,
    startDate: '',
    endDate: '',
    notes: '',
    createdAt: '2026-08-01T00:00:00.000Z',
    updatedAt: '2026-08-01T00:00:00.000Z',
    imdbId: null,
    isLocked: false,
    genres: '科幻',
    imdbRating: 7,
    tmdbStatus: null,
    interestLevel: 3,
    episodeRuntime: null,
    mediaType: '电影',
    contentTags: null,
    rev: 1,
    revActor: 'seed',
    ...overrides,
  };
}

async function openDashboard(page: import('@playwright/test').Page) {
  await page.getByRole('button', { name: '更多操作' }).click();
  await page.getByRole('menuitem', { name: '数据看板' }).click();
  await expect(page.getByRole('heading', { name: '观看概览' })).toBeVisible();
}

test('@discovery recommends locked unwatched records, excludes other statuses, and stays read-only', async ({ page }) => {
  await setupMockIpc(page, {
    records: [
      record('锁定科幻片', { isLocked: true, interestLevel: 5, imdbRating: 9 }),
      record('普通待看电影', { interestLevel: 2 }),
      record('正在观看作品', { status: '在看', interestLevel: 5, imdbRating: 10 }),
      record('已经看完作品', { status: '已看', interestLevel: 5, imdbRating: 10 }),
    ],
  });
  await page.goto('/');
  await openDashboard(page);
  const discovery = page.getByRole('region', { name: '今晚看什么推荐' });

  await expect(discovery.getByRole('heading', { name: '锁定科幻片' })).toBeVisible();
  await expect(discovery.getByText('已锁定 · 可查看')).toBeVisible();
  await expect(discovery.getByText('正在观看作品')).toHaveCount(0);
  await expect(discovery.getByText('已经看完作品')).toHaveCount(0);

  await page.getByRole('button', { name: '查看条目' }).click();
  await expect(page.getByRole('region', { name: '锁定科幻片' })).toBeVisible();
  await page.getByRole('button', { name: '关闭条目摘要' }).click();

  const snapshot = await mockSnapshot(page);
  const forbiddenWrites = new Set([
    'insert_record', 'update_record', 'delete_record', 'replace_all_records', 'replace_library',
    'enable_episode_tracking', 'set_next_episode', 'commit_sync_result', 'prepare_sync_publish',
  ]);
  expect(snapshot.calls.filter(call => forbiddenWrites.has(call.command))).toEqual([]);
});

test('@discovery does not repeat a round and keeps explicit skips across filters only until close', async ({ page }) => {
  await setupMockIpc(page, {
    records: [
      record('首选电影', { interestLevel: 5, imdbRating: 9 }),
      record('备选电影', { interestLevel: 4, imdbRating: 8 }),
    ],
  });
  await page.goto('/');
  await openDashboard(page);
  const discovery = page.getByRole('region', { name: '今晚看什么推荐' });

  await expect(discovery.getByRole('heading', { name: '首选电影' })).toBeVisible();
  await page.getByRole('button', { name: '本轮跳过' }).click();
  await expect(discovery.getByRole('heading', { name: '备选电影' })).toBeVisible();

  await page.getByLabel('推荐媒体类型').selectOption('电影');
  await expect(discovery.getByRole('heading', { name: '备选电影' })).toBeVisible();
  await expect(discovery.getByText('首选电影')).toHaveCount(0);

  await page.getByRole('button', { name: '换一个' }).click();
  await expect(page.getByRole('heading', { name: '本轮候选已看完' })).toBeVisible();
  await page.getByRole('button', { name: '重新浏览' }).click();
  await expect(discovery.getByRole('heading', { name: '备选电影' })).toBeVisible();

  await page.getByRole('button', { name: '关闭' }).click();
  await openDashboard(page);
  await expect(page.getByRole('region', { name: '今晚看什么推荐' }).getByRole('heading', { name: '首选电影' })).toBeVisible();
});

test('@discovery combines platform, media, completion, and single-episode duration filters', async ({ page }) => {
  await setupMockIpc(page, {
    records: [
      record('连载剧', { mediaType: '剧集', totalEpisodes: 12, episodeRuntime: 42, platform: 'Apple TV+', tmdbStatus: 'Returning Series', interestLevel: 5 }),
      record('完结迷你剧', { mediaType: '剧集', totalEpisodes: 6, episodeRuntime: null, platform: 'Netflix', tmdbStatus: 'Miniseries', interestLevel: 4 }),
      record('长电影', { movieDuration: 150 * 60, platform: 'Netflix', interestLevel: 5 }),
    ],
  });
  await page.goto('/');
  await openDashboard(page);

  await page.getByLabel('推荐媒体类型').selectOption('剧集');
  await page.getByLabel('推荐平台').selectOption('Apple TV+');
  await page.getByText('仅已完结').click();
  await expect(page.getByRole('heading', { name: '没有符合条件的已完结作品' })).toBeVisible();

  await page.getByText('仅已完结').click();
  await expect(page.getByRole('heading', { name: '连载剧' })).toBeVisible();
  await expect(page.getByText(/单集约 42 分/)).toBeVisible();

  await page.getByLabel('推荐平台').selectOption('Netflix');
  await page.getByText('仅已完结').click();
  await expect(page.getByRole('heading', { name: '完结迷你剧' })).toBeVisible();
  await expect(page.getByText('单集时长未知，按 45 分钟估算')).toBeVisible();
});

test('@display-title hides a redundant mainland first season without rewriting stored identity', async ({ page }) => {
  const mainlandSeries = record('mainland-season-one', {
    chineseName: '大陆示例剧 第一季',
    originalName: 'Mainland Example Season 1',
    mediaType: '剧集',
    totalEpisodes: 12,
    episodeRuntime: 45,
    originCountry: 'CN',
  });
  await setupMockIpc(page, { records: [mainlandSeries] });
  await page.goto('/');

  await expect(page.getByText('大陆示例剧', { exact: true })).toBeVisible();
  await expect(page.getByText('大陆示例剧 第一季', { exact: true })).toHaveCount(0);
  await expect(page.getByText('Mainland Example', { exact: true })).toBeVisible();

  await openDashboard(page);
  const discovery = page.getByRole('region', { name: '今晚看什么推荐' });
  await expect(discovery.getByRole('heading', { name: '大陆示例剧' })).toBeVisible();
  await discovery.getByRole('button', { name: '查看条目' }).click();
  await expect(page.getByRole('region', { name: '大陆示例剧' })).toContainText('Mainland Example');

  const snapshot = await mockSnapshot(page);
  expect(snapshot.records[0]).toMatchObject({
    chineseName: '大陆示例剧 第一季',
    originalName: 'Mainland Example Season 1',
  });
});

test('@dashboard-progress shows movie elapsed time, duration, and percentage', async ({ page }) => {
  await setupMockIpc(page, {
    records: [record('汉密尔顿', {
      status: '在看',
      movieProgress: 4020,
      movieDuration: 9600,
    })],
  });
  await page.goto('/');
  await openDashboard(page);

  await expect(page.getByText('已观看 1 小时 7 分钟 / 2 小时 40 分钟 · 42%')).toBeVisible();
  const snapshot = await mockSnapshot(page);
  expect(snapshot.records[0]).toMatchObject({ movieProgress: 4020, movieDuration: 9600 });
});
