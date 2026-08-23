import assert from 'node:assert/strict';
import test from 'node:test';
import {
  formatMovieTime,
  initialRecordFormValues,
  mediaTypeChange,
  parseTimeToSeconds,
  smartProgress,
} from '../../../features/watchlist/record-form/recordFormModel.ts';

const base = {
  id: 'r1', originalName: 'Original', chineseName: '中文', progress: '第2集', totalEpisodes: 10,
  movieProgress: 90, movieDuration: 7200, releaseYear: '2026', posterPath: '/poster', status: '已看',
  platform: 'Netflix', rating: 8, startDate: '2026-01-01', endDate: '', notes: 'note', createdAt: 'now',
  imdbId: 'tt1', genres: 'Drama', originCountry: 'US', imdbRating: 8.2, tmdbStatus: 'Released',
  interestLevel: null, episodeRuntime: 45, mediaType: '剧集', contentTags: '美国', tmdbMediaKind: 'tv',
  tmdbId: 1, tmdbParentId: null, tmdbSeasonNumber: null, seriesRecordKind: 'whole-series',
};

test('record form model preserves new and edit initial values', () => {
  const empty = initialRecordFormValues();
  assert.equal(empty.mediaType, '电影');
  assert.equal(empty.status, '未看');
  assert.equal(empty.movieProgress, null);
  const edit = initialRecordFormValues(base);
  assert.equal(edit.chineseName, '中文');
  assert.equal(edit.mediaType, '剧集');
  assert.equal(edit.tmdbId, 1);
});

test('media switching clears the same fields as the component', () => {
  const form = initialRecordFormValues(base);
  const movie = mediaTypeChange(form, '电影');
  assert.equal(movie.episodic, false);
  assert.equal(movie.form.progress, '');
  assert.equal(movie.form.totalEpisodes, null);
  const episodic = mediaTypeChange(movie.form, '剧集');
  assert.equal(episodic.episodic, true);
  assert.equal(episodic.form.movieProgress, null);
  assert.equal(episodic.form.movieDuration, null);
});

test('progress and movie time conversions preserve accepted inputs', () => {
  assert.equal(smartProgress('2'), '第2集');
  assert.equal(smartProgress('s1e02'), 'S1E02');
  assert.equal(smartProgress('wan'), '完结');
  assert.equal(smartProgress(' 在看 '), '在看');
  assert.equal(parseTimeToSeconds('1h 30m 45s'), 5445);
  assert.equal(parseTimeToSeconds('1:23:45'), 5025);
  assert.equal(formatMovieTime(5445), '1h 30m 45s');
});
