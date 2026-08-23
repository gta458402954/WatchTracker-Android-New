import { expect, test, type Page } from '@playwright/test';
import { createHash } from 'node:crypto';
import type { WatchRecord } from '../src/shared/types';
import type { SyncPayloadV3 } from '../src/shared/lib/syncMerge';
import { mockSnapshot, setupMockIpc } from './fixtures/mockIpc';

function record(id: string, overrides: Partial<WatchRecord> = {}): WatchRecord {
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
    createdAt: '2025-01-01T00:00:00.000Z',
    updatedAt: '2025-01-01T00:00:00.000Z',
    imdbId: null,
    mediaType: '电影',
    contentTags: null,
    originCountry: null,
    ...overrides,
  };
}

function v3Payload(records: WatchRecord[], overrides: Partial<SyncPayloadV3> = {}): SyncPayloadV3 {
  return {
    schemaVersion: 3,
    documentId: 'document-1',
    revision: 1,
    commitId: 'commit-1',
    parentCommitId: null,
    writerId: 'remote-device',
    committedAt: '2026-01-01T00:00:00.000Z',
    records,
    tombstones: [],
    ...overrides,
  };
}

async function openSettingsTools(page: Page) {
  await page.getByRole('button', { name: '设置' }).click();
  await page.getByRole('button', { name: /系统工具/ }).click();
}

test('@expected-settings-modal batch metadata previews and only fills missing movie fields', async ({ page }) => {
  const original = record('批量补全记录', {
    imdbId: 'tt-b003-settings',
    contentTags: '美国,律政,自定义',
  });
  await setupMockIpc(page, {
    records: [original],
    settings: { tmdb_api_key: 'encrypted:test-key' },
    tmdbSearchResults: [{ id: 301, title: '批量补全结果', media_type: 'movie' }],
    tmdbDetail: {
      id: 301,
      title: '批量补全结果',
      production_countries: [
        { iso_3166_1: ' uk ' }, { iso_3166_1: 'GB' }, { iso_3166_1: 'xx' }, { iso_3166_1: 'N/A' },
      ],
      genres: [{ name: 'Drama' }],
      runtime: 120,
    },
  });
  await page.goto('/');
  await openSettingsTools(page);

  const batchButton = page.getByRole('button', { name: /分析并预览缺失字段/ });
  await expect(batchButton).toBeEnabled();
  await batchButton.click();

  const preview = page.getByLabel('元数据补全预览');
  await expect(preview).toBeVisible();
  await expect(preview).toContainText('题材、国家、电影时长');
  expect((await mockSnapshot(page)).calls.filter(call => call.command === 'update_record')).toHaveLength(0);

  await preview.getByRole('button', { name: '确认写入 1 条' }).click();
  await expect(page.getByText('补全结束：已更新 1 条，跳过 0 条，失败 0 条。')).toBeVisible();
  await expect(page.getByLabel('元数据补全结果')).toContainText('已更新');

  const snapshot = await mockSnapshot(page);
  const update = snapshot.calls.find(call => call.command === 'update_record');
  expect(update?.args.id).toBe(original.id);
  expect(update?.args.updates).toMatchObject({
    originCountry: 'GB, XX',
    genres: 'Drama',
    movieDuration: 7200,
  });
  expect(update?.args.updates).not.toHaveProperty('contentTags');
  expect(update?.args.updates).not.toHaveProperty('mediaType');
  expect(update?.args.updates).not.toHaveProperty('episodeRuntime');
  expect(snapshot.records[0].contentTags).toBe('美国,律政,自定义');
  expect(snapshot.calls.filter(call => call.command === 'complete_missing_tmdb_identity')).toHaveLength(1);
  expect(snapshot.records[0]).toMatchObject({
    tmdbMediaKind: 'movie', tmdbId: 301, seriesRecordKind: 'single-work',
  });
});

test('@expected-settings-modal multiple TMDB matches require an explicit user choice', async ({ page }) => {
  const original = record('多候选记录', { imdbId: 'tt-multiple' });
  await setupMockIpc(page, {
    records: [original],
    settings: { tmdb_api_key: 'encrypted:test-key' },
    tmdbSearchResults: [
      { id: 701, title: '候选一', release_date: '2023-01-01', media_type: 'movie' },
      { id: 702, title: '候选二', release_date: '2024-01-01', media_type: 'movie' },
    ],
    tmdbDetail: { id: 702, title: '候选二', runtime: 95 },
  });
  await page.goto('/');
  await openSettingsTools(page);

  await page.getByRole('button', { name: /分析并预览缺失字段/ }).click();
  const preview = page.getByLabel('元数据补全预览');
  await expect(preview).toContainText('待选择');
  await expect(preview.getByRole('button', { name: '确认写入 0 条' })).toBeDisabled();
  expect((await mockSnapshot(page)).calls.filter(call => call.command === 'get_tmdb_detail')).toHaveLength(0);
  expect((await mockSnapshot(page)).calls.filter(call => call.command === 'update_record')).toHaveLength(0);

  await preview.getByRole('button', { name: /候选二.*2024.*电影.*#702/ }).click();
  await expect(preview).toContainText('可更新');
  await expect(preview).toContainText('movie:702:series');
  await preview.getByRole('button', { name: '确认写入 1 条' }).click();
  const snapshot = await mockSnapshot(page);
  expect(snapshot.calls.filter(call => call.command === 'get_tmdb_detail')).toHaveLength(1);
  expect(snapshot.calls.filter(call => call.command === 'update_record')).toHaveLength(1);
});

test('@expected-settings-modal remembered TMDB no-data fields are not queried again', async ({ page }) => {
  await setupMockIpc(page, {
    records: [record('无数据记忆', { imdbId: 'tt-no-data-memory' })],
    settings: { tmdb_api_key: 'encrypted:test-key' },
    tmdbSearchResults: [{ id: 801, title: '无数据记忆', media_type: 'movie' }],
    tmdbDetail: { id: 801, title: '无数据记忆', runtime: 100 },
  });
  await page.goto('/');
  await openSettingsTools(page);

  await page.getByRole('button', { name: /分析并预览缺失字段/ }).click();
  const preview = page.getByLabel('元数据补全预览');
  await expect(preview).toContainText('TMDB 无数据');
  await preview.getByRole('button', { name: '确认写入 1 条' }).click();
  await expect(page.getByText(/补全结束：已更新 1 条/)).toBeVisible();
  const firstSearchCount = (await mockSnapshot(page)).calls.filter(call => call.command === 'search_tmdb').length;

  await page.getByRole('button', { name: /分析并预览缺失字段/ }).click();
  await expect(page.getByText(/缺失字段已确认 TMDB 无数据/)).toBeVisible();
  const snapshot = await mockSnapshot(page);
  expect(snapshot.calls.filter(call => call.command === 'search_tmdb')).toHaveLength(firstSearchCount);
  expect(JSON.parse(snapshot.settings.batch_metadata_no_data_v1!)).toMatchObject({
    version: 1,
    records: { '无数据记忆': { imdbId: 'tt-no-data-memory' } },
  });
});

test('@expected-settings-modal shared IMDb seasons keep distinct episode totals', async ({ page }) => {
  const first = record('第一季', {
    originalName: 'Example Season 1', imdbId: 'tt-shared-series', mediaType: '剧集', contentTags: '自定义',
  });
  const second = record('第二季', {
    originalName: 'Example Season 2', imdbId: 'tt-shared-series', mediaType: '剧集', contentTags: '自定义',
  });
  await setupMockIpc(page, {
    records: [first, second],
    settings: { tmdb_api_key: 'encrypted:test-key' },
    tmdbSearchResults: [{ id: 510, name: 'Example', media_type: 'tv' }],
    tmdbDetail: {
      id: 510, name: 'Example', origin_country: ['US'], genres: [{ name: 'Drama' }],
      episode_run_time: [48], number_of_episodes: 23,
      seasons: [{ id: 51001, season_number: 1, episode_count: 10 }, { id: 51002, season_number: 2, episode_count: 13 }],
    },
  });
  await page.goto('/');
  await openSettingsTools(page);

  await page.getByRole('button', { name: /分析并预览缺失字段/ }).click();
  const preview = page.getByLabel('元数据补全预览');
  await expect(preview).toContainText('tv:510:season-1');
  await expect(preview).toContainText('tv:510:season-2');
  await preview.getByRole('button', { name: '确认写入 2 条' }).click();
  await expect(page.getByText('补全结束：已更新 2 条，跳过 0 条，失败 0 条。')).toBeVisible();

  const updates = (await mockSnapshot(page)).calls
    .filter(call => call.command === 'update_record')
    .map(call => ({ id: call.args.id, updates: call.args.updates }));
  expect(updates).toEqual(expect.arrayContaining([
    expect.objectContaining({ id: first.id, updates: expect.objectContaining({ totalEpisodes: 10, episodeRuntime: 48 }) }),
    expect.objectContaining({ id: second.id, updates: expect.objectContaining({ totalEpisodes: 13, episodeRuntime: 48 }) }),
  ]));
  const batchCalls = (await mockSnapshot(page)).calls;
  const recoveryIndex = batchCalls.findIndex(call => call.command === 'create_recovery_point' && call.args.reason === 'batch-metadata');
  const firstWriteIndex = batchCalls.findIndex(call => call.command === 'update_record');
  expect(recoveryIndex).toBeGreaterThanOrEqual(0);
  expect(recoveryIndex).toBeLessThan(firstWriteIndex);
  const snapshot = await mockSnapshot(page);
  expect(snapshot.calls.filter(call => call.command === 'complete_missing_tmdb_identity')).toHaveLength(2);
  expect(snapshot.records).toEqual(expect.arrayContaining([
    expect.objectContaining({ id: first.id, tmdbMediaKind: 'tv-season', tmdbId: 51001, tmdbParentId: 510, tmdbSeasonNumber: 1, seriesRecordKind: 'season' }),
    expect.objectContaining({ id: second.id, tmdbMediaKind: 'tv-season', tmdbId: 51002, tmdbParentId: 510, tmdbSeasonNumber: 2, seriesRecordKind: 'season' }),
  ]));
});

test('@expected-settings-modal partial write failure is visible and retryable', async ({ page }) => {
  const first = record('成功记录', { imdbId: 'tt-success' });
  const second = record('重试记录', { imdbId: 'tt-retry' });
  await setupMockIpc(page, {
    records: [first, second],
    settings: { tmdb_api_key: 'encrypted:test-key' },
    tmdbSearchResults: [{ id: 301, title: 'Movie', media_type: 'movie' }],
    tmdbDetail: { id: 301, runtime: 100, genres: [{ name: 'Drama' }], production_countries: [{ iso_3166_1: 'US' }] },
    updateFailureCounts: { [second.id]: 1 },
  });
  await page.goto('/');
  await openSettingsTools(page);

  await page.getByRole('button', { name: /分析并预览缺失字段/ }).click();
  await page.getByRole('button', { name: '确认写入 2 条' }).click();
  await expect(page.getByText('补全结束：已更新 1 条，跳过 0 条，失败 1 条。')).toBeVisible();

  await page.getByRole('button', { name: '重试失败项' }).click();
  await expect(page.getByText('补全结束：已更新 2 条，跳过 0 条，失败 0 条。')).toBeVisible();
  const retryWrites = (await mockSnapshot(page)).calls.filter(call => call.command === 'update_record' && call.args.id === second.id);
  expect(retryWrites).toHaveLength(2);
});

test('@expected-settings-modal planning can be cancelled without database writes', async ({ page }) => {
  await setupMockIpc(page, {
    records: [record('慢速记录一', { imdbId: 'tt-slow-1' }), record('慢速记录二', { imdbId: 'tt-slow-2' })],
    settings: { tmdb_api_key: 'encrypted:test-key' },
    tmdbSearchResults: [{ id: 301, title: 'Movie', media_type: 'movie' }],
    tmdbDetail: { id: 301, runtime: 100 },
    tmdbDelayMs: 400,
  });
  await page.goto('/');
  await openSettingsTools(page);

  await page.getByRole('button', { name: /分析并预览缺失字段/ }).click();
  await page.getByRole('button', { name: '安全停止', exact: true }).click();
  await expect(page.getByText(/分析已取消/)).toBeVisible();
  expect((await mockSnapshot(page)).calls.filter(call => call.command === 'update_record')).toHaveLength(0);
});

test('@expected-settings-modal successful write uses the normal action and schedules auto sync', async ({ page }) => {
  await setupMockIpc(page, {
    records: [record('自动同步记录', { imdbId: 'tt-auto-sync' })],
    settings: {
      tmdb_api_key: 'encrypted:test-key',
      sync_interval: '5',
      webdav_creds: 'encrypted:user:password',
      webdav_url: 'https://dav.example.test/watchtracker/',
    },
    tmdbSearchResults: [{ id: 301, title: 'Movie', media_type: 'movie' }],
    tmdbDetail: { id: 301, runtime: 100, genres: [{ name: 'Drama' }] },
    webdavRemote: [],
  });
  await page.goto('/');
  await openSettingsTools(page);

  await page.getByRole('button', { name: /分析并预览缺失字段/ }).click();
  await page.getByRole('button', { name: '确认写入 1 条' }).click();
  await expect(page.getByText('补全结束：已更新 1 条，跳过 0 条，失败 0 条。')).toBeVisible();

  await expect.poll(async () => (await mockSnapshot(page)).calls.some(call => call.command === 'webdav_request'), {
    timeout: 8000,
  }).toBe(true);
});

test('@conditional-record-form new movie preserves all normalized countries and custom tags', async ({ page }) => {
  await setupMockIpc(page, {
    settings: { tmdb_api_key: 'encrypted:test-key' },
    tmdbSearchResults: [{ id: 401, title: '电影候选', media_type: 'movie' }],
    tmdbDetail: {
      id: 401,
      title: '电影候选',
      original_title: 'Movie Candidate',
      production_countries: [
        { iso_3166_1: 'us' }, { iso_3166_1: ' UK ' }, { iso_3166_1: 'xx' },
      ],
      genres: [{ name: 'Drama' }],
      runtime: 110,
    },
  });
  await page.goto('/');
  await page.getByRole('button', { name: '更多操作' }).click();
  await page.getByRole('menuitem', { name: /添加记录/ }).click();
  await page.getByPlaceholder('请输入中文名称').fill('电影候选');
  await page.getByPlaceholder('如：韩国').fill('美国,律政,自定义');
  await page.getByRole('button', { name: /自动填充/ }).click();
  await page.getByRole('button', { name: /电影候选/ }).click();
  await page.getByRole('button', { name: '添加记录' }).click();

  const snapshot = await mockSnapshot(page);
  const inserted = snapshot.calls.find(call => call.command === 'insert_record')?.args.r as WatchRecord;
  expect(inserted.originCountry).toBe('US, GB, XX');
  expect(inserted.contentTags).toBe('律政,自定义,美国,英国');
  expect(inserted.movieDuration).toBe(6600);
  expect(inserted.episodeRuntime).toBeNull();
});

test('@expected-record-form saving a mainland-China record never clears a user platform', async ({ page }) => {
  await setupMockIpc(page, {
    records: [record('平台保护', {
      mediaType: '剧集', totalEpisodes: 12, originCountry: 'CN', platform: '用户手工平台',
    })],
  });
  await page.goto('/');
  await page.getByTitle('编辑').click();
  await page.getByPlaceholder('随便写点什么...').fill('只修改备注');
  await page.getByRole('button', { name: '保存修改' }).click();

  const update = (await mockSnapshot(page)).calls.find(call => call.command === 'update_record');
  expect(update?.args.updates).toMatchObject({ notes: '只修改备注', platform: '用户手工平台' });
});

test('@expected-record-form mainland-China TMDB metadata does not infer a missing platform', async ({ page }) => {
  await setupMockIpc(page, {
    settings: { tmdb_api_key: 'encrypted:test-key' },
    tmdbSearchResults: [{ id: 451, name: '大陆剧集候选', media_type: 'tv' }],
    tmdbDetail: {
      id: 451,
      name: '大陆剧集候选',
      original_name: 'Mainland Series',
      origin_country: ['CN'],
      networks: [{ name: 'Tencent Video' }],
      number_of_episodes: 20,
    },
  });
  await page.goto('/');
  await page.getByRole('button', { name: '更多操作' }).click();
  await page.getByRole('menuitem', { name: /添加记录/ }).click();
  await page.locator('select:has(option[value="剧集"])').selectOption('剧集');
  await page.getByPlaceholder('请输入中文名称').fill('大陆剧集候选');
  await page.getByRole('button', { name: /自动填充/ }).click();
  await page.getByRole('button', { name: /大陆剧集候选/ }).click();
  await expect(page.getByPlaceholder('Netflix / 爱奇艺 / B站...')).toHaveValue('');
  await page.getByRole('button', { name: '添加记录' }).click();

  const inserted = (await mockSnapshot(page)).calls.find(call => call.command === 'insert_record')?.args.r as WatchRecord;
  expect(inserted.originCountry).toBe('CN');
  expect(inserted.platform).toBe('');
  expect(inserted.episodeRuntime).toBeNull();
});

test('@conditional-record-form edited TV season preserves countries and custom tags', async ({ page }) => {
  await setupMockIpc(page, {
    records: [record('旧剧集', {
      imdbId: 'tt-b003-series',
      mediaType: '剧集',
      totalEpisodes: 8,
      contentTags: '美国,悬疑,自定义',
    })],
    settings: { tmdb_api_key: 'encrypted:test-key' },
    tmdbSearchResults: [{ id: 501, name: '剧集候选', media_type: 'tv' }],
    tmdbDetail: {
      id: 501,
      name: '剧集候选',
      original_name: 'Series Candidate',
      origin_country: [' hk ', 'TW', 'uk', 'HK'],
      networks: [{ name: 'Apple Tv' }],
      genres: [{ name: 'Drama' }],
      seasons: [{ id: 510, name: '第一季', season_number: 1, episode_count: 10 }],
    },
  });
  await page.goto('/');
  await page.getByTitle('编辑').click();
  await page.getByRole('button', { name: /自动填充/ }).click();
  await page.getByRole('button', { name: /剧集候选/ }).click();
  await page.getByRole('button', { name: /第一季/ }).click();
  await page.getByRole('button', { name: '保存修改' }).click();

  const snapshot = await mockSnapshot(page);
  const update = snapshot.calls.find(call => call.command === 'update_record');
  expect(update?.args.updates).toMatchObject({
    originCountry: 'HK, TW, GB',
    contentTags: '悬疑,自定义,中国香港,中国台湾,英国',
    platform: 'Apple TV+',
    episodeRuntime: null,
  });
});

test('@conditional-webdav-payload schema v2 merge and PUT preserve region fields', async ({ page }) => {
  const local = record('shared', {
    originCountry: 'GB, XX',
    contentTags: '律政,自定义',
    updatedAt: '2026-01-02T00:00:00.000Z',
  });
  const remote = record('remote-only', {
    originCountry: 'CN, HK, TW',
    contentTags: '历史,自定义',
    updatedAt: '2026-01-01T00:00:00.000Z',
  });
  await setupMockIpc(page, {
    records: [local],
    settings: {
      webdav_creds: 'encrypted:fixture-user:fixture-password',
      webdav_url: 'https://mock.invalid/dav/',
    },
    webdavRemote: { schemaVersion: 2, updatedAt: '', records: [remote], tombstones: [] },
  });
  await page.goto('/');

  const result = await page.evaluate(async () => {
    const { syncToWebDAV } = await import('/src/shared/lib/webdav.ts');
    return syncToWebDAV();
  });
  expect(result.ok).toBe(true);

  const snapshot = await mockSnapshot(page);
  const put = snapshot.calls.find(call => call.command === 'webdav_request' && call.args.method === 'PUT');
  expect(put?.args.url).toContain('records-v3.json');
  expect(put?.args.ifNoneMatch).toBe('*');
  const payload = JSON.parse(String(put?.args.body));
  expect(payload.records).toEqual(expect.arrayContaining([
    expect.objectContaining({ id: 'shared', originCountry: 'GB, XX', contentTags: '律政,自定义' }),
    expect.objectContaining({ id: 'remote-only', originCountry: 'CN, HK, TW', contentTags: '历史,自定义' }),
  ]));
});

test('@conditional-webdav-payload legacy array GET preserves region fields', async ({ page }) => {
  const remote = record('legacy-remote', {
    originCountry: 'US, GB',
    contentTags: '旧标签,自定义',
  });
  await setupMockIpc(page, {
    settings: {
      webdav_creds: 'encrypted:fixture-user:fixture-password',
      webdav_url: 'https://mock.invalid/dav/',
    },
    webdavRemote: [remote],
  });
  await page.goto('/');
  const result = await page.evaluate(async () => {
    const { loadFromWebDAV } = await import('/src/shared/lib/webdav.ts');
    return loadFromWebDAV();
  });
  expect(result.data?.[0]).toMatchObject({ originCountry: 'US, GB', contentTags: '旧标签,自定义' });
});

test('@expected-sync-v3 retries a stale ETag and never falls back to unconditional PUT', async ({ page }) => {
  const base = record('etag-record', { notes: 'base' });
  const local = record('etag-record', { notes: 'local change' });
  await setupMockIpc(page, {
    records: [local],
    settings: {
      webdav_creds: 'encrypted:fixture-user:fixture-password',
      webdav_url: 'https://mock.invalid/dav/',
      sync_v3_baseline: JSON.stringify(v3Payload([base])),
    },
    webdavV3Remote: v3Payload([base]),
    webdavPreconditionFailures: 1,
  });
  await page.goto('/');
  const result = await page.evaluate(async () => (await import('/src/shared/lib/webdav.ts')).syncToWebDAV());
  expect(result.ok).toBe(true);
  const snapshot = await mockSnapshot(page);
  const puts = snapshot.calls.filter(call => call.command === 'webdav_request' && call.args.method === 'PUT');
  expect(puts).toHaveLength(2);
  expect(puts.every(call => call.args.ifMatch === '"v3-1"' && call.args.ifNoneMatch === null)).toBe(true);
  expect(snapshot.webdavV3Remote?.records[0].notes).toBe('local change');
});

test('@expected-sync-v3 blocks upload when the server has no usable ETag', async ({ page }) => {
  const base = record('unsafe-server');
  await setupMockIpc(page, {
    records: [record('unsafe-server', { notes: 'local change' })],
    settings: {
      webdav_creds: 'encrypted:fixture-user:fixture-password',
      webdav_url: 'https://mock.invalid/dav/',
      sync_v3_baseline: JSON.stringify(v3Payload([base])),
    },
    webdavV3Remote: v3Payload([base]),
    webdavV3Etag: null,
  });
  await page.goto('/');
  const result = await page.evaluate(async () => (await import('/src/shared/lib/webdav.ts')).syncToWebDAV());
  expect(result).toMatchObject({ ok: false, error: 'conditional_write_unsupported' });
  const snapshot = await mockSnapshot(page);
  expect(snapshot.calls.some(call => call.command === 'webdav_request' && call.args.method === 'PUT')).toBe(false);
  expect(snapshot.calls.some(call => call.command === 'commit_sync_result')).toBe(false);
  expect(snapshot.records[0].notes).toBe('local change');
});

test('@expected-sync-v3 uses the WebDAV If header for a weak ETag', async ({ page }) => {
  const base = record('weak-etag-server');
  await setupMockIpc(page, {
    records: [record('weak-etag-server', { notes: 'safe local change' })],
    settings: {
      webdav_creds: 'encrypted:fixture-user:fixture-password',
      webdav_url: 'https://mock.invalid/dav/',
      sync_v3_baseline: JSON.stringify(v3Payload([base])),
    },
    webdavV3Remote: v3Payload([base]),
    webdavV3Etag: 'W/"jianguoyun-etag"',
    webdavPreconditionFailures: 1,
  });
  await page.goto('/');

  const result = await page.evaluate(async () => (await import('/src/shared/lib/webdav.ts')).syncToWebDAV());

  expect(result.ok).toBe(true);
  const snapshot = await mockSnapshot(page);
  const puts = snapshot.calls.filter(call => call.command === 'webdav_request' && call.args.method === 'PUT');
  expect(puts).toHaveLength(2);
  expect(puts.every(call => call.args.ifMatch === null && call.args.ifDavEtag === 'W/"jianguoyun-etag"')).toBe(true);
  expect(snapshot.webdavV3Remote?.records[0].notes).toBe('safe local change');
});

test('@expected-sync-v3 routes an unquoted server ETag through PROPFIND and WebDAV If', async ({ page }) => {
  const base = record('unquoted-etag-server');
  await setupMockIpc(page, {
    records: [record('unquoted-etag-server', { notes: 'safe local change' })],
    settings: {
      webdav_creds: 'encrypted:fixture-user:fixture-password',
      webdav_url: 'https://mock.invalid/dav/',
      sync_v3_baseline: JSON.stringify(v3Payload([base])),
    },
    webdavV3Remote: v3Payload([base]),
    webdavV3Etag: 'jianguoyun-unquoted-etag',
    webdavPreconditionFailures: 1,
  });
  await page.goto('/');

  const result = await page.evaluate(async () => (await import('/src/shared/lib/webdav.ts')).syncToWebDAV());

  expect(result.ok).toBe(true);
  const snapshot = await mockSnapshot(page);
  const puts = snapshot.calls.filter(call => call.command === 'webdav_request' && call.args.method === 'PUT');
  expect(puts).toHaveLength(2);
  expect(snapshot.calls.filter(call => call.command === 'webdav_request' && call.args.method === 'PROPFIND')).toHaveLength(2);
  expect(puts.every(call => call.args.ifMatch === null && call.args.ifDavEtag === '"jianguoyun-unquoted-etag"')).toBe(true);
  expect(snapshot.webdavV3Remote?.records[0].notes).toBe('safe local change');
});

test('@expected-sync-v3 recovers a verified publish intent before considering another upload', async ({ page }) => {
  const remote = v3Payload([record('intent-recovery')], {
    commitId: 'already-published',
    writerId: 'previous-device',
  });
  await setupMockIpc(page, {
    records: structuredClone(remote.records),
    webdavV3Remote: remote,
    settings: {
      webdav_creds: 'encrypted:fixture-user:fixture-password',
      webdav_url: 'https://mock.invalid/dav/',
      sync_publish_intent_v1: JSON.stringify({
        version: 1, commitId: 'already-published', previousCommitId: 'previous',
        expectedGeneration: 0, includedEntries: [],
        payloadFingerprint: createHash('sha256').update(JSON.stringify(remote)).digest('hex'),
        createdAt: '2026-08-02T00:00:00.000Z',
      }),
    },
  });
  await page.goto('/');

  const result = await page.evaluate(async () => (await import('/src/shared/lib/webdav.ts')).syncToWebDAV());

  expect(result.ok).toBe(true);
  const snapshot = await mockSnapshot(page);
  expect(snapshot.calls.some(call => call.command === 'webdav_request' && call.args.method === 'PUT')).toBe(false);
  expect(snapshot.settings.sync_publish_intent_v1).toBeUndefined();
  expect(JSON.parse(snapshot.settings.sync_v3_baseline as string).commitId).toBe('already-published');
});

test('@expected-sync-v3 converts a same-device orphan conflict back into a staged upload', async ({ page }) => {
  const old = record('orphan-record', { notes: 'remote old', rev: 1, revActor: 'legacy' });
  const local = record('orphan-record', { notes: 'local staged', rev: 2, revActor: 'mock-device' });
  const remote = v3Payload([old], { writerId: 'mock-device' });
  await setupMockIpc(page, {
    records: [local],
    webdavV3Remote: remote,
    settings: {
      webdav_creds: 'encrypted:fixture-user:fixture-password',
      webdav_url: 'https://mock.invalid/dav/',
      sync_v3_baseline: JSON.stringify(remote),
      sync_v3_conflicts: JSON.stringify([{
        id: 'orphan-record', kind: 'edit-edit', fields: ['record'], base: null,
        local, remote: old, localDeleted: false, remoteDeleted: false,
        detectedAt: '2026-08-02T00:00:00.000Z',
      }]),
    },
  });
  await page.goto('/');

  const result = await page.evaluate(async () => (await import('/src/shared/lib/webdav.ts')).syncToWebDAV());

  expect(result).toMatchObject({ ok: true, conflictCount: 0 });
  const snapshot = await mockSnapshot(page);
  expect(snapshot.webdavV3Remote?.records[0].notes).toBe('local staged');
  expect(JSON.parse(snapshot.settings.sync_v3_conflicts as string)).toEqual([]);
  const prepareIndex = snapshot.calls.findIndex(call => call.command === 'prepare_sync_publish_intent');
  const putIndex = snapshot.calls.findIndex(call => call.command === 'webdav_request' && call.args.method === 'PUT');
  expect(prepareIndex).toBeGreaterThanOrEqual(0);
  expect(putIndex).toBeGreaterThan(prepareIndex);
});

test('@expected-sync-v3 uses DAV getetag when GET and PUT omit ETag headers', async ({ page }) => {
  const base = record('propfind-server');
  const local = record('propfind-server', { notes: 'safe local change' });
  await setupMockIpc(page, {
    records: [local],
    settings: {
      webdav_creds: 'encrypted:fixture-user:fixture-password',
      webdav_url: 'https://mock.invalid/dav/',
      sync_v3_baseline: JSON.stringify(v3Payload([base])),
    },
    webdavV3Remote: v3Payload([base]),
    webdavV3Etag: '"v3-1"',
    omitGetEtag: true,
    omitPutEtag: true,
  });
  await page.goto('/');
  const result = await page.evaluate(async () => (await import('/src/shared/lib/webdav.ts')).syncToWebDAV());
  expect(result.ok).toBe(true);
  const snapshot = await mockSnapshot(page);
  const propfinds = snapshot.calls.filter(call => call.command === 'webdav_request' && call.args.method === 'PROPFIND');
  expect(propfinds).toHaveLength(2);
  const put = snapshot.calls.find(call => call.command === 'webdav_request' && call.args.method === 'PUT');
  expect(put?.args.ifDavEtag).toBe('"v3-1"');
  expect(snapshot.webdavV3Remote?.records[0].notes).toBe('safe local change');
  expect(snapshot.settings.sync_v3_remote_etag).toBe('"v3-2"');
});

test('@expected-sync-v3 keeps same-field divergence pending without overwriting either side', async ({ page }) => {
  const base = record('conflicted', { notes: 'base' });
  const local = record('conflicted', { notes: 'local' });
  const remote = record('conflicted', { notes: 'remote' });
  await setupMockIpc(page, {
    records: [local],
    settings: {
      webdav_creds: 'encrypted:fixture-user:fixture-password',
      webdav_url: 'https://mock.invalid/dav/',
      sync_v3_baseline: JSON.stringify(v3Payload([base])),
    },
    webdavV3Remote: v3Payload([remote], { revision: 2, commitId: 'remote-change' }),
  });
  await page.goto('/');
  const result = await page.evaluate(async () => (await import('/src/shared/lib/webdav.ts')).syncToWebDAV());
  expect(result).toMatchObject({ ok: true, conflictCount: 1 });
  const snapshot = await mockSnapshot(page);
  expect(snapshot.records[0].notes).toBe('local');
  expect(snapshot.webdavV3Remote?.records[0].notes).toBe('remote');
  expect(JSON.parse(snapshot.settings.sync_v3_conflicts || '[]')[0].fields).toEqual(['notes']);
});

test('@expected-sync-v3 rejects an unknown future payload without PUT or local commit', async ({ page }) => {
  await setupMockIpc(page, {
    records: [record('protected')],
    settings: {
      webdav_creds: 'encrypted:fixture-user:fixture-password',
      webdav_url: 'https://mock.invalid/dav/',
    },
    webdavV3Remote: { schemaVersion: 7, records: [], tombstones: [] } as unknown as SyncPayloadV3,
  });
  await page.goto('/');
  const result = await page.evaluate(async () => (await import('/src/shared/lib/webdav.ts')).syncToWebDAV());
  expect(result).toMatchObject({ ok: false, error: 'unsupported_remote_schema' });
  const snapshot = await mockSnapshot(page);
  expect(snapshot.calls.some(call => call.command === 'webdav_request' && call.args.method === 'PUT')).toBe(false);
  expect(snapshot.calls.some(call => call.command === 'commit_sync_result')).toBe(false);
  expect(snapshot.records[0].id).toBe('protected');
});

test('@expected-sync-v3 CAS preserves edits made while the network request is in flight', async ({ page }) => {
  const base = record('local-cas', { notes: 'base' });
  await setupMockIpc(page, {
    records: [record('local-cas', { notes: 'first local edit' })],
    settings: {
      webdav_creds: 'encrypted:fixture-user:fixture-password',
      webdav_url: 'https://mock.invalid/dav/',
      sync_v3_baseline: JSON.stringify(v3Payload([base])),
    },
    webdavV3Remote: v3Payload([base]),
    mutateLocalDuringPut: true,
  });
  await page.goto('/');
  const result = await page.evaluate(async () => (await import('/src/shared/lib/webdav.ts')).syncToWebDAV());
  expect(result).toMatchObject({ ok: false, error: 'stale_local_snapshot', staleLocal: true });
  const snapshot = await mockSnapshot(page);
  expect(snapshot.records[0].notes).toBe('edited during sync');
  expect(snapshot.webdavV3Remote?.records[0].notes).toBe('first local edit');
});

test('@expected-sync-v3 stops after three consecutive precondition failures', async ({ page }) => {
  const base = record('busy', { notes: 'base' });
  await setupMockIpc(page, {
    records: [record('busy', { notes: 'local' })],
    settings: {
      webdav_creds: 'encrypted:fixture-user:fixture-password',
      webdav_url: 'https://mock.invalid/dav/',
      sync_v3_baseline: JSON.stringify(v3Payload([base])),
    },
    webdavV3Remote: v3Payload([base]),
    webdavPreconditionFailures: 3,
    rotateEtagOnPreconditionFailure: true,
  });
  await page.goto('/');
  const result = await page.evaluate(async () => (await import('/src/shared/lib/webdav.ts')).syncToWebDAV());
  expect(result).toMatchObject({ ok: false, error: 'remote_busy' });
  const snapshot = await mockSnapshot(page);
  expect(snapshot.calls.filter(call => call.command === 'webdav_request' && call.args.method === 'PUT')).toHaveLength(3);
  expect(snapshot.calls.some(call => call.command === 'commit_sync_result')).toBe(false);
});

test('@expected-sync-v3 stops retrying when the same conditional validator is repeatedly rejected', async ({ page }) => {
  const base = record('rejected-validator', { notes: 'base' });
  await setupMockIpc(page, {
    records: [record('rejected-validator', { notes: 'local' })],
    settings: {
      webdav_creds: 'encrypted:fixture-user:fixture-password',
      webdav_url: 'https://mock.invalid/dav/',
      sync_v3_baseline: JSON.stringify(v3Payload([base])),
    },
    webdavV3Remote: v3Payload([base]),
    webdavV3Etag: 'jianguoyun-unquoted-etag',
    webdavPreconditionFailures: 3,
  });
  await page.goto('/');

  const result = await page.evaluate(async () => (await import('/src/shared/lib/webdav.ts')).syncToWebDAV());

  expect(result).toMatchObject({ ok: false, error: 'conditional_validator_rejected' });
  const snapshot = await mockSnapshot(page);
  expect(snapshot.calls.filter(call => call.command === 'webdav_request' && call.args.method === 'PUT')).toHaveLength(3);
  expect(snapshot.calls.filter(call => call.command === 'webdav_request' && call.args.method === 'PROPFIND')).toHaveLength(3);
  expect(snapshot.calls.some(call => call.command === 'commit_sync_result')).toBe(false);
});

test('@expected-sync-v3 verifies commitId when PUT omits the new ETag', async ({ page }) => {
  const base = record('verify-put', { notes: 'base' });
  await setupMockIpc(page, {
    records: [record('verify-put', { notes: 'local' })],
    settings: {
      webdav_creds: 'encrypted:fixture-user:fixture-password',
      webdav_url: 'https://mock.invalid/dav/',
      sync_v3_baseline: JSON.stringify(v3Payload([base])),
    },
    webdavV3Remote: v3Payload([base]),
    omitPutEtag: true,
  });
  await page.goto('/');
  const result = await page.evaluate(async () => (await import('/src/shared/lib/webdav.ts')).syncToWebDAV());
  expect(result.ok).toBe(true);
  const snapshot = await mockSnapshot(page);
  const v3Gets = snapshot.calls.filter(call => call.command === 'webdav_request'
    && call.args.method === 'GET' && String(call.args.url).endsWith('records-v3.json'));
  expect(v3Gets.length).toBeGreaterThanOrEqual(2);
  expect(snapshot.calls.some(call => call.command === 'commit_sync_result')).toBe(true);
});

test('@expected-sync-v3 detects continued legacy-client writes without mixing them into v3', async ({ page }) => {
  const current = record('v3-current');
  await setupMockIpc(page, {
    records: [current],
    settings: {
      webdav_creds: 'encrypted:fixture-user:fixture-password',
      webdav_url: 'https://mock.invalid/dav/',
      sync_v3_baseline: JSON.stringify(v3Payload([current])),
      sync_v2_source_fingerprint: '"legacy-older"',
    },
    webdavRemote: [record('legacy-new-write')],
    webdavV3Remote: v3Payload([current]),
  });
  await page.goto('/');
  const result = await page.evaluate(async () => (await import('/src/shared/lib/webdav.ts')).syncToWebDAV());
  expect(result).toMatchObject({ ok: false, error: 'legacy_remote_changed' });
  const snapshot = await mockSnapshot(page);
  expect(snapshot.calls.some(call => call.command === 'webdav_request' && call.args.method === 'PUT')).toBe(false);
  expect(snapshot.records.map(item => item.id)).toEqual(['v3-current']);
});

test('@expected-sync-v3 explicitly imports changed legacy data into the conflict center only', async ({ page }) => {
  const current = record('legacy-choice', { chineseName: '当前 v3 版本', originCountry: 'CN' });
  const legacy = record('legacy-choice', { chineseName: '旧版设备修改', originCountry: 'GB' });
  await setupMockIpc(page, {
    records: [current],
    settings: {
      webdav_creds: 'encrypted:fixture-user:fixture-password',
      webdav_url: 'https://mock.invalid/dav/',
      sync_v3_baseline: JSON.stringify(v3Payload([current])),
      sync_v2_source_fingerprint: '"legacy-older"',
    },
    webdavRemote: [legacy],
    webdavV3Remote: v3Payload([current]),
  });
  await page.goto('/');
  await page.getByRole('button', { name: '设置' }).click();
  await page.getByRole('button', { name: '☁️ 云端同步', exact: true }).click();
  page.once('dialog', dialog => dialog.accept());
  await page.getByRole('button', { name: /检查并导入旧版/ }).click();
  await expect(page.getByText(/已加入 1 项旧版差异/)).toBeVisible();
  await expect(page.getByText(/云端：旧版设备修改/)).toBeVisible();
  const snapshot = await mockSnapshot(page);
  expect(snapshot.records[0].chineseName).toBe('当前 v3 版本');
  expect(snapshot.webdavV3Remote?.records[0].chineseName).toBe('当前 v3 版本');
  expect(snapshot.calls.some(call => call.command === 'webdav_request' && call.args.method === 'PUT')).toBe(false);
  expect(JSON.parse(snapshot.settings.sync_v3_conflicts || '[]')).toHaveLength(1);
});

test('@conditional-watchlist-boundary local export and import preserve region fields', async ({ page }) => {
  const original = record('本地往返', {
    originCountry: 'GB, XX, CN',
    contentTags: '律政,自定义,日本料理',
  });
  await setupMockIpc(page, { records: [original] });
  await page.goto('/');
  await openSettingsTools(page);

  const downloadPromise = page.waitForEvent('download');
  await page.getByRole('button', { name: /导出备份 JSON/ }).click();
  const download = await downloadPromise;
  const path = await download.path();
  expect(path).not.toBeNull();

  const chooserPromise = page.waitForEvent('filechooser');
  await page.getByRole('button', { name: /导入本地 JSON/ }).click();
  const chooser = await chooserPromise;
  await chooser.setFiles(path!);
  await expect(page.getByText('已导入 1 条记录、0 条逐集历史和 0 个收藏集。')).toBeVisible();

  const snapshot = await mockSnapshot(page);
  const replacement = snapshot.calls.find(call => call.command === 'replace_library_v3');
  expect(replacement?.args.episodeCompletions).toEqual([]);
  expect(replacement?.args.records).toEqual([
    expect.objectContaining({ originCountry: 'GB, XX, CN', contentTags: '律政,自定义,日本料理' }),
  ]);
});

test('@conditional-watchlist-boundary sync replacement preserves region fields', async ({ page }) => {
  const base = record('shared', { chineseName: '共同旧版本' });
  const local = structuredClone(base);
  const remote = record('shared', {
    chineseName: '云端新版本',
    originCountry: 'HK, TW, GB',
    contentTags: '悬疑,自定义',
    updatedAt: '2026-02-01T00:00:00.000Z',
  });
  await setupMockIpc(page, {
    records: [local],
    settings: {
      webdav_creds: 'encrypted:fixture-user:fixture-password',
      webdav_url: 'https://mock.invalid/dav/',
      sync_v3_baseline: JSON.stringify(v3Payload([base])),
    },
    webdavV3Remote: v3Payload([remote], { revision: 2, commitId: 'commit-2' }),
  });
  await page.goto('/');
  await page.getByRole('button', { name: /云端同步：/ }).click();
  await page.getByRole('button', { name: '立即同步' }).click();
  await expect(page.getByText('云端新版本', { exact: true })).toBeVisible();

  const snapshot = await mockSnapshot(page);
  expect(snapshot.records[0]).toMatchObject({
    originCountry: 'HK, TW, GB',
    contentTags: '悬疑,自定义',
  });
  expect(snapshot.calls.some(call => call.command === 'commit_sync_result')).toBe(true);
  const put = snapshot.calls.find(call => call.command === 'webdav_request' && call.args.method === 'PUT');
  expect(put).toBeUndefined();
});

test('@expected-settings-modal automatic recovery point can restore a pre-import database', async ({ page }) => {
  const original = record('恢复前记录', { notes: '原始状态' });
  const replacement = record('导入后记录', { notes: '替换状态' });
  await setupMockIpc(page, { records: [original] });
  await page.goto('/');
  await openSettingsTools(page);

  const chooserPromise = page.waitForEvent('filechooser');
  await page.getByRole('button', { name: /导入本地 JSON/ }).click();
  const chooser = await chooserPromise;
  await chooser.setFiles({
    name: 'replacement.json',
    mimeType: 'application/json',
    buffer: Buffer.from(JSON.stringify([replacement])),
  });
  await expect(page.getByText('已导入 1 条本地记录。')).toBeVisible();

  const recoveryPanel = page.getByLabel('自动恢复点');
  await recoveryPanel.getByRole('button', { name: '刷新' }).click();
  await expect(recoveryPanel).toContainText('全量导入前');
  await expect(recoveryPanel).toContainText('1 条');

  const importPoint = recoveryPanel.getByLabel('恢复点 全量导入前');
  await importPoint.getByRole('button', { name: '手工保留' }).click();
  await expect(importPoint).toContainText('已手工保留');

  page.once('dialog', dialog => dialog.accept());
  await importPoint.getByRole('button', { name: '恢复', exact: true }).click();
  await expect(page.getByText(/已恢复 1 条记录/)).toBeVisible();

  let snapshot = await mockSnapshot(page);
  expect(snapshot.records).toEqual([expect.objectContaining({ id: original.id, notes: '原始状态' })]);
  expect(snapshot.recoveryPoints.some(point => point.reason === 'pre-restore')).toBe(true);
  expect(snapshot.calls.some(call => call.command === 'restore_recovery_point')).toBe(true);

  page.once('dialog', dialog => dialog.accept());
  await importPoint.getByRole('button', { name: '删除' }).click();
  await expect(importPoint).toHaveCount(0);
  snapshot = await mockSnapshot(page);
  expect(snapshot.calls.some(call => call.command === 'set_recovery_point_retained')).toBe(true);
  expect(snapshot.calls.some(call => call.command === 'delete_recovery_point')).toBe(true);
});

test('@conditional-watchlist-boundary conflict choice preserves region fields and clears pending state', async ({ page }) => {
  const current = record('conflict-record', {
    chineseName: '当前保留版本',
    originCountry: 'CN',
    contentTags: '当前标签',
    updatedAt: '2026-03-02T00:00:00.000Z',
  });
  const discarded = record('conflict-record', {
    chineseName: '被覆盖版本',
    originCountry: 'HK, TW, GB',
    contentTags: '悬疑,自定义',
    updatedAt: '2026-03-01T00:00:00.000Z',
  });
  const conflict = {
    id: current.id,
    kind: 'edit-edit',
    fields: ['chineseName', 'originCountry', 'contentTags'],
    base: record('conflict-record', { chineseName: '共同版本' }),
    local: current,
    remote: discarded,
    localDeleted: false,
    remoteDeleted: false,
    detectedAt: '2026-03-03T00:00:00.000Z',
  };
  await setupMockIpc(page, {
    records: [current],
    settings: {
      sync_v3_conflicts: JSON.stringify([conflict]),
    },
  });
  await page.goto('/');
  await page.getByRole('button', { name: '设置' }).click();
  await page.getByRole('button', { name: '☁️ 云端同步', exact: true }).click();

  await expect(page.getByText(/云端：被覆盖版本/)).toBeVisible();
  page.once('dialog', dialog => dialog.accept());
  await page.getByRole('button', { name: '采用云端' }).click();
  await expect(page.getByRole('status').filter({ hasText: '同步冲突已解决' })).toBeVisible();
  await expect(page.getByText('暂无同步冲突记录')).toBeVisible();

  const snapshot = await mockSnapshot(page);
  expect(snapshot.calls.some(call => call.command === 'resolve_sync_conflict'
    && call.args.resolution === 'remote')).toBe(true);
  expect(snapshot.records).toHaveLength(1);
  expect(snapshot.records[0]).toMatchObject({
    id: current.id,
    chineseName: '被覆盖版本',
    originCountry: 'HK, TW, GB',
    contentTags: '悬疑,自定义',
  });
  expect(snapshot.settings.sync_v3_conflicts).toBe('[]');
});
