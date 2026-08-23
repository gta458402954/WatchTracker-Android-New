import assert from 'node:assert/strict';
import { describe, test } from 'node:test';

import { formatWebDavTargetUrl } from '../webdavDisplay.ts';

describe('WebDAV target display formatting', () => {
  test('shows a friendly provider and decoded path for Jianguo Cloud', () => {
    assert.deepEqual(
      formatWebDavTargetUrl('https://dav.jianguoyun.com/dav/%E5%BD%B1%E8%A7%86%E8%BF%BD%E8%B8%AA/'),
      {
        provider: '坚果云',
        host: 'dav.jianguoyun.com',
        path: '/影视追踪',
        summary: '坚果云 · /影视追踪',
        safeUrl: 'https://dav.jianguoyun.com/dav/影视追踪',
      },
    );
  });

  test('keeps custom hosts and ports visible', () => {
    const display = formatWebDavTargetUrl('https://dav.example.test:8443/team/library/');
    assert.equal(display.summary, 'dav.example.test:8443 · /team/library');
    assert.equal(display.safeUrl, 'https://dav.example.test:8443/team/library');
  });

  test('never includes credentials, queries, or fragments', () => {
    const display = formatWebDavTargetUrl('https://user:secret@example.test/private/?token=secret#section');
    assert.equal(display.summary, 'example.test · /private');
    assert.equal(display.safeUrl, 'https://example.test/private');
    assert.doesNotMatch(JSON.stringify(display), /user|secret|token|section/);
  });

  test('falls back safely when the URL or path encoding is invalid', () => {
    const malformedPath = formatWebDavTargetUrl('https://example.test/%E0%A4%A/?token=secret');
    assert.equal(malformedPath.summary, 'example.test · /%E0%A4%A');
    assert.doesNotMatch(malformedPath.safeUrl, /token/);

    const invalidUrl = formatWebDavTargetUrl('not a url?token=secret');
    assert.equal(invalidUrl.summary, 'not a url');
    assert.doesNotMatch(JSON.stringify(invalidUrl), /token|secret/);
  });
});
