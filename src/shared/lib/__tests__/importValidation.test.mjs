import assert from 'node:assert/strict';
import test from 'node:test';

import {
  normalizeImportedRecord,
  normalizeImportedRecords,
} from '../importValidation.ts';

const fixedNow = new Date('2026-07-29T00:00:00.000Z');

test('import normalization preserves region and revision fields', () => {
  const record = normalizeImportedRecord({
    id: 'region-record',
    chineseName: '地区往返',
    status: '在看',
    mediaType: '剧集',
    originCountry: 'US, GB',
    contentTags: '美国, 英国',
    releaseYear: 2026,
    rev: 7,
    revActor: 'fixture',
  }, 0, fixedNow);

  assert.equal(record.originCountry, 'US, GB');
  assert.equal(record.contentTags, '美国, 英国');
  assert.equal(record.releaseYear, '2026');
  assert.equal(record.status, '在看');
  assert.equal(record.mediaType, '剧集');
  assert.equal(record.rev, 7);
  assert.equal(record.revActor, 'fixture');
});

test('missing identifiers and timestamps receive deterministic fallbacks', () => {
  const record = normalizeImportedRecord({ id: '', createdAt: '' }, 3, fixedNow);
  assert.equal(record.id, 'imported-1785283200000-3');
  assert.equal(record.createdAt, fixedNow.toISOString());
});

test('invalid top-level payloads and array rows are rejected before replacement', () => {
  assert.throws(() => normalizeImportedRecords({ records: [] }, fixedNow), /无效的 JSON 格式/);
  assert.throws(() => normalizeImportedRecords([[]], fixedNow), /第 1 条记录格式无效/);
});

test('B003 conditional: import normalization preserves exact country codes and custom tags', () => {
  const record = normalizeImportedRecord({
    id: 'b003-import',
    originCountry: 'GB, XX, CN',
    contentTags: '律政,自定义,日本料理',
  }, 0, fixedNow);

  assert.equal(record.originCountry, 'GB, XX, CN');
  assert.equal(record.contentTags, '律政,自定义,日本料理');
});
