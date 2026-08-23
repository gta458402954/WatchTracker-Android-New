import test from 'node:test';
import assert from 'node:assert/strict';
import { classifyMovieCollectionPart, normalizeImdbId } from '../../../features/collections/lib/movieCollectionIdentity.ts';

const record = (id, overrides = {}) => ({
  id, mediaType: '电影', tmdbMediaKind: null, tmdbId: null, imdbId: null, ...overrides,
});

test('normalizes only valid IMDb title identifiers', () => {
  assert.equal(normalizeImdbId(' TT0181852 '), 'tt0181852');
  assert.equal(normalizeImdbId('nm123'), null);
});

test('matches a legacy movie by IMDb when its TMDB identity is missing', () => {
  const legacy = record('terminator', { imdbId: 'tt0088247' });
  const result = classifyMovieCollectionPart(
    { id: 218, external_ids: { imdb_id: 'tt0088247' } },
    [legacy],
    new Set(['terminator']),
  );
  assert.equal(result.status, 'member');
  assert.equal(result.recordId, 'terminator');
});

test('distinguishes reusable library movies and true missing movies', () => {
  const library = record('matrix', { tmdbMediaKind: 'movie', tmdbId: 603, imdbId: 'tt0133093' });
  assert.equal(classifyMovieCollectionPart({ id: 603, imdb_id: 'tt0133093' }, [library], new Set()).status, 'library');
  assert.equal(classifyMovieCollectionPart({ id: 604, imdb_id: 'tt0234215' }, [library], new Set()).status, 'missing');
});

test('reports identities that resolve to different local records as a conflict', () => {
  const byTmdb = record('one', { tmdbMediaKind: 'movie', tmdbId: 10, imdbId: 'tt0000010' });
  const byImdb = record('two', { imdbId: 'tt0000020' });
  assert.equal(classifyMovieCollectionPart({ id: 10, imdb_id: 'tt0000020' }, [byTmdb, byImdb], new Set()).status, 'conflict');
});

test('a legacy documentary with the same IMDb identity is reused as a movie collection part', () => {
  const legacy = {
    id: '42-up',
    mediaType: '纪录片',
    imdbId: ' TT0164312 ',
    tmdbMediaKind: null,
    tmdbId: null,
  };
  const movie = {
    id: 610,
    title: '42 Up',
    external_ids: { imdb_id: 'tt0164312' },
  };

  assert.deepEqual(classifyMovieCollectionPart(movie, [legacy], new Set()), {
    movie,
    status: 'library',
    recordId: '42-up',
  });
});

test('an explicit TV identity conflicts instead of being duplicated as a movie collection part', () => {
  const tv = {
    id: 'tv-record',
    mediaType: '纪录片',
    imdbId: 'tt0164312',
    tmdbMediaKind: 'tv',
    tmdbId: 620,
  };
  const movie = {
    id: 610,
    title: '42 Up',
    external_ids: { imdb_id: 'tt0164312' },
  };

  assert.deepEqual(classifyMovieCollectionPart(movie, [tv], new Set()), {
    movie,
    status: 'conflict',
    conflictRecordIds: ['tv-record'],
  });
});
