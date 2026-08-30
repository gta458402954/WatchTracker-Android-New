import assert from 'node:assert/strict';
import test from 'node:test';
import {
  formatImportBytes,
  localImportErrorCode,
  localImportErrorMessage,
} from '../../../features/backup/localImport.ts';

test('local import errors map stable Rust and Android codes to safe Chinese messages', () => {
  assert.equal(localImportErrorCode(new Error('General error: future_backup_version')), 'future_backup_version');
  assert.equal(localImportErrorMessage(new Error('General error: future_backup_version')), '备份来自更新版本，请先升级 WatchTracker。');
  assert.equal(localImportErrorMessage(new Error('Database error: secret SQL')), '无法安全验证此备份。');
});

test('local import file sizes use compact binary units', () => {
  assert.equal(formatImportBytes(512), '512 B');
  assert.equal(formatImportBytes(2048), '2.0 KiB');
  assert.equal(formatImportBytes(2 * 1024 * 1024), '2.0 MiB');
});
