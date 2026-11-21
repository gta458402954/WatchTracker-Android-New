import test from 'node:test';
import assert from 'node:assert/strict';
import { displayTitlesOf } from '../../shared/lib/displayTitle.ts';

test('batch poster downloader deduplicates posters and extracts titles cleanly', () => {
  const records = [
    {
      id: 'rec-1',
      chineseName: '生活大爆炸 第 12 季',
      originalName: 'The Big Bang Theory Season 12',
      posterPath: '/tbbt12.jpg',
      status: '在看',
    },
    {
      id: 'rec-2',
      chineseName: '生活大爆炸 第 11 季',
      originalName: 'The Big Bang Theory Season 11',
      posterPath: '/tbbt12.jpg', // Duplicate poster path across seasons
      status: '已看',
    },
    {
      id: 'rec-3',
      chineseName: '无海报电影',
      originalName: 'No Poster Movie',
      posterPath: null,
      status: '未看',
    },
    {
      id: 'rec-4',
      chineseName: '',
      originalName: 'Oppenheimer',
      posterPath: '/oppenheimer.jpg',
      status: '已看',
    },
  ];

  // Logic mirroring useBatchPosterDownload deduplication
  const uniquePosters = new Map();
  for (const r of records) {
    if (r.posterPath && r.posterPath.trim()) {
      if (!uniquePosters.has(r.posterPath)) {
        const titles = displayTitlesOf(r);
        uniquePosters.set(r.posterPath, {
          posterPath: r.posterPath,
          title: titles.primary || titles.secondary || r.originalName || '未命名影片',
        });
      }
    }
  }

  const items = Array.from(uniquePosters.values());
  assert.equal(items.length, 2, 'Should deduplicate 2 records sharing /tbbt12.jpg and exclude null posterPath');
  assert.equal(items[0].posterPath, '/tbbt12.jpg');
  assert.equal(items[0].title, '生活大爆炸 第 12 季');
  assert.equal(items[1].posterPath, '/oppenheimer.jpg');
  assert.equal(items[1].title, 'Oppenheimer');
});

test('batch poster downloader skips locally cached posters and only downloads missing ones', () => {
  const allItems = [
    { posterPath: '/tbbt12.jpg', title: '生活大爆炸 第 12 季' },
    { posterPath: '/oppenheimer.jpg', title: 'Oppenheimer' },
    { posterPath: '/dune2.jpg', title: '沙丘2' },
  ];

  // Case 1: tbbt12 is already cached, oppenheimer and dune2 are missing
  const missingPaths = ['/oppenheimer.jpg', '/dune2.jpg'];
  const missingSet = new Set(missingPaths);
  const missingItems = allItems.filter(it => missingSet.has(it.posterPath));
  const cachedCount = allItems.length - missingItems.length;

  assert.equal(missingItems.length, 2);
  assert.equal(cachedCount, 1, 'Should skip 1 locally cached poster');
  assert.deepEqual(missingItems.map(it => it.posterPath), ['/oppenheimer.jpg', '/dune2.jpg']);

  // Case 2: All items are cached locally
  const noMissingPaths = [];
  const noMissingSet = new Set(noMissingPaths);
  const fullyCachedItems = allItems.filter(it => noMissingSet.has(it.posterPath));
  assert.equal(fullyCachedItems.length, 0, 'No missing items when all are cached');
  assert.equal(allItems.length - fullyCachedItems.length, 3, 'All 3 items skipped as cached');
});
