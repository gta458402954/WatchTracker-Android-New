import assert from 'node:assert/strict';
import { describe, test } from 'node:test';

import {
  aggregateRegions,
  compareRegionOptions,
  classifyTmdb,
  hasCountryCode,
  inferPlatformFromTmdb,
  mergeContentTags,
  normalizeCountryCodes,
  positiveEpisodeRuntimeOf,
  regionCodesOf,
  regionCodesForTopFilter,
  regionsOf,
} from '../classification.ts';
import {
  PREFERRED_COUNTRY_CODES,
  UNKNOWN_REGION_CODE,
  countryCodeOfLabel,
  countryLabelOf,
} from '../countryNames.ts';

describe('country-code normalization (FR-01)', () => {
  test('trims, uppercases, splits both comma forms, deduplicates, and retains valid fallback codes', () => {
    assert.deepEqual(normalizeCountryCodes(' fr, DE，fr, XX '), ['FR', 'DE', 'XX']);
  });

  test('filters placeholders and malformed values before accepting two-letter codes', () => {
    assert.deepEqual(normalizeCountryCodes('N/A, NA, NULL, UNKNOWN, 123, U1, ---'), []);
  });

  test('maps UK to GB before generic two-letter validation', () => {
    assert.deepEqual(normalizeCountryCodes('uk, GB, UK'), ['GB']);
  });

  test('keeps CN, HK, and TW distinct and recognizes legacy Chinese labels', () => {
    assert.deepEqual(normalizeCountryCodes('中国大陆, 香港, TW'), ['CN', 'HK', 'TW']);
    assert.equal(countryCodeOfLabel('英国'), 'GB');
  });
});

describe('TASK-D-DATA-003 exact country and platform inference', () => {
  test('matches mainland China only after normalization without confusing HK, TW, or substrings', () => {
    assert.equal(hasCountryCode('us, cn，GB', 'CN'), true);
    assert.equal(hasCountryCode('中国大陆, 香港, TW', 'cn'), true);
    assert.equal(hasCountryCode('中国香港, 中国台湾', 'CN'), false);
    assert.equal(hasCountryCode('ACN, CND', 'CN'), false);
    assert.equal(hasCountryCode('', 'CN'), false);
  });

  test('suppresses only inferred mainland-China platforms and normalizes known platform names', () => {
    assert.equal(inferPlatformFromTmdb('CN, US', 'Tencent Video'), null);
    assert.equal(inferPlatformFromTmdb('中国大陆', ' iQIYI '), null);
    assert.equal(inferPlatformFromTmdb('HK, TW', ' Apple Tv '), 'Apple TV+');
    assert.equal(inferPlatformFromTmdb('US', 'CBS All Access'), 'CBS');
    assert.equal(inferPlatformFromTmdb('US', ' Netflix '), 'Netflix');
    assert.equal(inferPlatformFromTmdb('US', '  '), null);
  });

  test('returns only a positive episode runtime and never uses zero as a missing-value sentinel', () => {
    assert.equal(positiveEpisodeRuntimeOf({ episode_run_time: [0, -1, 46.4], runtime: 52 }), 46);
    assert.equal(positiveEpisodeRuntimeOf({ episode_run_time: [], runtime: 52 }), 52);
    assert.equal(positiveEpisodeRuntimeOf({ episode_run_time: [0], runtime: 0 }), null);
    assert.equal(positiveEpisodeRuntimeOf({ runtime: Number.NaN }), null);
    assert.equal(positiveEpisodeRuntimeOf({}), null);
  });
});

describe('TASK-B-003 TMDB roundtrip rules', () => {
  test('B003 expected: movie countries reuse normalization and preserve mapped and unmapped codes', () => {
    const result = classifyTmdb({
      production_countries: [
        { iso_3166_1: ' us ' },
        { iso_3166_1: 'uk' },
        { iso_3166_1: 'GB' },
        { iso_3166_1: 'xx' },
        { iso_3166_1: 'N/A' },
        { iso_3166_1: 'CN' },
      ],
      genres: [{ name: 'Drama' }],
    }, false);

    assert.equal(result.originCountry, 'US, GB, XX, CN');
    assert.equal(result.contentTags, '美国,英国,中国大陆');
  });

  test('B003 expected: TV countries retain CN HK TW separately after trim case and deduplication', () => {
    const result = classifyTmdb({
      origin_country: [' cn ', 'hk', 'TW', 'tw', 'UNKNOWN', '12'],
      genres: [{ name: 'Documentary' }],
    }, true, '综艺');

    assert.equal(result.originCountry, 'CN, HK, TW');
    assert.equal(result.contentTags, '中国大陆,中国香港,中国台湾');
    assert.equal(result.mediaType, '纪录片');
  });

  test('B003 expected: tag merging removes only recognized legacy region and system tags', () => {
    assert.equal(
      mergeContentTags('美国, 律政，中国香港, UK, 自定义, 纪录片, 日本料理', '英国,中国台湾'),
      '律政,自定义,日本料理,英国,中国台湾',
    );
  });
});

describe('region source priority and display (FR-02)', () => {
  test('uses recognizable originCountry values instead of conflicting legacy tags', () => {
    assert.deepEqual(regionCodesOf({ originCountry: 'FR, DE', contentTags: '美国' }), ['FR', 'DE']);
  });

  test('falls back to recognized legacy tags when originCountry has no usable value', () => {
    assert.deepEqual(regionCodesOf({ originCountry: 'N/A', contentTags: '美国, 悬疑, 美国' }), ['US']);
  });

  test('returns one non-ISO unknown sentinel when neither source is recognizable', () => {
    assert.deepEqual(regionCodesOf({ originCountry: '', contentTags: '悬疑, AI' }), [UNKNOWN_REGION_CODE]);
    assert.equal(countryLabelOf(UNKNOWN_REGION_CODE), '未知地区');
  });

  test('displays unmapped valid codes as their uppercase code', () => {
    assert.deepEqual(regionCodesOf({ originCountry: 'xx', contentTags: null }), ['XX']);
    assert.equal(countryLabelOf('XX'), 'XX');
  });

  test('TMDB reality television is classified as variety content', () => {
    const result = classifyTmdb({
      origin_country: ['US'],
      genres: [{ name: 'Reality' }],
    }, true, '剧集');

    assert.equal(result.mediaType, '综艺');
  });

  test('an explicitly maintained special type is not silently replaced by reality classification', () => {
    const result = classifyTmdb({ genres: [{ name: 'Reality' }] }, true, '动画');
    assert.equal(result.mediaType, '动画');
  });

  test('displays BY as Belarus in Chinese', () => {
    assert.deepEqual(regionCodesOf({ originCountry: 'BY', contentTags: null }), ['BY']);
    assert.equal(countryLabelOf('BY'), '白俄罗斯');
    assert.equal(countryCodeOfLabel('白俄罗斯'), 'BY');
  });

  test('uses only the first country of each record for the top filter', () => {
    const record = { originCountry: 'US, CN, GB, BY, FR', contentTags: null };
    assert.deepEqual(regionCodesOf(record), ['US', 'CN', 'GB', 'BY', 'FR']);
    assert.deepEqual(regionCodesForTopFilter(record), ['US']);
  });

  test('keeps the existing fixed Chinese buttons working through a thin compatibility wrapper', () => {
    assert.deepEqual(regionsOf({ originCountry: 'US', contentTags: null }), ['美国']);
    assert.deepEqual(regionsOf({ originCountry: 'FR', contentTags: '美国' }), []);
  });
});

describe('region aggregation and stable ordering (FR-04)', () => {
  test('counts each record once using only its first country', () => {
    const options = aggregateRegions([
      { originCountry: 'FR, DE, FR', contentTags: null },
      { originCountry: 'FR', contentTags: null },
    ]);

    assert.deepEqual(options, [
      { code: 'FR', label: '法国', count: 2 },
    ]);
  });

  test('uses the exact preferred order, then count, and always places unknown last', () => {
    const records = PREFERRED_COUNTRY_CODES.map(code => ({ originCountry: code, contentTags: null }));
    records.push(
      { originCountry: 'IT', contentTags: null },
      { originCountry: 'IT', contentTags: null },
      { originCountry: 'FR', contentTags: null },
      { originCountry: null, contentTags: '悬疑' },
    );

    assert.deepEqual(
      aggregateRegions(records).map(option => option.code),
      ['CN', 'HK', 'TW', 'US', 'JP', 'KR', 'GB', 'IT', 'FR', UNKNOWN_REGION_CODE],
    );
  });

  test('uses country code as the final deterministic tie-break', () => {
    const first = { code: 'XB', label: '同名', count: 1 };
    const second = { code: 'XA', label: '同名', count: 1 };
    assert.ok(compareRegionOptions(first, second) > 0);
    assert.ok(compareRegionOptions(second, first) < 0);
  });
});
