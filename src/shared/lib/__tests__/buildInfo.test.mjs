import assert from 'node:assert/strict';
import { describe, test } from 'node:test';

import { createBuildInfo, formatGitCommitTime } from '../buildInfoCore.ts';

describe('build information', () => {
  test('keeps the full commit, derives the short commit, and retains the actual commit timestamp', () => {
    const info = createBuildInfo({
      productVersion: '1.10.0',
      gitCommit: 'ABCDEF1234567890ABCDEF1234567890ABCDEF12',
      gitCommitTime: '2026-08-08T10:11:12+08:00',
    });
    assert.deepEqual(info, {
      productVersion: '1.10.0',
      gitCommit: 'abcdef1234567890abcdef1234567890abcdef12',
      shortCommit: 'abcdef12',
      gitCommitTime: '2026-08-08T10:11:12+08:00',
      fallback: false,
    });
    assert.match(formatGitCommitTime(info.gitCommitTime, 'zh-CN'), /2026/);
  });

  test('uses an explicit development fallback for unavailable or invalid Git metadata', () => {
    assert.deepEqual(createBuildInfo(undefined), {
      productVersion: '0.0.0-dev',
      gitCommit: 'unknown',
      shortCommit: 'unknown',
      gitCommitTime: null,
      fallback: true,
    });
    assert.equal(formatGitCommitTime(null, 'zh-CN'), '开发环境不可用');
    assert.equal(createBuildInfo({ gitCommit: 'not-a-commit', gitCommitTime: 'today' }).fallback, true);
  });
});
