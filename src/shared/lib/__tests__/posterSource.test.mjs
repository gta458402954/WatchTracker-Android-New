import assert from 'node:assert/strict';
import { after, before, describe, test } from 'node:test';

import { posterCacheName, posterSource } from '../posterSource.ts';

const originalWindow = globalThis.window;

function mockConvertFileSrc(platform) {
  globalThis.window.__TAURI_INTERNALS__ = {
    convertFileSrc(filePath, protocol = 'asset') {
      const path = encodeURIComponent(filePath);
      return platform === 'windows'
        ? `http://${protocol}.localhost/${path}`
        : `${protocol}://localhost/${path}`;
    },
  };
}

describe('poster protocol source', () => {
  before(() => {
    globalThis.window = {};
  });

  after(() => {
    if (originalWindow === undefined) delete globalThis.window;
    else globalThis.window = originalWindow;
  });

  test('uses the Windows custom-protocol origin without changing safe TMDB names', () => {
    mockConvertFileSrc('windows');
    assert.equal(posterCacheName('/2baf1e.jpg', 'w92'), 'w92_2baf1e.jpg');
    assert.equal(
      posterSource('/2baf1e.jpg', 'w92', 4),
      'http://poster.localhost/w92_2baf1e.jpg?v=4',
    );
    assert.equal(
      posterSource('/abc-_123.jpg', 'w342', 0),
      'http://poster.localhost/abc-_123.jpg?v=0',
    );
  });

  test('uses the native custom-protocol origin on non-Windows platforms', () => {
    mockConvertFileSrc('linux');
    assert.equal(
      posterSource('/2baf1e.jpg', 'w342', 1),
      'poster://localhost/2baf1e.jpg?v=1',
    );
  });
});
