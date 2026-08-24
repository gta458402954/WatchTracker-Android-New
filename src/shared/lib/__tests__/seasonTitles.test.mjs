import assert from 'node:assert/strict';
import { describe, test } from 'node:test';

import { formatSeasonTitles, seasonNumberFromText } from '../seasonTitles.ts';

describe('season title formatting', () => {
  test('keeps the season identity and uses localized and original subtitles independently', () => {
    assert.deepEqual(formatSeasonTitles({
      seriesName: '时差游戏',
      originalName: 'Jet Lag: The Game',
      seasonNumber: 17,
      localizedSeasonName: '台湾：铁路竞速',
      originalSeasonName: 'Taiwan: Rail Rush',
    }), {
      chineseName: '时差游戏 第 17 季：台湾：铁路竞速',
      originalName: 'Jet Lag: The Game Season 17: Taiwan: Rail Rush',
    });
  });

  test('does not mistake a word prefix for a duplicated series name', () => {
    assert.deepEqual(formatSeasonTitles({
      seriesName: 'Up', originalName: 'Up', seasonNumber: 2,
      localizedSeasonName: 'Uprising', originalSeasonName: 'Uprising',
    }), {
      chineseName: 'Up 第 2 季：Uprising',
      originalName: 'Up Season 2: Uprising',
    });
  });

  test('falls back to the original series name without discarding a valid subtitle', () => {
    assert.deepEqual(formatSeasonTitles({
      originalName: 'Example', seasonNumber: 2, localizedSeasonName: '台湾篇',
    }), {
      chineseName: 'Example 第 2 季：台湾篇',
      originalName: 'Example Season 2',
    });
  });

  test('removes only a matching embedded season marker', () => {
    assert.deepEqual(formatSeasonTitles({
      seriesName: '示例剧', originalName: 'Example', seasonNumber: 17,
      localizedSeasonName: '示例剧 第 17 季：台湾篇', originalSeasonName: 'Example Season 18: Other Race',
    }), {
      chineseName: '示例剧 第 17 季：台湾篇',
      originalName: 'Example Season 17: Other Race',
    });
  });

  test('recognizes Arabic, compact, and Chinese season numbers', () => {
    assert.equal(seasonNumberFromText('Example Season 17: Taiwan'), 17);
    assert.equal(seasonNumberFromText('S03E02'), 3);
    assert.equal(seasonNumberFromText('示例剧 第一百零二季'), 102);
  });
});
