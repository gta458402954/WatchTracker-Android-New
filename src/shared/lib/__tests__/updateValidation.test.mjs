import assert from 'node:assert/strict';
import test from 'node:test';

import { assertValidUpdateNumbers } from '../updateValidation.ts';

test('rejects every non-finite numeric update', () => {
  for (const value of [Number.NaN, Number.POSITIVE_INFINITY, Number.NEGATIVE_INFINITY]) {
    assert.throws(
      () => assertValidUpdateNumbers({ imdbRating: value }),
      /Invalid non-finite number provided for field: imdbRating/,
    );
  }
});

test('accepts finite numbers, nulls and non-numeric fields', () => {
  assert.doesNotThrow(() => assertValidUpdateNumbers({
    rating: 10,
    imdbRating: 8.8,
    totalEpisodes: null,
    notes: 'ok',
  }));
});
