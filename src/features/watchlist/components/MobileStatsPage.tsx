import { useMemo } from 'react';
import type { WatchRecord } from '../../../shared/types';
import { getGenreDistribution } from '../../../shared/lib/analytics';
import { compareViewingDatesDesc, viewingDateText } from '../../../shared/lib/viewingDate';
import { displayTitlesOf } from '../../../shared/lib/displayTitle';
import SafePosterImage from './SafePosterImage';

interface MobileStatsPageProps {
  records: WatchRecord[];
  onDetail?: (record: WatchRecord) => void;
}

function titleOf(record: WatchRecord): string {
  const titles = displayTitlesOf(record);
  return titles.primary || titles.secondary || '未命名记录';
}

function estimateWatchedMinutes(record: WatchRecord): number {
  const isEpisodic = Boolean(record.totalEpisodes) || ['剧集', '动画', '纪录片', '综艺'].includes(record.mediaType);
  const epRuntime = record.episodeRuntime || 45;
  if (isEpisodic) {
    if (record.status === '已看') {
      return (record.totalEpisodes || 10) * epRuntime;
    }
    if (record.status === '在看') {
      const match = record.progress ? record.progress.match(/\d+/) : null;
      const current = match ? parseInt(match[0], 10) : (record.nextEpisode ? Math.max(1, record.nextEpisode - 1) : 1);
      return current * epRuntime;
    }
    return 0;
  }
  // Single/Movie
  if (record.status === '已看') {
    return record.movieDuration || 110;
  }
  if (record.status === '在看') {
    return Math.round((record.movieDuration || 110) * 0.5);
  }
  return 0;
}

function formatWatchDuration(minutes: number): { days: number; hours: number; mins: number; text: string } {
  const days = Math.floor(minutes / 1440);
  const hours = Math.floor((minutes % 1440) / 60);
  const mins = minutes % 60;

  if (days > 0) {
    return { days, hours, mins, text: `${days} 天 ${hours} 小时` };
  }
  if (hours > 0) {
    return { days: 0, hours, mins, text: `${hours} 小时 ${mins} 分` };
  }
  return { days: 0, hours: 0, mins, text: `${mins} 分钟` };
}

export default function MobileStatsPage({ records, onDetail }: MobileStatsPageProps) {
  const stats = useMemo(() => {
    const total = records.length;
    const completed = records.filter(r => r.status === '已看');
    const watching = records.filter(r => r.status === '在看');
    const unwatched = records.filter(r => r.status === '未看');

    const totalMinutes = records.reduce((sum, r) => sum + estimateWatchedMinutes(r), 0);
    const duration = formatWatchDuration(totalMinutes);
    const completionRate = total > 0 ? Math.round((completed.length / total) * 100) : 0;

    // Media type distribution
    const mediaTypes = new Map<string, number>();
    records.forEach(r => {
      mediaTypes.set(r.mediaType, (mediaTypes.get(r.mediaType) || 0) + 1);
    });
    const mediaList = Array.from(mediaTypes.entries())
      .map(([type, count]) => ({ type, count, pct: total > 0 ? Math.round((count / total) * 100) : 0 }))
      .sort((a, b) => b.count - a.count);

    // Genre distribution
    const genres = getGenreDistribution(completed.length > 0 ? completed : records).slice(0, 6);
    const maxGenreCount = genres.length > 0 ? genres[0].value : 1;

    // Platform distribution
    const platforms = new Map<string, number>();
    records.forEach(r => {
      const p = (r.platform || '').trim();
      if (p) platforms.set(p, (platforms.get(p) || 0) + 1);
    });
    const platformList = Array.from(platforms.entries())
      .map(([name, count]) => ({ name, count }))
      .sort((a, b) => b.count - a.count)
      .slice(0, 5);

    // Top rated records
    const topRated = [...records]
      .filter(r => r.rating != null && r.rating >= 8)
      .sort((a, b) => (b.rating ?? 0) - (a.rating ?? 0))
      .slice(0, 4);

    // Recent completions
    const recentCompleted = [...completed]
      .filter(r => Boolean(r.endDate))
      .sort((a, b) => compareViewingDatesDesc(a.endDate, b.endDate))
      .slice(0, 4);

    return {
      total,
      completedCount: completed.length,
      watchingCount: watching.length,
      unwatchedCount: unwatched.length,
      completionRate,
      duration,
      mediaList,
      genres,
      maxGenreCount,
      platformList,
      topRated,
      recentCompleted,
    };
  }, [records]);

  return (
    <div className="mobile-stats-page" aria-labelledby="mobile-stats-title">
      <header className="mobile-page-heading">
        <div>
          <h1 id="mobile-stats-title">观影统计</h1>
          <p>片库数据与个人观影偏好总览</p>
        </div>
      </header>

      {/* Hero Watch Time Card */}
      <section className="mobile-stats-hero-card" aria-label="总观看时间">
        <div className="mobile-stats-hero-label">累计观看投入时长</div>
        <div className="mobile-stats-hero-value">{stats.duration.text}</div>
        <div className="mobile-stats-hero-hint">基于已看与在看剧集进度估算</div>
      </section>

      {/* Metric Cards Grid */}
      <div className="mobile-stats-grid">
        <div className="mobile-stats-metric-card">
          <div className="mobile-stats-metric-num">{stats.total}</div>
          <div className="mobile-stats-metric-label">片库总量</div>
        </div>
        <div className="mobile-stats-metric-card">
          <div className="mobile-stats-metric-num" style={{ color: '#10b981' }}>{stats.completedCount}</div>
          <div className="mobile-stats-metric-label">已看完 ({stats.completionRate}%)</div>
        </div>
        <div className="mobile-stats-metric-card">
          <div className="mobile-stats-metric-num" style={{ color: '#6366f1' }}>{stats.watchingCount}</div>
          <div className="mobile-stats-metric-label">追更在看</div>
        </div>
        <div className="mobile-stats-metric-card">
          <div className="mobile-stats-metric-num" style={{ color: '#94a3b8' }}>{stats.unwatchedCount}</div>
          <div className="mobile-stats-metric-label">待看储备</div>
        </div>
      </div>

      {/* Status Proportion Bar */}
      <section className="mobile-stats-section" aria-label="状态分布">
        <h2 className="mobile-stats-section-title">状态分布</h2>
        <div className="mobile-stats-progress-track">
          {stats.total > 0 && (
            <>
              <div
                className="mobile-stats-progress-seg completed"
                style={{ width: `${(stats.completedCount / stats.total) * 100}%` }}
                title={`已看 ${stats.completedCount}`}
              />
              <div
                className="mobile-stats-progress-seg watching"
                style={{ width: `${(stats.watchingCount / stats.total) * 100}%` }}
                title={`在看 ${stats.watchingCount}`}
              />
              <div
                className="mobile-stats-progress-seg unwatched"
                style={{ width: `${(stats.unwatchedCount / stats.total) * 100}%` }}
                title={`未看 ${stats.unwatchedCount}`}
              />
            </>
          )}
        </div>
        <div className="mobile-stats-legend-row">
          <span className="legend-dot completed">已看 {stats.completedCount} ({stats.completionRate}%)</span>
          <span className="legend-dot watching">在看 {stats.watchingCount}</span>
          <span className="legend-dot unwatched">未看 {stats.unwatchedCount}</span>
        </div>
      </section>

      {/* Media Types */}
      {stats.mediaList.length > 0 && (
        <section className="mobile-stats-section" aria-label="媒体类型分布">
          <h2 className="mobile-stats-section-title">媒体类型分布</h2>
          <div className="mobile-stats-type-chips">
            {stats.mediaList.map(item => (
              <div key={item.type} className="mobile-stats-type-chip">
                <span className="type-name">{item.type}</span>
                <span className="type-count">{item.count} 部</span>
                <span className="type-pct">{item.pct}%</span>
              </div>
            ))}
          </div>
        </section>
      )}

      {/* Top Genres */}
      {stats.genres.length > 0 && (
        <section className="mobile-stats-section" aria-label="常看题材偏好">
          <h2 className="mobile-stats-section-title">常看题材偏好</h2>
          <div className="mobile-stats-bars">
            {stats.genres.map((g, idx) => {
              const fillPct = Math.round((g.value / stats.maxGenreCount) * 100);
              return (
                <div key={g.name} className="mobile-stats-bar-row">
                  <div className="bar-label">
                    <span className="bar-rank">0{idx + 1}</span>
                    <span className="bar-name">{g.name}</span>
                  </div>
                  <div className="bar-track">
                    <div className="bar-fill" style={{ width: `${fillPct}%` }} />
                  </div>
                  <div className="bar-count">{g.value} 部</div>
                </div>
              );
            })}
          </div>
        </section>
      )}

      {/* Top Platforms */}
      {stats.platformList.length > 0 && (
        <section className="mobile-stats-section" aria-label="播放平台分布">
          <h2 className="mobile-stats-section-title">播放平台 / 厂牌</h2>
          <div className="mobile-stats-platform-list">
            {stats.platformList.map(item => (
              <div key={item.name} className="mobile-stats-platform-item">
                <span className="platform-name">{item.name}</span>
                <span className="platform-count">{item.count} 部</span>
              </div>
            ))}
          </div>
        </section>
      )}

      {/* Top Rated Highlights */}
      {stats.topRated.length > 0 && (
        <section className="mobile-stats-section" aria-label="高分佳作">
          <h2 className="mobile-stats-section-title">殿堂高分榜单</h2>
          <div className="mobile-stats-mini-records">
            {stats.topRated.map(r => {
              const title = titleOf(r);
              return (
                <div
                  key={r.id}
                  className={`mobile-stats-mini-card ${onDetail ? 'clickable' : ''}`}
                  onClick={() => onDetail?.(r)}
                  role={onDetail ? 'button' : undefined}
                  tabIndex={onDetail ? 0 : undefined}
                >
                  {r.posterPath ? (
                    <SafePosterImage
                      posterPath={r.posterPath}
                      alt={title}
                      compact
                      autoDownload={false}
                      className="mini-card-poster"
                    />
                  ) : (
                    <div className="mini-card-poster-placeholder">🎬</div>
                  )}
                  <div className="mini-card-info">
                    <div className="mini-card-title">{title}</div>
                    <div className="mini-card-meta">
                      {r.releaseYear && <span>{r.releaseYear}</span>}
                      <span>{r.mediaType}</span>
                    </div>
                  </div>
                  <div className="mini-card-rating">⭐ {r.rating}</div>
                </div>
              );
            })}
          </div>
        </section>
      )}

      {/* Recent Completions */}
      {stats.recentCompleted.length > 0 && (
        <section className="mobile-stats-section" aria-label="最近完成">
          <h2 className="mobile-stats-section-title">最近完成</h2>
          <div className="mobile-stats-mini-records">
            {stats.recentCompleted.map(r => {
              const title = titleOf(r);
              return (
                <div
                  key={r.id}
                  className={`mobile-stats-mini-card ${onDetail ? 'clickable' : ''}`}
                  onClick={() => onDetail?.(r)}
                  role={onDetail ? 'button' : undefined}
                  tabIndex={onDetail ? 0 : undefined}
                >
                  {r.posterPath ? (
                    <SafePosterImage
                      posterPath={r.posterPath}
                      alt={title}
                      compact
                      autoDownload={false}
                      className="mini-card-poster"
                    />
                  ) : (
                    <div className="mini-card-poster-placeholder">🎬</div>
                  )}
                  <div className="mini-card-info">
                    <div className="mini-card-title">{title}</div>
                    <div className="mini-card-meta">
                      <span>{viewingDateText(r.endDate, 'end')}</span>
                    </div>
                  </div>
                  {r.rating != null && <div className="mini-card-rating">⭐ {r.rating}</div>}
                </div>
              );
            })}
          </div>
        </section>
      )}
    </div>
  );
}
