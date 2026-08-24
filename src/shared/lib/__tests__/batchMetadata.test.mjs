import assert from 'node:assert/strict';
import { describe, test } from 'node:test';

import {
  buildBatchMetadataPatch,
  isBatchMetadataCandidate,
  missingBatchMetadataFields,
  noDataFieldsForRecord,
  parseBatchMetadataNoDataState,
  pruneBatchMetadataNoDataState,
  recordNoDataFields,
  retainMissingMetadataPatch,
  remoteIdentityKey,
  seasonNumberOf,
  selectBatchMetadataPatch,
  selectTmdbMatch,
  tmdbTypeHintOf,
} from '../batchMetadata.ts';

function record(overrides = {}) {
  return {
    id: 'record-1', originalName: 'Example', chineseName: '示例', progress: '',
    totalEpisodes: null, movieProgress: null, movieDuration: null, releaseYear: null,
    posterPath: null, status: '未看', platform: '', rating: null, startDate: '', endDate: '',
    notes: '', createdAt: '2025-01-01T00:00:00Z', imdbId: 'tt123', mediaType: '电影',
    contentTags: null, originCountry: null, genres: null, imdbRating: null, tmdbStatus: null,
    episodeRuntime: null, ...overrides,
  };
}

describe('batch metadata identity', () => {
  test('recognizes movie, episodic, special-type and season hints without guessing ambiguous specials', () => {
    assert.equal(tmdbTypeHintOf(record()), 'movie');
    assert.equal(tmdbTypeHintOf(record({ mediaType: '综艺' })), 'tv');
    assert.equal(tmdbTypeHintOf(record({ mediaType: '动画', totalEpisodes: 12 })), 'tv');
    assert.equal(tmdbTypeHintOf(record({ mediaType: '纪录片', movieDuration: 5400 })), 'movie');
    assert.equal(tmdbTypeHintOf(record({ mediaType: '动画' })), null);
  });

  test('extracts a stable season number and includes it in the remote identity', () => {
    const season = record({ originalName: 'Example Season 3', mediaType: '剧集' });
    assert.equal(seasonNumberOf(season), 3);
    assert.equal(remoteIdentityKey({ id: 77, type: 'tv' }, 3), 'tv:77:season-3');
    assert.equal(remoteIdentityKey({ id: 77, type: 'tv' }, null), 'tv:77:series');
  });

  test('recognizes legacy Chinese-number season names and rejects conflicting title fields', () => {
    assert.equal(seasonNumberOf(record({ chineseName: '示例剧 第五季', originalName: '', mediaType: '剧集' })), 5);
    assert.equal(seasonNumberOf(record({ chineseName: '示例剧 第五季', originalName: 'Example Season 6', mediaType: '剧集' })), null);
  });

  test('never falls back to a mismatched type and rejects ambiguous special media', () => {
    const movie = record();
    assert.deepEqual(selectTmdbMatch(movie, [{ id: 9, media_type: 'tv' }]), {
      ok: false, reason: 'TMDB 没有返回匹配的电影结果',
    });
    const animation = record({ mediaType: '动画' });
    const ambiguous = selectTmdbMatch(animation, [
      { id: 10, media_type: 'movie' }, { id: 11, media_type: 'tv' },
    ]);
    assert.equal(ambiguous.ok, false);
    assert.deepEqual(ambiguous.candidates, [{ id: 10, type: 'movie' }, { id: 11, type: 'tv' }]);
  });

  test('requires a user choice instead of silently taking the first same-type match', () => {
    const result = selectTmdbMatch(record(), [
      { id: 20, media_type: 'movie' }, { id: 21, media_type: 'movie' },
    ]);
    assert.equal(result.ok, false);
    assert.deepEqual(result.candidates, [{ id: 20, type: 'movie' }, { id: 21, type: 'movie' }]);
  });
});

describe('field-level safe patch', () => {
  test('fills movie fields only and never overwrites existing metadata', () => {
    const original = record({
      genres: 'Local genre', originCountry: 'JP', contentTags: '自定义', imdbRating: 8.8,
      tmdbStatus: 'Local status', mediaType: '电影', platform: '用户平台',
    });
    const patch = buildBatchMetadataPatch(original, {
      runtime: 120, vote_average: 6.5, status: 'Released', genres: [{ name: 'Drama' }],
      production_countries: [{ iso_3166_1: 'US' }], number_of_episodes: 99,
      episode_run_time: [45], production_companies: [{ name: 'Remote Platform' }],
    }, 'movie');

    assert.deepEqual(patch.updates, { movieDuration: 7200 });
    assert.deepEqual(patch.fields, ['movieDuration']);
    assert.equal('mediaType' in patch.updates, false);
    assert.equal('episodeRuntime' in patch.updates, false);
    assert.equal('totalEpisodes' in patch.updates, false);
  });

  test('fills a specific TV season with series runtime and season episode count', () => {
    const season = record({ originalName: 'Example Season 2', mediaType: '动画' });
    const patch = buildBatchMetadataPatch(season, {
      vote_average: 8.1, status: 'Returning Series', genres: [{ name: 'Animation' }],
      origin_country: ['JP', 'US'], episode_run_time: [24], number_of_episodes: 50,
      seasons: [{ season_number: 1, episode_count: 12 }, { season_number: 2, episode_count: 13 }],
    }, 'tv');

    assert.deepEqual(patch.updates, {
      genres: 'Animation', originCountry: 'JP, US', contentTags: '日本,美国', imdbRating: 8.1,
      tmdbStatus: 'Returning Series', episodeRuntime: 24, totalEpisodes: 13,
    });
    assert.equal(patch.seasonNumber, 2);
    assert.equal('movieDuration' in patch.updates, false);
    assert.equal('mediaType' in patch.updates, false);
  });

  test('formats missing season names through the shared season-title formatter', () => {
    const season = record({ originalName: 'Example Season 2', chineseName: '', mediaType: '剧集' });
    const patch = buildBatchMetadataPatch(season, {
      id: 46511, name: '示例剧', original_name: 'Example',
      seasons: [{ id: 62090, season_number: 2, name: '示例剧：台湾篇', episode_count: 13 }],
    }, 'tv');

    assert.equal(patch.updates.chineseName, '示例剧 第 2 季：台湾篇');
    assert.equal(patch.updates.originalName, undefined);
  });

  test('does not write a localized season subtitle into the missing original name', () => {
    const season = record({
      originalName: '', chineseName: '', mediaType: '剧集', tmdbSeasonNumber: 2,
    });
    const patch = buildBatchMetadataPatch(season, {
      id: 46511, name: '示例剧', original_name: 'Example',
      seasons: [{ id: 62090, season_number: 2, name: '台湾篇', episode_count: 13 }],
    }, 'tv');

    assert.equal(patch.updates.chineseName, '示例剧 第 2 季：台湾篇');
    assert.equal(patch.updates.originalName, 'Example Season 2');
  });

  test('does not turn remote nulls or invalid numbers into destructive updates', () => {
    const patch = buildBatchMetadataPatch(record(), {
      runtime: 0, vote_average: 0, status: ' ', genres: [], production_countries: [],
    }, 'movie');
    assert.deepEqual(patch.updates, {});
  });

  test('does not infer a platform for mainland-China metadata', () => {
    const patch = buildBatchMetadataPatch(record(), {
      production_countries: [{ iso_3166_1: 'CN' }],
      production_companies: [{ name: 'Tencent Video' }],
    }, 'movie');

    assert.equal('platform' in patch.updates, false);
    assert.equal(patch.updates.originCountry, 'CN');
  });

  test('fills every supported missing field supplied by TMDB', () => {
    const patch = buildBatchMetadataPatch(record({ chineseName: '', originalName: '' }), {
      title: '中文名', original_title: 'Original', release_date: '2024-06-01', poster_path: '/poster.jpg',
      production_companies: [{ name: 'Apple Tv' }], runtime: 101, vote_average: 7.4,
      status: 'Released', genres: [{ name: 'Drama' }], production_countries: [{ iso_3166_1: 'US' }],
    }, 'movie');
    assert.deepEqual(patch.updates, {
      chineseName: '中文名', originalName: 'Original', releaseYear: '2024', posterPath: '/poster.jpg',
      platform: 'Apple TV+', genres: 'Drama', originCountry: 'US', contentTags: '美国',
      imdbRating: 7.4, tmdbStatus: 'Released', movieDuration: 6060,
    });
  });

  test('revalidates missing fields at write time so preview cannot overwrite a later edit', () => {
    const current = record({ genres: '用户刚刚填写', movieDuration: null });
    const retained = retainMissingMetadataPatch(current, { genres: 'Remote', movieDuration: 7200 });
    assert.deepEqual(retained.updates, { movieDuration: 7200 });
  });

  test('plans missing movie TMDB identity without mixing it into ordinary overwrites', () => {
    const patch = buildBatchMetadataPatch(record(), { id: 101, runtime: 90 }, 'movie');
    assert.deepEqual(patch.identityPlan, {
      tmdbMediaKind: 'movie', tmdbId: 101, tmdbParentId: null,
      tmdbSeasonNumber: null, seriesRecordKind: 'single-work',
    });
    assert.deepEqual(patch.updates, {
      movieDuration: 5400, tmdbMediaKind: 'movie', tmdbId: 101, seriesRecordKind: 'single-work',
    });
  });

  test('plans a concrete TV season identity from the parent and season detail ids', () => {
    const patch = buildBatchMetadataPatch(record({ originalName: 'Example Season 2', mediaType: '剧集' }), {
      id: 46511, seasons: [{ id: 62090, season_number: 2, episode_count: 13 }],
    }, 'tv');
    assert.deepEqual(patch.identityPlan, {
      tmdbMediaKind: 'tv-season', tmdbId: 62090, tmdbParentId: 46511,
      tmdbSeasonNumber: 2, seriesRecordKind: 'season',
    });
    assert.equal(patch.updates.tmdbParentId, 46511);
    assert.equal(patch.updates.tmdbSeasonNumber, 2);
  });

  test('reports an existing identity conflict instead of replacing it', () => {
    const patch = buildBatchMetadataPatch(record({ tmdbMediaKind: 'movie', tmdbId: 999 }), {
      id: 101, runtime: 90,
    }, 'movie');
    assert.equal(patch.identityPlan, undefined);
    assert.match(patch.identityConflict, /TMDB ID/);
    assert.deepEqual(patch.updates, { movieDuration: 5400 });
  });

  test('candidate selection excludes locked, unidentified and already complete records', () => {
    assert.equal(isBatchMetadataCandidate(record({ isLocked: true })), false);
    assert.equal(isBatchMetadataCandidate(record({ imdbId: null })), false);
    assert.equal(isBatchMetadataCandidate(record({
      releaseYear: '2024', posterPath: '/poster.jpg', platform: 'Netflix',
      genres: 'Drama', originCountry: 'US', contentTags: '美国', imdbRating: 8,
      tmdbStatus: 'Released', movieDuration: 6000, tmdbMediaKind: 'movie',
      tmdbId: 9, seriesRecordKind: 'single-work',
    })), false);
  });
});

describe('persistent TMDB no-data state', () => {
  test('skips remembered fields for the same IMDb id and invalidates them when IMDb changes', () => {
    const item = record();
    const state = recordNoDataFields(parseBatchMetadataNoDataState(null), item, missingBatchMetadataFields(item));
    const remembered = noDataFieldsForRecord(state, item);
    assert.equal(isBatchMetadataCandidate(item, remembered), false);
    assert.deepEqual([...noDataFieldsForRecord(state, record({ imdbId: 'tt999' }))], []);
    assert.equal(isBatchMetadataCandidate(record({ imdbId: 'tt999' }), noDataFieldsForRecord(state, record({ imdbId: 'tt999' }))), true);
  });

  test('parses corrupt state safely and prunes deleted or changed records', () => {
    assert.deepEqual(parseBatchMetadataNoDataState('{broken'), { version: 1, records: {} });
    const first = record();
    const state = recordNoDataFields(parseBatchMetadataNoDataState(null), first, ['posterPath'], '2026-01-01T00:00:00Z');
    assert.deepEqual(pruneBatchMetadataNoDataState(state, [record({ imdbId: 'tt999' })]), { version: 1, records: {} });
  });

  test('filters remembered no-data fields out of an otherwise valid patch', () => {
    const patch = buildBatchMetadataPatch(record(), {
      release_date: '2024-01-01', poster_path: '/poster.jpg', runtime: 90,
    }, 'movie');
    const selected = selectBatchMetadataPatch(patch, new Set(['releaseYear', 'movieDuration']));
    assert.deepEqual(selected.updates, { releaseYear: '2024', movieDuration: 5400 });
  });
});
