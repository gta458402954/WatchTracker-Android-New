import assert from 'node:assert/strict';
import test from 'node:test';
import { syncError, syncFailureMessage } from '../../../features/sync/domain/syncErrors.ts';

test('sync errors map stable codes to safe Chinese messages', () => {
  const codes = [
    'conditional_write_unsupported', 'conditional_validator_rejected', 'remote_busy',
    'stale_local_snapshot', 'stale_sync_target', 'target_migration_required',
    'unsupported_remote_schema', 'legacy_remote_changed', 'episode_sync_upgrade_required',
    'episode_completion_conflict', 'collections_sync_upgrade_required',
  ];
  for (const code of codes) {
    const message = syncFailureMessage(code);
    assert.ok(message && !message.includes(code));
  }
  assert.equal(syncFailureMessage('internal-secret'), null);
  assert.deepEqual(syncError(new Error('conditional_write_unsupported: password=secret')), { ok: false, error: 'conditional_write_unsupported' });
  assert.deepEqual(syncError(new Error('stale_local_snapshot')), { ok: false, error: 'stale_local_snapshot', staleLocal: true });
});
