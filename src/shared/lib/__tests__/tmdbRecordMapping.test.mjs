import test from 'node:test';
import assert from 'node:assert/strict';
import { movieRecordMetadata, seasonRecordMetadata, tmdbOriginalLanguageLocale } from '../../../features/collections/lib/tmdbRecordMapping.ts';

const blackMirror = {
  id: 42009,
  name: '黑镜',
  original_name: 'Black Mirror',
  external_ids: { imdb_id: 'tt2085059' },
  origin_country: ['GB'],
  genres: [{ name: 'Sci-Fi & Fantasy' }, { name: '剧情' }],
  networks: [{ name: 'Channel 4' }],
  episode_run_time: [60],
  vote_average: 8.288,
  status: 'Returning Series',
};

test('missing-season creation inherits complete series metadata', () => {
  const result = seasonRecordMetadata(blackMirror, {
    id: 63871, season_number: 3, name: '第 3 季', air_date: '2016-10-21', episode_count: 6,
    poster_path: '/black-mirror-s3.jpg',
  });
  assert.equal(result.chineseName, '黑镜 第 3 季');
  assert.equal(result.originCountry, 'GB');
  assert.equal(result.genres, 'Sci-Fi & Fantasy,剧情');
  assert.equal(result.contentTags, '英国');
  assert.equal(result.platform, 'Channel 4');
  assert.equal(result.episodeRuntime, 60);
  assert.equal(result.tmdbParentId, 42009);
  assert.equal(result.tmdbSeasonNumber, 3);
});

test('Jet Lag reality seasons preserve their specific TMDB season title', () => {
  const result = seasonRecordMetadata({
    id: 258321,
    name: 'Jet Lag: The Game',
    original_name: 'Jet Lag: The Game',
    original_language: 'en',
    genres: [{ name: 'Reality' }],
    origin_country: ['US'],
  }, {
    id: 383896,
    season_number: 1,
    name: '第 1 季',
  }, undefined, {
    id: 383896,
    season_number: 1,
    name: 'Connect 4 Across America',
  });

  assert.equal(result.mediaType, '综艺');
  assert.equal(result.chineseName, 'Jet Lag: The Game：Connect 4 Across America');
  assert.equal(result.originalName, 'Jet Lag: The Game: Connect 4 Across America');
  assert.equal(tmdbOriginalLanguageLocale('en'), 'en-US');
});

test('generic TMDB season names keep the established numbered-title fallback', () => {
  const result = seasonRecordMetadata(blackMirror, {
    id: 63871, season_number: 3, name: '第 3 季',
  }, undefined, {
    id: 63871, season_number: 3, name: 'Season 3',
  });

  assert.equal(result.chineseName, '黑镜 第 3 季');
  assert.equal(result.originalName, 'Black Mirror Season 3');
});

test('metadata completion does not replace existing manually maintained fields', () => {
  const result = seasonRecordMetadata(blackMirror, { id: 63871, season_number: 3 }, {
    platform: 'Netflix', originCountry: 'US', genres: '自定义', contentTags: '自定义标签', episodeRuntime: 42,
  });
  assert.equal(result.platform, undefined);
  assert.equal(result.originCountry, undefined);
  assert.equal(result.genres, undefined);
  assert.equal(result.episodeRuntime, undefined);
  assert.equal(result.contentTags, '自定义标签,英国');
});

test('movie collection parts become stable movie records with complete detail metadata', () => {
  const result = movieRecordMetadata({
    id: 603, title: '黑客帝国', original_title: 'The Matrix', release_date: '1999-03-30',
    poster_path: '/matrix.jpg', external_ids: { imdb_id: 'tt0133093' }, runtime: 136,
    production_countries: [{ iso_3166_1: 'US' }], genres: [{ name: '科幻' }], vote_average: 8.2,
  });
  assert.equal(result.tmdbMediaKind, 'movie');
  assert.equal(result.tmdbId, 603);
  assert.equal(result.imdbId, 'tt0133093');
  assert.equal(result.movieDuration, 8160);
  assert.equal(result.originCountry, 'US');
});
