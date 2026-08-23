import assert from 'node:assert/strict';
import { describe, test } from 'node:test';

import {
  buildDiscoveryQueue,
  discoveryEmptyMessage,
  discoveryFilterOptions,
  estimateDiscoveryViewing,
  isDiscoveryCompletedProduction,
  scoreDiscoveryRecord,
} from '../discovery.ts';

function record(id, overrides = {}) {
  return {
    id,
    originalName: id,
    chineseName: id,
    progress: '',
    totalEpisodes: null,
    movieProgress: null,
    movieDuration: 90 * 60,
    releaseYear: '2026',
    posterPath: null,
    status: '未看',
    platform: '',
    rating: null,
    startDate: '',
    endDate: '',
    notes: '',
    createdAt: '2026-08-01T00:00:00.000Z',
    imdbId: null,
    mediaType: '电影',
    interestLevel: 3,
    imdbRating: 6,
    ...overrides,
  };
}

const defaults = { durationLimit: 0, mediaType: '全部', platform: null, endedOnly: false };

describe('TASK-D-DISCOVERY-001 candidate eligibility and viewing time', () => {
  test('includes locked unwatched records but excludes watching and watched records', () => {
    const locked = record('locked', { isLocked: true });
    const result = buildDiscoveryQueue([
      locked,
      record('watching', { status: '在看' }),
      record('watched', { status: '已看' }),
    ], defaults);
    assert.deepEqual(result.candidates.map(item => item.record.id), ['locked']);
  });

  test('uses a whole movie or one episode and marks conservative defaults', () => {
    assert.deepEqual(estimateDiscoveryViewing(record('movie', { movieDuration: 100 * 60 })), {
      minutes: 100, episodic: false, estimated: false,
    });
    assert.deepEqual(estimateDiscoveryViewing(record('series', { mediaType: '剧集', totalEpisodes: 10, episodeRuntime: 52 })), {
      minutes: 52, episodic: true, estimated: false,
    });
    assert.deepEqual(estimateDiscoveryViewing(record('unknown-series', { mediaType: '纪录片', totalEpisodes: 4, episodeRuntime: null })), {
      minutes: 45, episodic: true, estimated: true,
    });
    assert.equal(estimateDiscoveryViewing(record('unknown-movie', { movieDuration: null })).minutes, 120);
  });

  test('treats single works, Ended, and Miniseries as completed productions', () => {
    assert.equal(isDiscoveryCompletedProduction(record('movie')), true);
    assert.equal(isDiscoveryCompletedProduction(record('ended', { mediaType: '剧集', totalEpisodes: 8, tmdbStatus: 'Ended' })), true);
    assert.equal(isDiscoveryCompletedProduction(record('mini', { mediaType: '剧集', totalEpisodes: 6, tmdbStatus: 'Miniseries' })), true);
    assert.equal(isDiscoveryCompletedProduction(record('returning', { mediaType: '剧集', totalEpisodes: 8, tmdbStatus: 'Returning Series' })), false);
  });
});

describe('TASK-D-DISCOVERY-001 explainable stable scoring', () => {
  test('makes interest the largest component and returns bounded reasons', () => {
    const favorite = record('favorite', { status: '已看', rating: 9, genres: '科幻,悬疑', platform: 'Netflix' });
    const candidate = record('candidate', { interestLevel: 5, imdbRating: 8.5, genres: '科幻,悬疑,剧情', platform: 'Netflix' });
    const scored = scoreDiscoveryRecord(candidate, [favorite, candidate]);
    assert.deepEqual(scored.breakdown, { interest: 50, imdb: 26, completion: 8, genres: 8, platform: 4 });
    assert.equal(scored.score, 96);
    assert.equal(scored.reasons.length, 3);
    assert.equal(scored.reasons[0], '必看神作');
  });

  test('uses deterministic title and id tie-breakers', () => {
    const result = buildDiscoveryQueue([
      record('z-id', { chineseName: '乙' }),
      record('b-id', { chineseName: '甲' }),
      record('a-id', { chineseName: '甲' }),
    ], defaults);
    assert.deepEqual(result.candidates.map(item => item.record.id), ['z-id', 'a-id', 'b-id']);
  });
});

describe('TASK-D-DISCOVERY-001 filters and empty reasons', () => {
  const records = [
    record('movie', { platform: 'Netflix', movieDuration: 95 * 60 }),
    record('series', { mediaType: '剧集', totalEpisodes: 8, episodeRuntime: 42, platform: 'Apple TV+', tmdbStatus: 'Returning Series' }),
    record('mini', { mediaType: '剧集', totalEpisodes: 6, episodeRuntime: 55, platform: 'Netflix', tmdbStatus: 'Miniseries' }),
  ];

  test('combines media, platform, completion and duration filters', () => {
    const result = buildDiscoveryQueue(records, {
      durationLimit: 60, mediaType: '剧集', platform: 'Netflix', endedOnly: true,
    });
    assert.deepEqual(result.candidates.map(item => item.record.id), ['mini']);
  });

  test('returns stable dynamic filter options and precise empty messages', () => {
    assert.deepEqual(discoveryFilterOptions(records), {
      mediaTypes: ['电影', '剧集'], platforms: ['Apple TV+', 'Netflix'],
    });
    const result = buildDiscoveryQueue(records, { ...defaults, durationLimit: 30 });
    assert.equal(result.emptyReason, 'duration');
    assert.equal(discoveryEmptyMessage(result.emptyReason), '没有符合当前时长的作品');
  });

  test('keeps explicit skips out across rebuilt queues', () => {
    const result = buildDiscoveryQueue([record('only')], defaults, new Set(['only']));
    assert.equal(result.emptyReason, 'skipped');
  });
});
