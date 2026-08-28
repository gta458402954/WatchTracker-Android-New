import assert from 'node:assert/strict';
import { describe, test } from 'node:test';
import { applyMobileLibraryQuery, canonicalRecordForm, DEFAULT_MOBILE_LIBRARY_PREFERENCES, isRecordFormDirty, normalizeMobilePreferences, sortMobileRecords } from '../../../features/watchlist/mobileLibraryModel.ts';

function record(id, overrides = {}) {
  return { id, originalName: id, chineseName: id, progress: '', totalEpisodes: null, movieProgress: null, movieDuration: null, releaseYear: null, posterPath: null, status: '未看', platform: '', rating: null, startDate: '', endDate: '', notes: '', createdAt: id, imdbId: null, mediaType: '电影', ...overrides };
}

describe('M1.2 mobile library model', () => {
  test('normalizes versioned preferences and never stores search text', () => {
    const prefs = normalizeMobilePreferences({ version: 99, searchText: 'secret', viewMode: 'poster', sortBy: 'rating', statuses: ['已看', '已看'], lock: 'locked' });
    assert.deepEqual(prefs, DEFAULT_MOBILE_LIBRARY_PREFERENCES);
  });

  test('ignores a legacy mobile poster mode while preserving valid version-one preferences', () => {
    const prefs = normalizeMobilePreferences({ version: 1, viewMode: 'poster', sortBy: 'rating', mediaTypes: ['剧集'], statuses: ['在看'], lock: 'locked' });
    assert.deepEqual(prefs, { version: 1, sortBy: 'rating', mediaTypes: ['剧集'], statuses: ['在看'], lock: 'locked' });
    assert.equal('viewMode' in prefs, false);
  });

  test('sorts null values last and uses stable deterministic ties', () => {
    const values = [record('b', { rating: 8, createdAt: 'same' }), record('a', { rating: 8, createdAt: 'same' }), record('c', { rating: null, createdAt: 'same' })];
    assert.deepEqual(sortMobileRecords(values, 'rating').map(item => item.id), ['a', 'b', 'c']);
  });

  test('searches all mobile fields and applies basic filters', () => {
    const values = [record('one', { platform: 'Netflix', status: '已看' }), record('two', { notes: '晚点看', status: '未看' })];
    assert.deepEqual(applyMobileLibraryQuery(values, 'netflix', DEFAULT_MOBILE_LIBRARY_PREFERENCES).map(item => item.id), ['one']);
    assert.deepEqual(applyMobileLibraryQuery(values, '', { ...DEFAULT_MOBILE_LIBRARY_PREFERENCES, statuses: ['未看'] }).map(item => item.id), ['two']);
  });

  test('canonical dirty comparison ignores identity and metadata', () => {
    const initial = record('one', { rev: 1, updatedAt: 'a', createdAt: 'a' });
    assert.equal(isRecordFormDirty(initial, { ...initial, rev: 2, updatedAt: 'b' }), false);
    assert.equal(isRecordFormDirty(initial, { ...initial, notes: 'changed' }), true);
    assert.equal(canonicalRecordForm(initial), canonicalRecordForm({ ...initial, rev: 8 }));
  });

  test('derives a 1000-record query within the mobile budget', () => {
    const values = Array.from({ length: 1000 }, (_, index) => record(`record-${index}`, { platform: index % 2 ? 'Netflix' : '本地' }));
    const started = performance.now();
    const result = applyMobileLibraryQuery(values, 'record-99', DEFAULT_MOBILE_LIBRARY_PREFERENCES);
    const elapsed = performance.now() - started;
    assert.equal(result.length, 11);
    assert.ok(elapsed < 200, `derived query took ${elapsed.toFixed(2)}ms`);
  });
});
