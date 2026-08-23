import { expect, test } from '@playwright/test';
import type { EpisodeCompletion, WatchRecord } from '../src/shared/types';
import type { SyncPayloadV3 } from '../src/shared/lib/syncMerge';
import { mockSnapshot, setupMockIpc } from './fixtures/mockIpc';

function series(overrides: Partial<WatchRecord> = {}): WatchRecord {
  return {
    id: 'episode-series',
    originalName: 'Episode Series',
    chineseName: '逐集测试剧',
    progress: '旧进度 E01',
    totalEpisodes: 6,
    episodeTrackingEnabled: false,
    nextEpisode: null,
    movieProgress: null,
    movieDuration: null,
    releaseYear: '2026',
    posterPath: null,
    status: '未看',
    platform: '',
    rating: null,
    startDate: '',
    endDate: '',
    notes: '',
    createdAt: '2026-08-01T00:00:00.000Z',
    updatedAt: '2026-08-01T00:00:00.000Z',
    imdbId: null,
    mediaType: '剧集',
    contentTags: null,
    originCountry: 'CN',
    rev: 1,
    revActor: 'seed',
    ...overrides,
  };
}

function completion(completedAt: string | null, episodeNumber = 1): EpisodeCompletion {
  return {
    id: `episode-completion-${episodeNumber}`,
    recordId: 'episode-series',
    episodeNumber,
    completedAt,
    createdAt: '2026-08-01T00:00:00.000Z',
    updatedAt: completedAt ?? '2026-08-01T00:00:00.000Z',
    rev: 2,
    revActor: 'local-device',
  };
}

function payload(record: WatchRecord, schemaVersion: 3 | 4, episodeCompletions: EpisodeCompletion[] = []): SyncPayloadV3 {
  return {
    schemaVersion,
    documentId: 'episode-document',
    revision: 1,
    commitId: 'episode-base',
    parentCommitId: null,
    writerId: 'remote-device',
    committedAt: '2026-08-01T00:00:00.000Z',
    records: [record],
    tombstones: [],
    episodeCompletions,
  };
}

test('@episode-history enables explicitly, records skips, completes, and preserves history on retreat', async ({ page }) => {
  await setupMockIpc(page, { records: [series()] });
  await page.goto('/');

  const selector = page.getByLabel('逐集测试剧 下一集');
  page.once('dialog', dialog => dialog.accept());
  await selector.selectOption('2');
  await expect(selector).toHaveValue('2');

  let snapshot = await mockSnapshot(page);
  expect(snapshot.records[0]).toMatchObject({
    progress: '旧进度 E01', episodeTrackingEnabled: true, nextEpisode: 2, status: '在看',
  });
  expect(snapshot.calls.some(call => call.command === 'enable_episode_tracking')).toBe(true);

  await selector.selectOption('5');
  await expect(selector).toHaveValue('5');
  snapshot = await mockSnapshot(page);
  expect(snapshot.episodeCompletions.map(item => [item.episodeNumber, item.completedAt === null])).toEqual([
    [2, true], [3, true], [4, false],
  ]);

  await page.getByRole('button', { name: '查看逐集完成历史' }).click();
  await expect(page.getByText('第 2 集 · 已完成，时间未记录')).toBeVisible();
  await expect(page.getByText('第 4 集 · 已完成，时间未记录')).toHaveCount(0);

  await selector.selectOption('completed');
  await expect(selector).toHaveCount(0);
  snapshot = await mockSnapshot(page);
  expect(snapshot.records[0]).toMatchObject({ nextEpisode: null, status: '已看' });
  await expect(page.getByText(`已完成 · 已记录 ${snapshot.episodeCompletions.length} 集历史`)).toBeVisible();
});

test('@episode-history keeps completed records read-only unless tracked episodes were added', async ({ page }) => {
  await setupMockIpc(page, {
    records: [series({ status: '已看', episodeTrackingEnabled: false, nextEpisode: null })],
  });
  await page.goto('/');

  await expect(page.getByText('已完成 · 无逐集历史')).toBeVisible();
  await expect(page.getByLabel('逐集测试剧 下一集')).toHaveCount(0);
  await expect(page.getByText('启用逐集记录')).toHaveCount(0);
});

test('@episode-history resumes a completed tracked record only when new episodes exist', async ({ page }) => {
  const completions = [completion('2026-08-01T10:00:00.000Z', 1), completion('2026-08-02T10:00:00.000Z', 2)];
  await setupMockIpc(page, {
    records: [series({ totalEpisodes: 4, status: '已看', episodeTrackingEnabled: true, nextEpisode: null, endDate: '2026-08-02' })],
    episodeCompletions: completions,
  });
  await page.goto('/');

  await expect(page.getByText('已完成 · 已记录 2 集历史')).toBeVisible();
  await expect(page.getByText('发现新增 2 集')).toBeVisible();
  await page.getByRole('button', { name: '继续追更（第 3 集）' }).click();

  await expect(page.getByLabel('逐集测试剧 下一集')).toHaveValue('3');
  const snapshot = await mockSnapshot(page);
  expect(snapshot.records[0]).toMatchObject({ status: '在看', nextEpisode: 3, endDate: '2026-08-02' });
  expect(snapshot.episodeCompletions).toEqual(completions);
});

test('@episode-history upgrades WebDAV V3 to V4 only after confirmation', async ({ page }) => {
  const tracked = series({ episodeTrackingEnabled: true, nextEpisode: 2, status: '在看' });
  const localCompletion = completion('2026-08-02T10:00:00.000Z');
  const baseline = payload(tracked, 3);
  await setupMockIpc(page, {
    records: [tracked],
    episodeCompletions: [localCompletion],
    settings: {
      webdav_creds: 'encrypted:user:password',
      webdav_url: 'https://mock.invalid/dav/',
      sync_v3_baseline: JSON.stringify(baseline),
    },
    webdavV3Remote: baseline,
  });
  await page.goto('/');
  page.once('dialog', async dialog => {
    expect(dialog.message()).toContain('升级到 V4');
    await dialog.accept();
  });

  const result = await page.evaluate(async () => (await import('/src/shared/lib/webdav.ts')).syncToWebDAV());
  expect(result.ok).toBe(true);
  const snapshot = await mockSnapshot(page);
  expect(snapshot.webdavV3Remote?.schemaVersion).toBe(4);
  expect(snapshot.webdavV3Remote?.episodeCompletions).toEqual([localCompletion]);
  expect(snapshot.settings.sync_v4_upgrade_confirmed).toBe('1');
});

test('@episode-history asks which completion time to keep during V4 merge', async ({ page }) => {
  const tracked = series({ episodeTrackingEnabled: true, nextEpisode: 2, status: '在看' });
  const baseCompletion = completion('2026-08-01T10:00:00.000Z');
  const localCompletion = { ...completion('2026-08-02T10:00:00.000Z'), rev: 3 };
  const remoteCompletion = { ...completion('2026-08-03T10:00:00.000Z'), rev: 3, revActor: 'remote-device' };
  const baseline = payload(tracked, 4, [baseCompletion]);
  const remote = { ...payload(tracked, 4, [remoteCompletion]), revision: 2, commitId: 'episode-remote' };
  await setupMockIpc(page, {
    records: [tracked],
    episodeCompletions: [localCompletion],
    settings: {
      webdav_creds: 'encrypted:user:password',
      webdav_url: 'https://mock.invalid/dav/',
      sync_v3_baseline: JSON.stringify(baseline),
    },
    webdavV3Remote: remote,
  });
  await page.goto('/');
  page.once('dialog', async dialog => {
    expect(dialog.message()).toContain('确定：采用本机；取消：采用云端');
    await dialog.accept();
  });

  const result = await page.evaluate(async () => (await import('/src/shared/lib/webdav.ts')).syncToWebDAV());
  expect(result.ok).toBe(true);
  const snapshot = await mockSnapshot(page);
  expect(snapshot.webdavV3Remote?.episodeCompletions?.[0].completedAt).toBe(localCompletion.completedAt);
});
