import assert from 'node:assert/strict';
import { describe, test } from 'node:test';

import {
  EMPTY_WATCHLIST_QUERY,
  activeQueryDimensionCount,
  activeAdvancedQueryDimensionCount,
  advancedQuerySummaryItems,
  clearAdvancedQuery,
  filterRecordsByQuery,
  normalizeWatchlistQuery,
  querySummaryItems,
  watchlistFilterOptions,
} from '../watchlistQuery.ts';
import {
  MAX_SAVED_VIEWS,
  parseSavedViews,
  serializeSavedViews,
  validateSavedViewName,
} from '../savedViews.ts';

function record(id, overrides = {}) {
  return {
    id, originalName: id, chineseName: id, progress: '', totalEpisodes: null,
    movieProgress: null, movieDuration: null, releaseYear: null, posterPath: null,
    status: '未看', platform: '', rating: null, startDate: '', endDate: '', notes: '',
    createdAt: id, imdbId: null, mediaType: '电影', contentTags: null, originCountry: null,
    ...overrides,
  };
}

const records = [
  record('cn-drama', { mediaType: '剧集', status: '在看', originCountry: 'CN,US', platform: '腾讯视频', genres: '剧情,悬疑', contentTags: '中国大陆,刑侦', rating: 9, imdbRating: 8.2, releaseYear: '2025', isLocked: true }),
  record('uk-movie', { status: '已看', originCountry: 'UK', platform: 'Netflix', genres: '剧情', contentTags: '经典', rating: 7, imdbRating: 7.5, releaseYear: '1999' }),
  record('unknown', { contentTags: '实验', releaseYear: '未知' }),
];

describe('advanced watchlist query', () => {
  test('uses OR within one dimension and AND across dimensions', () => {
    const query = normalizeWatchlistQuery({
      ...EMPTY_WATCHLIST_QUERY,
      mediaTypes: ['剧集', '综艺'], statuses: ['在看', '未看'], regions: ['CN'],
      genres: ['悬疑'], rating: { min: 8, max: null }, lock: 'locked',
    });
    assert.deepEqual(filterRecordsByQuery(records, query).map(item => item.id), ['cn-drama']);
  });

  test('keeps first-country, unknown-region, text, and inclusive range behavior', () => {
    const cn = normalizeWatchlistQuery({ ...EMPTY_WATCHLIST_QUERY, regions: ['US'] });
    assert.deepEqual(filterRecordsByQuery(records, cn), []);
    const unknown = normalizeWatchlistQuery({ ...EMPTY_WATCHLIST_QUERY, regions: ['__UNKNOWN__'], contentTags: ['实验'] });
    assert.deepEqual(filterRecordsByQuery(records, unknown).map(item => item.id), ['unknown']);
    const range = normalizeWatchlistQuery({ ...EMPTY_WATCHLIST_QUERY, searchText: 'NETFLIX', releaseYear: { min: 1999, max: 1999 } });
    assert.deepEqual(filterRecordsByQuery(records, range).map(item => item.id), ['uk-movie']);
  });

  test('normalizes duplicate values and preserves unknown enum values as non-matching', () => {
    const query = normalizeWatchlistQuery({ ...EMPTY_WATCHLIST_QUERY, mediaTypes: ['未来类型', '未来类型'], rating: { min: 11, max: -1 } });
    assert.deepEqual(query.mediaTypes, ['未来类型']);
    assert.deepEqual(query.rating, { min: 0, max: 10 });
    assert.equal(filterRecordsByQuery(records, query).length, 0);
  });

  test('aggregates dynamic values without legacy country labels and summarizes active dimensions', () => {
    const options = watchlistFilterOptions(records);
    assert.deepEqual(options.regions.map(option => option.value), ['CN', 'GB', '__UNKNOWN__']);
    assert.deepEqual(options.contentTags.map(option => option.value).sort(), ['刑侦', '实验', '经典']);
    const query = normalizeWatchlistQuery({ ...EMPTY_WATCHLIST_QUERY, platforms: ['Netflix'], imdbRating: { min: 8, max: null } });
    assert.equal(activeQueryDimensionCount(query), 2);
    assert.deepEqual(querySummaryItems(query).map(item => item.label), ['平台：Netflix', 'IMDb 评分 ≥ 8']);
  });

  test('counts, summarizes, and clears only advanced dimensions', () => {
    const query = normalizeWatchlistQuery({
      ...EMPTY_WATCHLIST_QUERY,
      searchText: '悬疑', mediaTypes: ['剧集'], statuses: ['在看'], regions: ['CN'], lock: 'locked',
      platforms: ['腾讯视频'], rating: { min: 8, max: null },
    });
    assert.equal(activeQueryDimensionCount(query), 7);
    assert.equal(activeAdvancedQueryDimensionCount(query), 2);
    assert.deepEqual(advancedQuerySummaryItems(query).map(item => item.label), ['平台：腾讯视频', '个人评分 ≥ 8']);
    assert.deepEqual(clearAdvancedQuery(query), normalizeWatchlistQuery({
      ...EMPTY_WATCHLIST_QUERY,
      searchText: '悬疑', mediaTypes: ['剧集'], statuses: ['在看'], regions: ['CN'], lock: 'locked',
    }));
  });
});

describe('saved watchlist views', () => {
  const view = {
    id: 'view-1', name: '高分电影', query: normalizeWatchlistQuery({ ...EMPTY_WATCHLIST_QUERY, rating: { min: 8, max: null } }),
    sortBy: 'rating', viewMode: 'poster', createdAt: '2026-08-09T00:00:00.000Z', updatedAt: '2026-08-09T00:00:00.000Z',
  };

  test('round-trips the versioned local file', () => {
    assert.deepEqual(parseSavedViews(serializeSavedViews([view])), [view]);
    assert.deepEqual(parseSavedViews(null), []);
  });

  test('rejects future roots and malformed views instead of broadening them', () => {
    assert.throws(() => parseSavedViews('{"schemaVersion":2,"views":[]}'), /unsupported_saved_views_schema/);
    assert.throws(() => parseSavedViews('{"schemaVersion":1,"views":[{"id":"bad"}]}'), /invalid_saved_view/);
    assert.throws(() => parseSavedViews(JSON.stringify({ schemaVersion: 1, views: [{ ...view, query: { ...view.query, schemaVersion: 2 } }] })), /invalid_saved_view/);
  });

  test('validates names and the product limit', () => {
    assert.equal(validateSavedViewName(' 高分电影 ', [view]), '已存在同名视图。');
    assert.equal(validateSavedViewName('', [view]), '请输入视图名称。');
    assert.equal(MAX_SAVED_VIEWS, 20);
  });
});
