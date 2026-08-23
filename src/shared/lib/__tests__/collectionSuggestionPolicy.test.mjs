import test from 'node:test';
import assert from 'node:assert/strict';
import {
  parseSuggestionDismissals,
  serializeSuggestionDismissals,
  suggestionIsCovered,
  tvSuggestionEligibility,
  upsertSuggestionDismissal,
} from '../../../features/collections/lib/collectionSuggestionPolicy.ts';

const record = (id, overrides = {}) => ({
  id,
  chineseName: '',
  originalName: '',
  progress: '',
  tmdbParentId: null,
  tmdbSeasonNumber: null,
  ...overrides,
});

test('single aired regular season already represented locally is complete', () => {
  const detail = { seasons: [
    { season_number: 0, air_date: '2020-01-01' },
    { season_number: 1, air_date: '2021-01-01' },
    { season_number: 2, air_date: '2099-01-01' },
  ] };
  assert.equal(tvSuggestionEligibility(detail, 100, ['r1'], [record('r1')], new Date('2026-01-01T00:00:00Z')), 'complete');
});

test('an aired missing season remains actionable', () => {
  const detail = { seasons: [
    { season_number: 1, air_date: '2021-01-01' },
    { season_number: 2, air_date: '2022-01-01' },
  ] };
  assert.equal(tvSuggestionEligibility(detail, 100, ['r1'], [record('r1', { tmdbParentId: 100, tmdbSeasonNumber: 1 })], new Date('2026-01-01T00:00:00Z')), 'actionable');
});

test('missing or unusable TMDB season data is never treated as complete', () => {
  assert.equal(tvSuggestionEligibility({}, 100, ['r1'], [record('r1')]), 'unknown');
  assert.equal(tvSuggestionEligibility({ seasons: [{ season_number: 0, air_date: '2020-01-01' }] }, 100, ['r1'], [record('r1')]), 'unknown');
});

test('a single candidate already in any collection is covered regardless of collection name', () => {
  const members = [{ id: 'm1', collectionId: 'blacklist', recordId: 'redemption' }];
  assert.equal(suggestionIsCovered(['redemption'], members), true);
});

test('multiple candidates are covered only when one collection contains all of them', () => {
  const split = [
    { id: 'm1', collectionId: 'a', recordId: 's1' },
    { id: 'm2', collectionId: 'b', recordId: 's2' },
  ];
  const together = [...split, { id: 'm3', collectionId: 'a', recordId: 's2' }];
  assert.equal(suggestionIsCovered(['s1', 's2'], split), false);
  assert.equal(suggestionIsCovered(['s1', 's2'], together), true);
});

test('dismissals serialize safely, deduplicate and survive malformed settings', () => {
  const first = { key: 'tmdb:tv-show:1', name: '示例', sourceKind: 'tmdb-tv-show', dismissedAt: '2026-01-01T00:00:00.000Z' };
  const updated = { ...first, name: '新名称', dismissedAt: '2026-02-01T00:00:00.000Z' };
  const entries = upsertSuggestionDismissal(upsertSuggestionDismissal([], first), updated);
  assert.equal(entries.length, 1);
  assert.deepEqual(parseSuggestionDismissals(serializeSuggestionDismissals(entries)), [updated]);
  assert.deepEqual(parseSuggestionDismissals('{broken'), []);
  assert.deepEqual(parseSuggestionDismissals(JSON.stringify({ version: 2, entries })), []);
});
