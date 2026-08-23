import assert from 'node:assert/strict';
import test from 'node:test';

import {
  classifySyncFailure,
  focusPullDue,
  nextRetryAt,
  parsePullInterval,
  periodicPullDue,
  retryDelayMs,
} from '../syncScheduling.ts';

test('pull interval accepts only the supported independent minute values', () => {
  assert.equal(parsePullInterval(null), 15);
  assert.equal(parsePullInterval('0'), 0);
  assert.equal(parsePullInterval('5'), 5);
  assert.equal(parsePullInterval('60'), 60);
  assert.equal(parsePullInterval('30 seconds'), 15);
  assert.equal(parsePullInterval('7'), 15);
});

test('sync failures distinguish retry, local races, and user blockers', () => {
  assert.equal(classifySyncFailure('HTTP Error: 503'), 'retry');
  assert.equal(classifySyncFailure('remote_busy'), 'retry');
  assert.equal(classifySyncFailure('stale_local_snapshot'), 'stale-local');
  assert.equal(classifySyncFailure('HTTP Error: 401'), 'blocked');
  assert.equal(classifySyncFailure('conditional_write_unsupported'), 'blocked');
  assert.equal(classifySyncFailure('conditional_validator_rejected'), 'blocked');
  assert.equal(classifySyncFailure('unsupported_remote_schema'), 'blocked');
});

test('retry delay uses the approved bounded ladder and deterministic jitter', () => {
  assert.equal(retryDelayMs(1), 10_000);
  assert.equal(retryDelayMs(2), 30_000);
  assert.equal(retryDelayMs(3), 120_000);
  assert.equal(retryDelayMs(4), 300_000);
  assert.equal(retryDelayMs(99), 900_000);
  assert.equal(retryDelayMs(1, 0.2), 12_000);
  assert.equal(retryDelayMs(1, -0.2), 8_000);
  assert.equal(nextRetryAt(2, 0), '1970-01-01T00:00:30.000Z');
});

test('focus and periodic pulls handle cooldown, disable, and clock rollback', () => {
  const now = Date.parse('2026-08-02T12:00:00.000Z');
  assert.equal(focusPullDue('2026-08-02T11:59:40.000Z', now), false);
  assert.equal(focusPullDue('2026-08-02T11:59:29.000Z', now), true);
  assert.equal(focusPullDue('2026-08-03T00:00:00.000Z', now), true);
  assert.equal(periodicPullDue(null, 15, now), true);
  assert.equal(periodicPullDue('2026-08-02T11:50:00.000Z', 15, now), false);
  assert.equal(periodicPullDue('2026-08-02T11:44:59.000Z', 15, now), true);
  assert.equal(periodicPullDue(null, 0, now), false);
});
