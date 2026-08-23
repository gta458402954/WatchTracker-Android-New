import assert from 'node:assert/strict';
import { describe, test } from 'node:test';

import { displayTitlesOf } from '../displayTitle.ts';

function record(overrides = {}) {
  return {
    chineseName: '示例剧 第一季',
    originalName: 'Example Season 1',
    mediaType: '剧集',
    originCountry: 'CN',
    contentTags: null,
    ...overrides,
  };
}

describe('mainland-China first-season display titles', () => {
  test('hides redundant Chinese and English first-season suffixes without mutating source values', () => {
    const source = record();
    assert.deepEqual(displayTitlesOf(source), { primary: '示例剧', secondary: 'Example' });
    assert.equal(source.chineseName, '示例剧 第一季');
    assert.equal(source.originalName, 'Example Season 1');
  });

  test('recognizes numeric spacing and a legacy mainland tag', () => {
    assert.deepEqual(displayTitlesOf(record({
      chineseName: '示例剧 第 1 季',
      originalName: 'Example Season 1',
      originCountry: null,
      contentTags: '中国大陆,悬疑',
    })), { primary: '示例剧', secondary: 'Example' });
  });

  test('keeps later seasons and official part names visible', () => {
    assert.deepEqual(displayTitlesOf(record({
      chineseName: '示例剧 第二季', originalName: 'Example Season 2',
    })), { primary: '示例剧 第二季', secondary: 'Example Season 2' });
    assert.equal(displayTitlesOf(record({ chineseName: '示例剧 下部' })).primary, '示例剧 下部');
  });

  test('does not alter non-mainland or movie titles', () => {
    assert.equal(displayTitlesOf(record({ originCountry: 'US' })).primary, '示例剧 第一季');
    assert.equal(displayTitlesOf(record({ mediaType: '电影' })).primary, '示例剧 第一季');
  });
});
