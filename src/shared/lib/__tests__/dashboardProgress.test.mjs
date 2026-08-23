import assert from 'node:assert/strict';
import { describe, test } from 'node:test';

import { dashboardWatchingProgress } from '../dashboardProgress.ts';

function record(overrides = {}) {
  return {
    id: 'progress',
    originalName: 'Progress',
    chineseName: '进度',
    progress: '',
    totalEpisodes: null,
    episodeTrackingEnabled: false,
    nextEpisode: null,
    movieProgress: null,
    movieDuration: null,
    releaseYear: null,
    posterPath: null,
    status: '在看',
    platform: '',
    rating: null,
    startDate: '',
    endDate: '',
    notes: '',
    createdAt: '2026-08-05',
    imdbId: null,
    mediaType: '电影',
    ...overrides,
  };
}

describe('dashboard watching progress', () => {
  test('shows readable movie progress, duration, and bounded percentage', () => {
    assert.equal(dashboardWatchingProgress(record({ movieProgress: 4020, movieDuration: 9600 })),
      '已观看 1 小时 7 分钟 / 2 小时 40 分钟 · 42%');
    assert.equal(dashboardWatchingProgress(record({ movieProgress: 200, movieDuration: 100 })),
      '已观看 3 分钟 / 2 分钟 · 100%');
  });

  test('shows elapsed movie time when total duration is unavailable', () => {
    assert.equal(dashboardWatchingProgress(record({ movieProgress: 2700 })), '已观看 45 分钟');
    assert.equal(dashboardWatchingProgress(record()), '尚未记录进度');
  });

  test('uses next-episode semantics for tracked series and legacy progress otherwise', () => {
    assert.equal(dashboardWatchingProgress(record({
      mediaType: '剧集', totalEpisodes: 12, episodeTrackingEnabled: true, nextEpisode: 4,
    })), '下一集：第 4 集');
    assert.equal(dashboardWatchingProgress(record({
      mediaType: '剧集', totalEpisodes: 12, progress: '第3集',
    })), '第3集');
  });
});
