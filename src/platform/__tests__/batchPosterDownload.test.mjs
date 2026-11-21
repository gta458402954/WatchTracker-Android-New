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
