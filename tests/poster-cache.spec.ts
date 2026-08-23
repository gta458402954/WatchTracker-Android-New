import { expect, test } from '@playwright/test';
import type { WatchRecord } from '../src/shared/types';
import { mockSnapshot, setupMockIpc } from './fixtures/mockIpc';

function posterRecord(): WatchRecord {
  return {
    id: 'poster-record',
    originalName: 'Poster Protocol Test',
    chineseName: '海报协议测试',
    progress: '',
    totalEpisodes: null,
    episodeTrackingEnabled: false,
    nextEpisode: null,
    movieProgress: null,
    movieDuration: null,
    releaseYear: '2026',
    posterPath: '/2baf1e.jpg',
    status: '未看',
    platform: '',
    rating: null,
    startDate: null,
    endDate: null,
    notes: '',
    createdAt: '2026-08-09T00:00:00.000Z',
    updatedAt: null,
    imdbId: 'tt0000001',
    isLocked: false,
    genres: null,
    originCountry: null,
    imdbRating: null,
    tmdbStatus: null,
    interestLevel: null,
    episodeRuntime: null,
    mediaType: '电影',
    contentTags: null,
    rev: 0,
    revActor: '',
  };
}

test('@poster-cache maintenance is explicit and never writes business records', async ({ page }) => {
  await setupMockIpc(page, { records: [] });
  await page.goto('/');
  await page.getByRole('button', { name: '设置' }).click();
  await page.getByRole('button', { name: /系统工具/ }).click();

  const cache = page.getByLabel('海报缓存');
  await expect(cache).toContainText('自动清理永不删除仍被条目引用的海报');
  await expect(cache).toContainText('建议上限：500.0 MB');

  page.once('dialog', dialog => dialog.accept());
  await cache.getByRole('button', { name: '清理未引用缓存' }).click();
  await expect(cache).toContainText('未引用海报缓存已清理');

  const snapshot = await mockSnapshot(page);
  expect(snapshot.calls.some(call => call.command === 'clean_poster_cache' && call.args.mode === 'unreferenced')).toBe(true);
  expect(snapshot.calls.some(call => ['insert_record', 'update_record', 'delete_record', 'replace_all_records', 'replace_library'].includes(call.command))).toBe(false);
});

test('@poster-cache Windows source retries once through the controlled downloader', async ({ page }) => {
  const posterRequests: string[] = [];
  await page.route('http://poster.localhost/**', route => {
    posterRequests.push(route.request().url());
    return route.abort();
  });
  await setupMockIpc(page, { records: [posterRecord()] });
  await page.goto('/');
  await page.getByRole('button', { name: '更多操作' }).click();
  await page.getByRole('menuitem', { name: '切换至海报墙' }).click();

  await expect(page.getByRole('button', { name: '重试海报' })).toBeVisible();
  await expect.poll(() => posterRequests).toEqual([
    'http://poster.localhost/2baf1e.jpg?v=0',
    'http://poster.localhost/2baf1e.jpg?v=1',
  ]);

  await expect.poll(async () => {
    const snapshot = await mockSnapshot(page);
    return snapshot.calls.filter(call => call.command === 'download_poster');
  }).toEqual([{
    command: 'download_poster',
    args: { path: '/2baf1e.jpg', proxy: null, size: 'w342' },
  }]);
});
