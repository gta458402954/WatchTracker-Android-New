import assert from 'node:assert/strict';
import { describe, test } from 'node:test';

import {
  effectiveRegionOf,
  filterRecords,
  recordsInRegionScope,
  regionOptionsForScope,
} from '../filtering.ts';
import { UNKNOWN_REGION_CODE } from '../countryNames.ts';

function record(id, overrides = {}) {
  return {
    id,
    originalName: id,
    chineseName: id,
    progress: '',
    totalEpisodes: null,
    movieProgress: null,
    movieDuration: null,
    releaseYear: null,
    posterPath: null,
    status: '未看',
    platform: '',
    rating: null,
    startDate: '',
    endDate: '',
    notes: '',
    createdAt: id,
    imdbId: null,
    mediaType: '电影',
    contentTags: null,
    originCountry: null,
    ...overrides,
  };
}

const records = [
  record('cn-watching', { status: '在看', originCountry: 'CN', isLocked: true }),
  record('hk-seen', { status: '已看', mediaType: '剧集', originCountry: 'HK, GB' }),
  record('tw-unseen', { originCountry: 'TW' }),
  record('uk-alias', { originCountry: 'UK' }),
  record('multi', { originCountry: 'FR, DE' }),
  record('unknown', { contentTags: '悬疑' }),
  record('unmapped', { originCountry: 'XX' }),
];

describe('region scope and dynamic options', () => {
  test('uses only media type and status to define the aggregation scope', () => {
    assert.deepEqual(recordsInRegionScope(records, '剧集', '已看').map(item => item.id), ['hk-seen']);
    assert.deepEqual(regionOptionsForScope(records, '剧集', '已看'), [
      { code: 'HK', label: '中国香港', count: 1 },
    ]);
  });

  test('keeps option counts independent of search, lock, sort, and active region', () => {
    const before = regionOptionsForScope(records, 'all', 'all');
    const filtered = filterRecords(records, {
      mediaType: 'all',
      status: 'all',
      region: 'CN',
      searchText: 'cn-watching',
      lock: 'locked',
    });

    assert.deepEqual(filtered.map(item => item.id), ['cn-watching']);
    assert.deepEqual(regionOptionsForScope([...records].reverse(), 'all', 'all'), before);
  });

  test('keeps all aggregated options, including unknown and unmapped regions', () => {
    const options = regionOptionsForScope(records, 'all', 'all');
    assert.deepEqual(options.map(option => option.code), [
      'CN', 'HK', 'TW', 'GB', 'FR', 'XX', UNKNOWN_REGION_CODE,
    ]);
    assert.deepEqual(options.find(option => option.code === UNKNOWN_REGION_CODE), {
      code: UNKNOWN_REGION_CODE,
      label: '未知地区',
      count: 1,
    });
    assert.equal(options.find(option => option.code === 'XX')?.label, 'XX');
  });

  test('excludes the second and later countries of one record from options and matching', () => {
    const longCountryRecord = record('many-countries', { originCountry: 'US, CN, GB, BY, FR' });
    assert.deepEqual(
      regionOptionsForScope([longCountryRecord], 'all', 'all').map(option => option.code),
      ['US'],
    );

    const base = { mediaType: 'all', status: 'all', searchText: '', lock: 'all' };
    assert.deepEqual(filterRecords([longCountryRecord], { ...base, region: 'US' }).map(item => item.id), ['many-countries']);
    assert.deepEqual(filterRecords([longCountryRecord], { ...base, region: 'CN' }), []);
    assert.deepEqual(filterRecords([longCountryRecord], { ...base, region: 'GB' }), []);
    assert.deepEqual(filterRecords([longCountryRecord], { ...base, region: 'BY' }), []);
  });
});

describe('combined record filters', () => {
  test('combines media type, status, region, search, and lock predicates', () => {
    const matched = filterRecords(records, {
      mediaType: '电影',
      status: '在看',
      region: 'CN',
      searchText: 'WATCHING',
      lock: 'locked',
    });
    assert.deepEqual(matched.map(item => item.id), ['cn-watching']);
  });

  test('supports GB normalized from UK, unknown, and unmapped region selection', () => {
    const base = { mediaType: 'all', status: 'all', searchText: '', lock: 'all' };
    assert.deepEqual(filterRecords(records, { ...base, region: 'GB' }).map(item => item.id), ['uk-alias']);
    assert.deepEqual(filterRecords(records, { ...base, region: UNKNOWN_REGION_CODE }).map(item => item.id), ['unknown']);
    assert.deepEqual(filterRecords(records, { ...base, region: 'XX' }).map(item => item.id), ['unmapped']);
  });
});

describe('invalid region fallback', () => {
  test('uses all immediately and remains all when the old option later returns', () => {
    const initial = regionOptionsForScope(records, 'all', 'all');
    assert.equal(effectiveRegionOf('CN', initial), 'CN');

    const narrowed = regionOptionsForScope(records, 'all', '已看');
    const effective = effectiveRegionOf('CN', narrowed);
    assert.equal(effective, 'all');
    assert.deepEqual(
      filterRecords(records, {
        mediaType: 'all', status: '已看', region: effective, searchText: '', lock: 'all',
      }).map(item => item.id),
      ['hk-seen'],
    );

    const clearedState = effective;
    assert.equal(effectiveRegionOf(clearedState, initial), 'all');
  });
});
