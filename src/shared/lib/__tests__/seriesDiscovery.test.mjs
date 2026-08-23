import test from 'node:test';
import assert from 'node:assert/strict';
import { chronologicalRecords, defaultMissingSeasonNumbers, locallyKnownSeries, recordSeasonIdentity, seasonNumberOf } from '../../../features/collections/lib/seriesDiscovery.ts';

const record = (id, chineseName, releaseYear = null) => ({ id, chineseName, originalName: '', progress: '', releaseYear, imdbId: null });

test('detects Chinese, English and compact season markers', () => {
  assert.equal(seasonNumberOf(record('1', '傲骨之战 第 5 季')), 5);
  assert.equal(seasonNumberOf(record('2', '小谢尔顿 第五季')), 5);
  assert.equal(seasonNumberOf(record('3', '实习医生格蕾 第二十二季')), 22);
  assert.equal(seasonNumberOf({ chineseName: '', originalName: 'The Good Fight Season 2', progress: '' }), 2);
  assert.equal(seasonNumberOf({ chineseName: '', originalName: '', progress: 'S03E02' }), 3);
});

test('chronological order is old to new, unknown last and season zero after regular seasons', () => {
  const values = chronologicalRecords([record('u', '未知'), record('s0', '剧 第0季', '2020'), record('s2', '剧 第2季', '2020'), record('old', '旧作', '2018')]);
  assert.deepEqual(values.map(item => item.id), ['old', 's2', 's0', 'u']);
});

test('local discovery groups explicit seasons', () => {
  const values = locallyKnownSeries([record('1', '生活大爆炸 第1季'), record('2', '生活大爆炸 第2季')]);
  assert.equal(values.length, 1);
  assert.deepEqual(values[0].seasons, [1, 2]);
});

test('local discovery groups Arabic and Chinese season names under the same base title', () => {
  const values = locallyKnownSeries([
    { ...record('1', '小谢尔顿 第 1 季'), originalName: 'Young Sheldon Season 1' },
    { ...record('5', '小谢尔顿 第五季'), originalName: 'Young Sheldon Season 5' },
  ]);
  assert.equal(values.length, 1);
  assert.equal(values[0].name, '小谢尔顿');
  assert.deepEqual(values[0].seasons, [1, 5]);
});

test('stable parent IDs use the same canonical source key as collections', () => {
  const values = locallyKnownSeries([
    { ...record('3', '黑镜 第 3 季'), originalName: 'Black Mirror Season 3', tmdbParentId: 42009 },
    { ...record('7', '黑镜 第 7 季'), originalName: 'Black Mirror Season 7', tmdbParentId: 42009 },
  ]);
  assert.equal(values[0].key, 'tmdb:tv-show:42009');
  assert.equal(values[0].tmdbParentId, 42009);
});

test('conflicting localized and original season numbers require confirmation', () => {
  const value = { chineseName: '示例剧 第 2 季', originalName: 'Example Season 3', progress: '' };
  assert.deepEqual(recordSeasonIdentity(value), { seasonNumber: null, conflicted: true });
  assert.equal(locallyKnownSeries([{ ...record('conflict', value.chineseName), originalName: value.originalName }]).length, 0);
});

test('missing season defaults include only aired regular seasons', () => {
  const seasons = [{ season_number: 0, air_date: '2010-01-01' }, { season_number: 1, air_date: '2011-01-01' }, { season_number: 2, air_date: '2099-01-01' }, { season_number: 3, air_date: null }];
  assert.deepEqual(defaultMissingSeasonNumbers(seasons, new Set(), new Date('2026-01-01T00:00:00Z')), [1]);
});
