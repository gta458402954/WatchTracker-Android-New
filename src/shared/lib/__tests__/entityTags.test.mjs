import assert from 'node:assert/strict';
import test from 'node:test';
import {
  entityTagKind,
  firstUsableEntityTag,
  normalizedEntityTag,
} from '../../../features/sync/domain/entityTags.ts';

test('entity tags normalize quoted, unquoted, and weak values without accepting controls', () => {
  assert.equal(normalizedEntityTag('  "abc"  '), '"abc"');
  assert.equal(normalizedEntityTag('abc'), '"abc"');
  assert.equal(normalizedEntityTag('W/abc'), 'W/"abc"');
  assert.equal(normalizedEntityTag('"a\\"b"'), null);
  assert.equal(normalizedEntityTag('abc\u0001'), null);
  assert.equal(entityTagKind('"abc"'), 'strong');
  assert.equal(entityTagKind('W/"abc"'), 'weak');
  assert.equal(entityTagKind('abc'), null);
});

test('PROPFIND candidate selection ignores unusable values and keeps the first safe validator', () => {
  assert.equal(firstUsableEntityTag([null, '', 'bad\u0001', 'W/opaque', 'later']), 'W/"opaque"');
  assert.equal(firstUsableEntityTag(['"strong-1"', 'later']), '"strong-1"');
  assert.equal(firstUsableEntityTag([null, '', 'bad\u0001']), null);
});
