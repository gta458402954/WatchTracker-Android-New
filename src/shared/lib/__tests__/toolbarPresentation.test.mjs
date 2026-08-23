import assert from 'node:assert/strict';
import { describe, test } from 'node:test';

import { partitionRegionOptions, syncPresentation } from '../toolbarPresentation.ts';

function runtime(overrides = {}) {
  return {
    targetId: 'target', targetEpoch: 1,
    outbox: { version: 1, pending: false, dirtyGeneration: 0, reasons: [], firstQueuedAt: null, lastQueuedAt: null },
    scheduler: { version: 1, paused: false, consecutiveFailures: 0, nextAttemptAt: null, lastAttemptAt: null, lastSuccessAt: null, lastErrorCode: null, lastRemoteCheckAt: null },
    conflictCount: 0, lastCommit: null, stagedCount: 0, publishPending: false,
    ...overrides,
  };
}

describe('toolbar sync presentation', () => {
  test('prioritizes syncing, conflicts, failures, paused, pending, then success', () => {
    assert.equal(syncPresentation({ hasCredentials: true, syncing: true, message: '', runtime: runtime({ conflictCount: 2 }), paused: false }).label, '同步中');
    assert.equal(syncPresentation({ hasCredentials: true, syncing: false, message: '', runtime: runtime({ conflictCount: 2 }), paused: false }).label, '2 项冲突');
    assert.equal(syncPresentation({ hasCredentials: true, syncing: false, message: '', runtime: runtime({ scheduler: { ...runtime().scheduler, lastErrorCode: 'http_503' } }), paused: false }).label, '同步失败');
    assert.equal(syncPresentation({ hasCredentials: true, syncing: false, message: '', runtime: runtime(), paused: true }).label, '已暂停');
    assert.equal(syncPresentation({ hasCredentials: true, syncing: false, message: '', runtime: runtime({ stagedCount: 2 }), paused: false }).label, '待同步');
    assert.equal(syncPresentation({ hasCredentials: true, syncing: false, message: '', runtime: runtime(), paused: false }).label, '已同步');
  });

  test('keeps the unconfigured state explicit and counts every pending source', () => {
    const result = syncPresentation({ hasCredentials: false, syncing: false, message: '', runtime: runtime({ outbox: { ...runtime().outbox, pending: true }, stagedCount: 3, publishPending: true }), paused: false });
    assert.equal(result.label, '未配置');
    assert.equal(result.pendingCount, 5);
  });
});

describe('toolbar region partition', () => {
  const options = ['CN', 'US', 'JP', 'KR', 'GB', 'FR', 'DE', 'unknown'].map((code, index) => ({ code, label: code, count: index + 1 }));

  test('shows seven by default and leaves the remainder in stable overflow order', () => {
    const result = partitionRegionOptions(options, [], 7);
    assert.deepEqual(result.direct.map(option => option.code), ['CN', 'US', 'JP', 'KR', 'GB', 'FR', 'DE']);
    assert.deepEqual(result.overflow.map(option => option.code), ['unknown']);
  });

  test('promotes an active zero-match option without losing the original order in overflow', () => {
    const result = partitionRegionOptions(options, ['unknown'], 4);
    assert.deepEqual(result.direct.map(option => option.code), ['unknown', 'CN', 'US', 'JP']);
    assert.deepEqual(result.overflow.map(option => option.code), ['KR', 'GB', 'FR', 'DE']);
  });
});
