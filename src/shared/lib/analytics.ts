import { WatchRecord } from '../types';
import { mediaTypeOf } from './classification';

export function calculateAffinityScore(targetRecord: WatchRecord, allRecords: WatchRecord[]): number {
  if (targetRecord.status !== '未看') return 1.0;

  // 评分采用 10 分制，8 分及以上视为偏好样本
  const highlyRated = allRecords.filter(r => r.status === '已看' && (r.rating ?? 0) >= 8);
  if (highlyRated.length === 0) return 1.0;

  let scoreMultiplier = 1.0;

  // 提取用户喜爱的高频标签
  const favoriteGenres = new Map<string, number>();
  let totalGenresCount = 0;
  highlyRated.forEach(r => {
    if (r.genres) {
      r.genres.split(',').forEach(g => {
        const genre = g.trim();
        if (genre) {
          favoriteGenres.set(genre, (favoriteGenres.get(genre) || 0) + 1);
          totalGenresCount++;
        }
      });
    }
  });

  // 如果目标剧有标签，按匹配频率加分
  if (targetRecord.genres) {
    targetRecord.genres.split(',').forEach(g => {
      const genre = g.trim();
      const count = favoriteGenres.get(genre) || 0;
      if (count > 0 && totalGenresCount > 0) {
        // 简单计算：每命中一个喜爱标签，加 0.05
        scoreMultiplier += 0.05;
      }
    });
  }

  // 也可以对 Platform (Network) 做类似匹配
  const favoritePlatforms = new Map<string, number>();
  highlyRated.forEach(r => {
    if (r.platform) {
      favoritePlatforms.set(r.platform, (favoritePlatforms.get(r.platform) || 0) + 1);
    }
  });

  if (targetRecord.platform) {
    const count = favoritePlatforms.get(targetRecord.platform) || 0;
    if (count > 0) {
      // 命中喜爱的制作厂牌加 0.05
      scoreMultiplier += 0.05;
    }
  }

  return scoreMultiplier;
}

export function calculateWatchValue(record: WatchRecord, allRecords: WatchRecord[]): number {
  if (record.status !== '未看') return 0;

  // 1. 基础分: 如果没有 IMDb 评分，默认 6.0
  const baseScore = record.imdbRating || 6.0;

  // 2. 兴趣倍率
  let interestMultiplier = 1.0;
  if (record.interestLevel === 5) interestMultiplier = 1.2;
  else if (record.interestLevel === 4) interestMultiplier = 1.1;
  else if (record.interestLevel === 3) interestMultiplier = 1.0;
  else if (record.interestLevel === 2) interestMultiplier = 0.9;
  else if (record.interestLevel === 1) interestMultiplier = 0.8;

  // 3. 完结加成
  let completionBonus = 1.0;
  if (record.tmdbStatus === 'Ended' || record.tmdbStatus === 'Miniseries') {
    completionBonus = 1.15;
  } else if (!record.totalEpisodes && ['电影', '纪录片', '动画'].includes(mediaTypeOf(record))) {
    completionBonus = 1.15; // 单部电影、纪录片和动画通常一次性看完
  }

  // 4. 相似度得分
  const affinity = calculateAffinityScore(record, allRecords);

  // 综合计算
  let finalValue = baseScore * interestMultiplier * completionBonus * affinity;

  // 放缩到 0-100 (因为最高分可能是 10 * 1.2 * 1.15 * 1.2 = 16.5)
  // 乘以 6.5，使得基础 8 分剧通常在 50-70 分左右，神作可以逼近 100
  finalValue = finalValue * 6.5;

  return Math.min(Math.round(finalValue), 100);
}

// === Dashboard Analytics ===

export function getGenreDistribution(records: WatchRecord[]) {
  const genres = new Map<string, number>();
  records.forEach(r => {
    if (r.genres) {
      r.genres.split(',').forEach(g => {
        const genre = g.trim();
        if (genre && genre !== '剧情' && genre.toLowerCase() !== 'drama') {
          genres.set(genre, (genres.get(genre) || 0) + 1);
        }
      });
    }
  });
  return Array.from(genres.entries())
    .map(([name, count]) => ({ name, value: count }))
    .sort((a, b) => b.value - a.value)
    .slice(0, 8); // 取前 8 种主要类型
}
