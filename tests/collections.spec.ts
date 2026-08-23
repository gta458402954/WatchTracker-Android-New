import { expect, test } from '@playwright/test';
import type { CollectionMember, WatchCollection, WatchRecord } from '../src/shared/types';
import { mockSnapshot, setupMockIpc } from './fixtures/mockIpc';

function record(id: string, status: WatchRecord['status'] = '未看'): WatchRecord {
  return {
    id, originalName: `${id} original`, chineseName: id, progress: '', totalEpisodes: null,
    movieProgress: null, movieDuration: null, releaseYear: '2026', posterPath: null,
    status, platform: '', rating: null, startDate: '', endDate: '', notes: '',
    createdAt: '2026-08-09T00:00:00.000Z', imdbId: null, mediaType: '电影',
    contentTags: null, originCountry: 'CN',
  };
}

const collection: WatchCollection = {
  id: 'collection-1', name: '测试系列', normalizedName: '测试系列', description: '端到端测试',
  sourceKind: 'manual', sourceKey: null, createdAt: '2026-08-09T00:00:00.000Z',
  collectionKind: 'manual', orderMode: 'manual',
  updatedAt: '2026-08-09T00:00:00.000Z', rev: 1, revActor: 'mock-device',
};

function member(recordId: string, position: number, collectionId = collection.id): CollectionMember {
  return {
    id: `member-${recordId}`, collectionId, recordId, position, sourceKind: 'manual',
    createdAt: '2026-08-09T00:00:00.000Z', updatedAt: '2026-08-09T00:00:00.000Z',
    rev: 1, revActor: 'mock-device',
  };
}

async function openCenter(page: Parameters<typeof setupMockIpc>[0]) {
  await page.getByRole('button', { name: '更多操作' }).click();
  await page.getByRole('menuitem', { name: '系列与收藏集' }).click();
  await expect(page.getByRole('dialog', { name: '系列与收藏集' })).toBeVisible();
}

test('@collections creates a collection, adds members, reorders them, and keeps records when deleted', async ({ page }) => {
  await setupMockIpc(page, { records: [record('第一部'), record('第二部', '已看')] });
  await page.goto('/');
  await openCenter(page);

  await page.getByRole('button', { name: '新建收藏集' }).click();
  await page.getByPlaceholder('收藏集名称').fill('我的系列');
  await page.getByPlaceholder('说明（可选）').fill('不会改变原始记录');
  await page.getByRole('button', { name: '创建', exact: true }).click();
  await expect(page.getByRole('heading', { name: '我的系列' })).toBeVisible();

  await page.getByRole('button', { name: '从片库添加' }).first().click();
  const addDialog = page.getByRole('dialog', { name: '从片库添加' });
  await addDialog.getByText('第一部', { exact: true }).click();
  await addDialog.getByText('第二部', { exact: true }).click();
  await addDialog.getByRole('button', { name: '加入收藏集' }).click();
  await expect(page.getByText('🎬 2 部作品')).toBeVisible();

  await page.getByRole('button', { name: '上移 第二部' }).click();
  const titles = await page.getByRole('main').locator('button').filter({ hasText: /第一部|第二部/ }).allTextContents();
  expect(titles.join('|').indexOf('第二部')).toBeLessThan(titles.join('|').indexOf('第一部'));
  await page.getByRole('button', { name: '从收藏集移除 第二部' }).click();
  await expect(page.getByText('🎬 1 部作品')).toBeVisible();
  expect((await mockSnapshot(page)).records.some(item => item.id === '第二部')).toBe(true);

  page.once('dialog', dialog => dialog.accept());
  await page.getByRole('dialog', { name: '系列与收藏集' }).getByRole('button', { name: '删除', exact: true }).click();
  await expect(page.getByText('创建第一个收藏集')).toBeVisible();
  const snapshot = await mockSnapshot(page);
  expect(snapshot.records.map(item => item.id).sort()).toEqual(['第一部', '第二部']);
  expect(snapshot.collections).toHaveLength(0);
  expect(snapshot.collectionMembers).toHaveLength(0);
});

test('@collections opening is read-only until an explicit action', async ({ page }) => {
  await setupMockIpc(page, { records: [record('只读检查')], collections: [collection] });
  await page.goto('/');
  const before = await mockSnapshot(page);
  await openCenter(page);
  await expect(page.getByRole('heading', { name: '测试系列' })).toBeVisible();
  const after = await mockSnapshot(page);
  const writes = new Set(['create_collection', 'update_collection', 'delete_collection', 'add_collection_members', 'remove_collection_member', 'reorder_collection_members']);
  expect(after.calls.filter(call => writes.has(call.command))).toHaveLength(before.calls.filter(call => writes.has(call.command)).length);
});

test('@collections keeps library suggestions separate from a selected collection detail', async ({ page }) => {
  const inside = record('收藏集具体条目');
  const candidate = {
    ...record('待归组电影'),
    imdbId: 'tt1234567',
  };
  await setupMockIpc(page, {
    records: [inside, candidate],
    collections: [collection],
    collectionMembers: [member(inside.id, 0)],
    tmdbSearchResults: [{ id: 101, media_type: 'movie', title: '待归组电影' }],
    tmdbDetail: {
      id: 101,
      media_type: 'movie',
      title: '待归组电影',
      belongs_to_collection: { id: 202, name: '待归组系列' },
    },
    tmdbDetails: {
      'movie:101': { id: 101, media_type: 'movie', title: '待归组电影', belongs_to_collection: { id: 202, name: '待归组系列' } },
      'collection:202': { id: 202, name: '待归组系列', parts: [
        { id: 101, title: '待归组电影', release_date: '2020-01-01' },
        { id: 102, title: '待归组电影续集', release_date: '2022-01-01' },
      ] },
    },
  });
  await page.goto('/');
  await openCenter(page);

  await page.getByRole('button', { name: '扫描片库归组建议' }).click();
  await expect(page.getByRole('heading', { name: '片库归组建议' })).toBeVisible();
  await expect(page.getByText('TMDB 只读建议 · 确认前不会修改数据')).toBeVisible();
  await expect(page.getByRole('button', { name: /待归组系列.*应用/ })).toBeVisible();

  await page.getByRole('button', { name: /测试系列.*1 部/ }).click();
  await expect(page.getByRole('heading', { name: '测试系列' })).toBeVisible();
  await expect(page.getByRole('button', { name: /收藏集具体条目.*original/ })).toBeVisible();
  await expect(page.getByText('TMDB 只读建议 · 确认前不会修改数据')).toHaveCount(0);
  await expect(page.getByRole('button', { name: /待归组系列.*应用/ })).toHaveCount(0);
});

test('@collections arrow keys switch the focused collection and search can enter the list', async ({ page }) => {
  const second: WatchCollection = {
    ...collection,
    id: 'collection-2',
    name: '第二系列',
    normalizedName: '第二系列',
  };
  await setupMockIpc(page, { collections: [collection, second] });
  await page.goto('/');
  await openCenter(page);
  const dialog = page.getByRole('dialog', { name: '系列与收藏集' });
  const firstButton = dialog.getByRole('button', { name: /测试系列.*0 部/ });
  const secondButton = dialog.getByRole('button', { name: /第二系列.*0 部/ });

  await firstButton.focus();
  await firstButton.press('ArrowDown');
  await expect(secondButton).toBeFocused();
  await expect(dialog.getByRole('heading', { name: '第二系列' })).toBeVisible();
  await secondButton.press('Home');
  await expect(firstButton).toBeFocused();
  await expect(dialog.getByRole('heading', { name: '测试系列' })).toBeVisible();

  const search = dialog.getByPlaceholder('搜索收藏集');
  await search.fill('第二');
  await search.press('ArrowDown');
  await expect(secondButton).toBeFocused();
  await expect(dialog.getByRole('heading', { name: '第二系列' })).toBeVisible();
});

test('@collections record editing returns to the originating collection after cancel and save', async ({ page }) => {
  const inside = record('集合内条目');
  await setupMockIpc(page, {
    records: [inside],
    collections: [collection],
    collectionMembers: [member(inside.id, 0)],
  });
  await page.goto('/');
  await openCenter(page);

  await page.getByRole('button', { name: /集合内条目.*original/ }).click();
  await expect(page.getByRole('heading', { name: '编辑记录' })).toBeVisible();
  await page.getByRole('button', { name: '取消', exact: true }).click();
  await expect(page.getByRole('dialog', { name: '系列与收藏集' })).toBeVisible();
  await expect(page.getByRole('heading', { name: '测试系列' })).toBeVisible();

  await page.getByRole('button', { name: /集合内条目.*original/ }).click();
  await page.getByPlaceholder('请输入中文名称').fill('集合内条目（已编辑）');
  await page.getByRole('button', { name: '保存修改' }).click();
  await expect(page.getByRole('dialog', { name: '系列与收藏集' })).toBeVisible();
  await expect(page.getByRole('heading', { name: '测试系列' })).toBeVisible();
  await expect(page.getByRole('button', { name: /^集合内条目（已编辑） · 2026/ })).toBeVisible();
});

test('@collections remains page-safe at 360px', async ({ page }) => {
  await page.setViewportSize({ width: 360, height: 740 });
  await setupMockIpc(page, {
    records: [record('窄屏电影')],
    collections: [collection],
    collectionMembers: [member('窄屏电影', 0)],
  });
  await page.goto('/');
  await openCenter(page);
  await expect(page.getByRole('heading', { name: '测试系列' })).toBeVisible();
  expect(await page.locator('body').evaluate(element => element.scrollWidth > element.clientWidth)).toBe(false);
  await page.getByRole('button', { name: '返回收藏集' }).click();
  await expect(page.getByPlaceholder('搜索收藏集')).toBeVisible();
});

test('@collections creates a staged collection from record editing only when the record is saved', async ({ page }) => {
  await setupMockIpc(page, { records: [record('权力的游戏 第 1 季')] });
  await page.goto('/');
  await page.getByTitle('编辑').click();
  await expect(page.getByRole('heading', { name: '编辑记录' })).toBeVisible();
  await expect(page.getByRole('heading', { name: '整理与归组' })).toBeVisible();
  await page.getByRole('button', { name: '管理' }).click();
  const manager = page.getByRole('dialog', { name: '管理所属收藏集' });
  await manager.getByRole('button', { name: '新建' }).click();
  await manager.getByPlaceholder('收藏集名称').fill('权力的游戏');
  await manager.locator('select').selectOption('tv-series');
  await manager.getByRole('button', { name: '加入待创建列表' }).click();
  await manager.getByRole('button', { name: '完成' }).click();
  expect((await mockSnapshot(page)).collections).toHaveLength(0);
  await page.getByRole('button', { name: '保存修改' }).click();
  const snapshot = await mockSnapshot(page);
  expect(snapshot.collections).toHaveLength(1);
  expect(snapshot.collections[0].collectionKind).toBe('tv-series');
  expect(snapshot.collectionMembers).toHaveLength(1);
});

test('@arch002 collection manager Escape restores focus without writing', async ({ page }) => {
  await setupMockIpc(page, { records: [record('escape-record')] });
  await page.goto('/');
  await page.getByTitle('编辑').click();
  const form = page.getByRole('dialog', { name: '编辑记录' });
  const manage = form.getByRole('button', { name: '管理' });
  await manage.click();
  const manager = page.getByRole('dialog', { name: '管理所属收藏集' });
  await expect(manager).toBeVisible();
  await expect(manager.getByPlaceholder('搜索收藏集')).toBeFocused();
  const writesBefore = (await mockSnapshot(page)).calls.filter(call => /^(create_|add_|update_|delete_)/.test(call.command));
  await page.keyboard.press('Escape');
  await expect(manager).toHaveCount(0);
  await expect(manage).toBeFocused();
  const writesAfter = (await mockSnapshot(page)).calls.filter(call => /^(create_|add_|update_|delete_)/.test(call.command));
  expect(writesAfter).toEqual(writesBefore);
});

test('@collections suppresses an already complete canonical TMDB series suggestion', async ({ page }) => {
  const blackMirror: WatchCollection = {
    ...collection, id: 'black-mirror', name: '黑镜', normalizedName: '黑镜', sourceKind: 'tmdb-tv-show',
    sourceKey: 'tmdb:tv-show:42009', collectionKind: 'tv-series', orderMode: 'chronological',
  };
  const records = [3, 4, 5, 6, 7].map(season => ({
    ...record(`black-mirror-${season}`), chineseName: `黑镜 第 ${season} 季`, originalName: `Black Mirror Season ${season}`,
    imdbId: `tt-black-mirror-${season}`, mediaType: '剧集' as const, tmdbMediaKind: 'tv-season' as const,
    tmdbParentId: 42009, tmdbSeasonNumber: season,
  }));
  await setupMockIpc(page, {
    records,
    collections: [blackMirror],
    collectionMembers: records.map((item, index) => member(item.id, index * 1024, blackMirror.id)),
  });
  await page.goto('/');
  await openCenter(page);
  await page.getByRole('button', { name: '扫描片库归组建议' }).click();
  await expect(page.getByText('TMDB 只读建议 · 确认前不会修改数据')).toHaveCount(0);
  expect((await mockSnapshot(page)).calls.filter(call => call.command === 'search_tmdb')).toHaveLength(0);
});

test('@collections upgrades a manual single-parent series before checking missing seasons', async ({ page }) => {
  const youngSheldon: WatchCollection = { ...collection, id: 'young-sheldon', name: '小谢尔顿', normalizedName: '小谢尔顿' };
  const records = [1, 5].map(season => ({
    ...record(`young-sheldon-${season}`), chineseName: `小谢尔顿 第 ${season} 季`, originalName: `Young Sheldon Season ${season}`,
    mediaType: '剧集' as const, tmdbMediaKind: 'tv-season' as const, tmdbId: 88000 + season,
    tmdbParentId: 71728, tmdbSeasonNumber: season,
  }));
  await setupMockIpc(page, {
    records,
    collections: [youngSheldon],
    collectionMembers: records.map((item, index) => member(item.id, index * 1024, youngSheldon.id)),
    tmdbDetail: {
      id: 71728, name: '小谢尔顿', original_name: 'Young Sheldon',
      seasons: [1, 2, 3, 4, 5, 6, 7].map(season => ({ id: 90000 + season, season_number: season, name: `第 ${season} 季`, air_date: `20${16 + season}-09-01`, episode_count: 22 })),
    },
  });
  await page.goto('/');
  await openCenter(page);
  page.once('dialog', dialog => dialog.accept());
  await page.getByRole('button', { name: '检查缺失季' }).click();
  const missingDialog = page.getByRole('dialog', { name: '检查缺失季' });
  await expect(missingDialog).toBeVisible();
  await expect(missingDialog.getByText('第 2 季 · 2018')).toBeVisible();
  const snapshot = await mockSnapshot(page);
  expect(snapshot.collections[0]).toMatchObject({
    sourceKind: 'tmdb-tv-show', sourceKey: 'tmdb:tv-show:71728', collectionKind: 'tv-series', orderMode: 'chronological',
  });
});

test('@collections identifies a legacy shared-IMDb series without scanning the library', async ({ page }) => {
  const legacy: WatchCollection = { ...collection, id: 'legacy-show', name: '高堡奇人', normalizedName: '高堡奇人' };
  const records = [1, 2].map(season => ({
    ...record(`legacy-show-${season}`), chineseName: `高堡奇人 第 ${season} 季`, originalName: `The Man in the High Castle Season ${season}`,
    imdbId: 'tt1740299', mediaType: '剧集' as const,
  }));
  await setupMockIpc(page, {
    records,
    collections: [legacy],
    collectionMembers: records.map((item, index) => member(item.id, index * 1024, legacy.id)),
    tmdbSearchResults: [{ id: 62017, name: '高堡奇人', original_name: 'The Man in the High Castle', first_air_date: '2015-01-15', media_type: 'tv' }],
    tmdbDetail: {
      id: 62017, name: '高堡奇人', original_name: 'The Man in the High Castle',
      seasons: [1, 2, 3, 4].map(season => ({ id: 92000 + season, season_number: season, name: `第 ${season} 季`, air_date: `201${4 + season}-01-01`, episode_count: 10 })),
    },
  });
  await page.goto('/');
  await openCenter(page);
  page.once('dialog', dialog => dialog.accept());
  await page.getByRole('button', { name: '确认 TMDB 系列' }).click();
  const missingDialog = page.getByRole('dialog', { name: '检查缺失季' });
  await expect(missingDialog).toBeVisible();
  await expect(missingDialog.getByText('第 3 季 · 2017')).toBeVisible();

  const snapshot = await mockSnapshot(page);
  expect(snapshot.collections[0]).toMatchObject({
    sourceKind: 'tmdb-tv-show', sourceKey: 'tmdb:tv-show:62017', collectionKind: 'tv-series', orderMode: 'chronological',
  });
  expect(snapshot.records.every(item => item.tmdbParentId == null)).toBe(true);
  expect(snapshot.calls.filter(call => call.command === 'search_tmdb')).toHaveLength(1);
});

test('@collections requires an explicit choice when legacy IMDb matches multiple TV series', async ({ page }) => {
  const legacy: WatchCollection = { ...collection, id: 'legacy-choice', name: '旧系列', normalizedName: '旧系列' };
  const records = [1, 2].map(season => ({
    ...record(`legacy-choice-${season}`), chineseName: `旧系列 第 ${season} 季`, originalName: `Legacy Show Season ${season}`,
    imdbId: 'tt1234567', mediaType: '剧集' as const,
  }));
  await setupMockIpc(page, {
    records,
    collections: [legacy],
    collectionMembers: records.map((item, index) => member(item.id, index * 1024, legacy.id)),
    tmdbSearchResults: [
      { id: 100, name: '候选一', first_air_date: '2010-01-01', media_type: 'tv' },
      { id: 200, name: '候选二', first_air_date: '2020-01-01', media_type: 'tv' },
    ],
  });
  await page.goto('/');
  await openCenter(page);
  await page.getByRole('button', { name: '确认 TMDB 系列' }).click();
  await expect(page.getByRole('dialog', { name: '选择 TMDB 电视剧系列' })).toBeVisible();
  const snapshot = await mockSnapshot(page);
  expect(snapshot.calls.filter(call => call.command === 'update_collection')).toHaveLength(0);
});

test('@collections suppresses a single-season work and a work already covered by any collection', async ({ page }) => {
  const redemption = {
    ...record('redemption'), chineseName: '罪恶黑名单：救赎', originalName: 'The Blacklist: Redemption',
    imdbId: 'tt5592230', mediaType: '剧集' as const,
  };
  const blacklist: WatchCollection = { ...collection, id: 'blacklist', name: '罪恶黑名单', normalizedName: '罪恶黑名单', collectionKind: 'universe' };
  await setupMockIpc(page, {
    records: [redemption],
    collections: [blacklist],
    collectionMembers: [member(redemption.id, 0, blacklist.id)],
    tmdbSearchResults: [{ id: 68841, name: '罪恶黑名单：救赎', media_type: 'tv' }],
    tmdbDetail: { id: 68841, name: '罪恶黑名单：救赎', seasons: [{ id: 1, season_number: 1, air_date: '2017-02-23' }] },
  });
  await page.goto('/');
  await openCenter(page);
  await page.getByRole('button', { name: '扫描片库归组建议' }).click();
  await expect(page.getByText('TMDB 只读建议 · 确认前不会修改数据')).toHaveCount(0);
  await expect(page.getByText(/已归组 1/)).toBeVisible();
  const snapshot = await mockSnapshot(page);
  expect(snapshot.calls.filter(call => call.command === 'apply_collection_suggestion')).toHaveLength(0);
});

test('@collections excludes a TMDB-confirmed single aired season outside collections', async ({ page }) => {
  const singleSeason = {
    ...record('single-season'), chineseName: '只有一季', originalName: 'One Season Only',
    imdbId: 'tt1234999', mediaType: '剧集' as const,
  };
  await setupMockIpc(page, {
    records: [singleSeason],
    collections: [collection],
    tmdbSearchResults: [{ id: 4321, name: '只有一季', media_type: 'tv' }],
    tmdbDetail: { id: 4321, name: '只有一季', seasons: [
      { id: 0, season_number: 0, air_date: '2020-01-01' },
      { id: 1, season_number: 1, air_date: '2021-01-01' },
      { id: 2, season_number: 2, air_date: '2099-01-01' },
    ] },
  });
  await page.goto('/');
  await openCenter(page);
  await page.getByRole('button', { name: '扫描片库归组建议' }).click();
  await expect(page.getByText('TMDB 只读建议 · 确认前不会修改数据')).toHaveCount(0);
  await expect(page.getByText(/可处理 0 · 完整排除 1 · 已归组 0/)).toBeVisible();
  expect((await mockSnapshot(page)).calls.filter(call => call.command === 'get_tmdb_detail')).toHaveLength(1);
});

test('@collections filters invalid search matches before deciding that a result is ambiguous', async ({ page }) => {
  const candidate = {
    ...record('大时代'), originalName: 'The Greed of Man', imdbId: 'tt0843185', mediaType: '剧集' as const,
  };
  await setupMockIpc(page, {
    records: [candidate],
    collections: [collection],
    tmdbSearchResults: [
      { id: 810, title: 'The Greed of Man', media_type: 'movie', release_date: '1992-01-01' },
      { id: 820, name: '大时代', original_name: 'The Greed of Man', media_type: 'tv', first_air_date: '1992-10-05' },
    ],
    tmdbDetails: {
      'movie:810': { id: 810, title: 'The Greed of Man', media_type: 'movie', release_date: '1992-01-01', belongs_to_collection: null },
      'tv:820': { id: 820, name: '大时代', original_name: 'The Greed of Man', media_type: 'tv', seasons: [
        { id: 821, season_number: 1, air_date: '1992-10-05' },
        { id: 822, season_number: 2, air_date: '1993-10-05' },
      ] },
    },
  });
  await page.goto('/');
  await openCenter(page);
  await page.getByRole('button', { name: '扫描片库归组建议' }).click();

  await expect(page.getByText(/存在多个有效 TMDB 匹配/)).toHaveCount(0);
  await expect(page.getByRole('button', { name: /大时代.*应用/ })).toBeVisible();
  await expect(page.getByText(/非合集\/无差异 1/)).toBeVisible();
  expect((await mockSnapshot(page)).calls.filter(call => call.command === 'apply_collection_suggestion')).toHaveLength(0);
});

test('@collections keeps only an actionable movie collection when the TV result is already complete', async ({ page }) => {
  const candidate = {
    ...record('人生七年9'), originalName: '63 Up', imdbId: 'tt8929142', mediaType: '电影' as const,
  };
  await setupMockIpc(page, {
    records: [candidate],
    collections: [collection],
    tmdbSearchResults: [
      { id: 901, title: '人生七年9', original_title: '63 Up', media_type: 'movie', release_date: '2019-06-04' },
      { id: 902, name: '人生七年9', original_name: '63 Up', media_type: 'tv', first_air_date: '2019-06-04' },
    ],
    tmdbDetails: {
      'movie:901': { id: 901, title: '人生七年9', media_type: 'movie', release_date: '2019-06-04', belongs_to_collection: { id: 990, name: '人生七年（系列）' } },
      'collection:990': { id: 990, name: '人生七年（系列）', parts: [
        { id: 900, title: '人生七年8', release_date: '2012-01-01' },
        { id: 901, title: '人生七年9', release_date: '2019-06-04' },
      ] },
      'tv:902': { id: 902, name: '人生七年9', media_type: 'tv', seasons: [{ id: 903, season_number: 1, air_date: '2019-06-04' }] },
    },
  });
  await page.goto('/');
  await openCenter(page);
  await page.getByRole('button', { name: '扫描片库归组建议' }).click();

  await expect(page.getByText(/存在多个有效 TMDB 匹配/)).toHaveCount(0);
  await expect(page.getByRole('button', { name: /人生七年（系列）.*应用/ })).toBeVisible();
  await expect(page.getByText(/完整排除 1/)).toBeVisible();
});

test('@collections reuses a legacy documentary by IMDb when completing a movie series', async ({ page }) => {
  const movieSeries: WatchCollection = {
    ...collection,
    name: '人生七年（系列）',
    normalizedName: '人生七年（系列）',
    sourceKind: 'tmdb-movie-collection',
    sourceKey: 'tmdb:movie-collection:699',
    collectionKind: 'movie-series',
    orderMode: 'chronological',
  };
  const existingMember = {
    ...record('人生七年5'),
    imdbId: 'tt0098575',
    tmdbMediaKind: 'movie' as const,
    tmdbId: 600,
  };
  const legacyDocumentary = {
    ...record('人生七年6'),
    originalName: '42 Up',
    mediaType: '纪录片' as const,
    imdbId: 'tt0164312',
    tmdbMediaKind: null,
    tmdbId: null,
  };
  await setupMockIpc(page, {
    records: [existingMember, legacyDocumentary],
    collections: [movieSeries],
    collectionMembers: [member(existingMember.id, 0)],
    tmdbDetails: {
      'collection:699': { id: 699, name: '人生七年（系列）', parts: [
        { id: 600, title: '35 Up', release_date: '1991-01-01' },
        { id: 610, title: '42 Up', release_date: '1998-07-21' },
      ] },
      'movie:600': { id: 600, title: '35 Up', release_date: '1991-01-01', external_ids: { imdb_id: 'tt0098575' } },
      'movie:610': { id: 610, title: '42 Up', release_date: '1998-07-21', external_ids: { imdb_id: 'tt0164312' } },
    },
  });
  await page.goto('/');
  await openCenter(page);
  await page.getByRole('button', { name: '检查缺失电影' }).click();
  const dialog = page.getByRole('dialog', { name: '检查缺失电影' });
  await expect(dialog.getByText('42 Up · 1998')).toBeVisible();
  await expect(dialog.getByText('已在片库，可加入收藏集')).toBeVisible();
  await dialog.getByRole('button', { name: '补充到片库' }).click();

  const snapshot = await mockSnapshot(page);
  expect(snapshot.records).toHaveLength(2);
  const reused = snapshot.records.find(item => item.id === legacyDocumentary.id);
  expect(reused).toMatchObject({ mediaType: '纪录片', tmdbMediaKind: 'movie', tmdbId: 610 });
  expect(snapshot.collectionMembers.some(item => item.collectionId === movieSeries.id && item.recordId === legacyDocumentary.id)).toBe(true);
});

test('@collections asks only between multiple qualified sources and keeps the choice read-only', async ({ page }) => {
  const candidate = {
    ...record('人生七年6'), originalName: '42 Up', imdbId: 'tt0164312', mediaType: '纪录片' as const,
  };
  await setupMockIpc(page, {
    records: [candidate],
    collections: [collection],
    tmdbSearchResults: [
      { id: 610, title: '人生七年6', original_title: '42 Up', media_type: 'movie', release_date: '1998-07-21' },
      { id: 620, name: '42: Forty Two Up', media_type: 'tv', first_air_date: '1998-07-21' },
    ],
    tmdbDetails: {
      'movie:610': { id: 610, title: '人生七年6', original_title: '42 Up', media_type: 'movie', release_date: '1998-07-21', belongs_to_collection: { id: 699, name: '人生七年（系列）' } },
      'collection:699': { id: 699, name: '人生七年（系列）', parts: [
        { id: 600, title: '人生七年5', release_date: '1991-01-01' },
        { id: 610, title: '人生七年6', release_date: '1998-07-21' },
      ] },
      'tv:620': { id: 620, name: '42: Forty Two Up', media_type: 'tv', seasons: [
        { id: 621, season_number: 1, air_date: '1998-07-21' },
        { id: 622, season_number: 2, air_date: '1999-07-21' },
      ] },
    },
  });
  await page.goto('/');
  await openCenter(page);
  await page.getByRole('button', { name: '扫描片库归组建议' }).click();

  await expect(page.getByText('1 项存在多个有效 TMDB 匹配，请选择')).toBeVisible();
  await expect(page.getByText('本地：人生七年6')).toBeVisible();
  await expect(page.getByRole('button', { name: /人生七年6 · 电影.*TMDB 610.*电影合集：人生七年（系列）/ })).toBeVisible();
  await expect(page.getByRole('button', { name: /42: Forty Two Up · 剧集.*TMDB 620.*电视剧系列/ })).toBeVisible();
  const snapshot = await mockSnapshot(page);
  expect(snapshot.calls.filter(call => ['apply_collection_suggestion', 'create_collection', 'add_collection_members'].includes(call.command))).toHaveLength(0);
});

test('@collections persists dismissed suggestions and can restore them', async ({ page }) => {
  const candidate = {
    ...record('candidate'), chineseName: '多季候选 第 1 季', originalName: 'Multi Season Candidate Season 1',
    imdbId: 'tt1234000', mediaType: '剧集' as const,
  };
  await setupMockIpc(page, {
    records: [candidate],
    collections: [collection],
    tmdbSearchResults: [{ id: 1234, name: '多季候选', media_type: 'tv' }],
    tmdbDetail: { id: 1234, name: '多季候选', seasons: [
      { id: 1, season_number: 1, air_date: '2020-01-01' },
      { id: 2, season_number: 2, air_date: '2021-01-01' },
    ] },
  });
  await page.goto('/');
  await openCenter(page);
  await page.getByRole('button', { name: '扫描片库归组建议' }).click();
  await expect(page.getByRole('button', { name: '不再推荐 多季候选' })).toBeVisible();
  expect((await mockSnapshot(page)).calls.filter(call => call.command === 'get_tmdb_detail')).toHaveLength(1);
  await page.getByRole('button', { name: '不再推荐 多季候选' }).click();
  await expect(page.getByRole('button', { name: '已忽略建议（1）' })).toBeEnabled();

  await page.getByRole('button', { name: '关闭收藏集中心' }).first().click();
  await openCenter(page);
  await page.getByRole('button', { name: '扫描片库归组建议' }).click();
  await expect(page.getByRole('button', { name: '不再推荐 多季候选' })).toHaveCount(0);
  await expect(page.getByText('非合集/无差异 0 · 已忽略 1 · 待确认 0 · 无法确认 0', { exact: true })).toBeVisible();

  await page.getByRole('button', { name: '已忽略建议（1）' }).click();
  const ignoredDialog = page.getByRole('dialog', { name: '已忽略建议' });
  await ignoredDialog.getByRole('button', { name: '恢复', exact: true }).click();
  await expect(page.getByRole('button', { name: '已忽略建议（0）' })).toBeDisabled();
  const snapshot = await mockSnapshot(page);
  expect(snapshot.settings.collection_suggestion_dismissals_v1).toContain('"entries":[]');
  expect(snapshot.calls.filter(call => call.command === 'apply_collection_suggestion')).toHaveLength(0);
  await page.close();
});
