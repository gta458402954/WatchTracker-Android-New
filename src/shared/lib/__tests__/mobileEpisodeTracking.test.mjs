import assert from 'node:assert/strict';
import { describe, test } from 'node:test';
import {
  episodeAdjustmentAction,
  isMobileEpisodeTrackable,
  maxEpisodeInHistory,
  mobileEpisodeOptions,
  mobileEpisodeSummary,
  mobileEpisodeWriteReason,
  nextEpisodeAfterCompletion,
  resumableNextEpisode,
} from '../../../features/watchlist/mobileEpisodeTracking.ts';

const record = (fields = {}) => ({
  mediaType: '剧集', totalEpisodes: 4, episodeTrackingEnabled: false,
  nextEpisode: null, status: '未看', ...fields,
});
const completion = episodeNumber => ({
  id: String(episodeNumber), recordId: 'series', episodeNumber,
  completedAt: null, createdAt: '2026-01-01', updatedAt: '2026-01-01', rev: 1, revActor: 'test',
});

describe('mobile episode tracking model', () => {
  test('shows controls only for non-film records with a positive integer total', () => {
    assert.equal(isMobileEpisodeTrackable(record()), true);
    assert.equal(isMobileEpisodeTrackable(record({ mediaType: '纪录片' })), true);
    assert.equal(isMobileEpisodeTrackable(record({ mediaType: '电影' })), false);
    assert.equal(isMobileEpisodeTrackable(record({ totalEpisodes: null })), false);
    assert.equal(isMobileEpisodeTrackable(record({ totalEpisodes: 0 })), false);
    assert.deepEqual(mobileEpisodeOptions(record({ totalEpisodes: 3 })), [1, 2, 3]);
  });

  test('derives complete, jump, retreat, finish, and summary without reading legacy progress', () => {
    const active = record({ episodeTrackingEnabled: true, nextEpisode: 3, status: '在看', progress: '旧进度 E01' });
    assert.equal(nextEpisodeAfterCompletion(active), 4);
    assert.equal(nextEpisodeAfterCompletion({ ...active, nextEpisode: 4 }), null);
    assert.equal(episodeAdjustmentAction(active, 1), 'retreat');
    assert.equal(episodeAdjustmentAction(active, 4), 'jump');
    assert.equal(episodeAdjustmentAction(active, null), 'finish');
    assert.equal(mobileEpisodeSummary(active), '共 4 集 · 下一集：第 3 集');
    assert.equal(mobileEpisodeSummary({ ...active, nextEpisode: 5 }), '共 4 集 · 逐集进度与总集数不一致');
    assert.equal(mobileEpisodeSummary(record({ progress: '旧进度 E01' })), '共 4 集 · 未启用逐集跟踪');
  });

  test('resumes only a completed tracked record whose total exceeds persisted history', () => {
    const completed = record({ episodeTrackingEnabled: true, status: '已看', nextEpisode: null });
    const history = [completion(1), completion(2)];
    assert.equal(maxEpisodeInHistory(history), 2);
    assert.equal(resumableNextEpisode(completed, history), 3);
    assert.equal(resumableNextEpisode({ ...completed, totalEpisodes: 2 }, history), null);
    assert.equal(resumableNextEpisode({ ...completed, episodeTrackingEnabled: false }, history), null);
    assert.equal(resumableNextEpisode({ ...completed, status: '在看' }, history), null);
  });

  test('classifies atomic command failures into safe mobile outcomes', () => {
    assert.equal(mobileEpisodeWriteReason(new Error('stale_episode_progress')), 'stale');
    assert.equal(mobileEpisodeWriteReason('episode_record_locked'), 'locked');
    assert.equal(mobileEpisodeWriteReason({ code: 'episode_record_missing' }), 'missing');
    assert.equal(mobileEpisodeWriteReason('episode_out_of_range'), 'domain');
    assert.equal(mobileEpisodeWriteReason(new Error('database unavailable')), 'error');
  });
});
