import { useMemo, useState } from 'react';
import type { WatchRecord } from '../../../shared/types';
import { displayTitlesOf } from '../../../shared/lib/displayTitle';
import SafePosterImage from './SafePosterImage';
import { openExternalUrl } from '../../../platform/externalLink';
import {
  buildDiscoveryQueue,
  discoveryEmptyMessage,
  discoveryFilterOptions,
  type DiscoveryDurationLimit,
  type DiscoveryFilters,
  type DiscoveryMediaFilter,
} from '../../../shared/lib/discovery';

interface MobileDiscoverPageProps {
  records: WatchRecord[];
  onDetail: (record: WatchRecord) => void;
}

const DEFAULT_FILTERS: DiscoveryFilters = {
  durationLimit: 120,
  mediaType: '全部',
  platform: null,
  endedOnly: false,
};

const DURATION_OPTIONS: Array<{ limit: DiscoveryDurationLimit; label: string }> = [
  { limit: 30, label: '30 分钟内' },
  { limit: 60, label: '1 小时内' },
  { limit: 120, label: '2 小时内' },
  { limit: 0, label: '不限时长' },
];

function titleOf(record: WatchRecord): string {
  const titles = displayTitlesOf(record);
  return titles.primary || titles.secondary || '未命名记录';
}

export default function MobileDiscoverPage({ records, onDetail }: MobileDiscoverPageProps) {
  const [filters, setFilters] = useState<DiscoveryFilters>(DEFAULT_FILTERS);
  const [seenIds, setSeenIds] = useState<Set<string>>(() => new Set());
  const [skippedIds, setSkippedIds] = useState<Set<string>>(() => new Set());

  const options = useMemo(() => discoveryFilterOptions(records), [records]);

  const queue = useMemo(
    () => buildDiscoveryQueue(records, filters, skippedIds),
    [filters, records, skippedIds],
  );

  const activeCandidates = useMemo(
    () => queue.candidates.filter(c => !seenIds.has(c.record.id)),
    [queue.candidates, seenIds],
  );

  const currentCandidate = activeCandidates[0] ?? null;
  const currentRecord = currentCandidate?.record ?? null;
  const currentTitle = currentRecord ? titleOf(currentRecord) : '';

  const updateFilters = (updates: Partial<DiscoveryFilters>) => {
    setFilters(prev => ({ ...prev, ...updates }));
    setSeenIds(new Set());
  };

  const handleNext = () => {
    if (!currentCandidate) return;
    setSeenIds(prev => new Set(prev).add(currentCandidate.record.id));
  };

  const handleSkip = () => {
    if (!currentCandidate) return;
    setSkippedIds(prev => new Set(prev).add(currentCandidate.record.id));
    setSeenIds(prev => new Set(prev).add(currentCandidate.record.id));
  };

  const handleResetRound = () => {
    setSeenIds(new Set());
    setSkippedIds(new Set());
  };

  return (
    <div className="mobile-discover-page" aria-labelledby="mobile-discover-title">
      <header className="mobile-page-heading">
        <div>
          <h1 id="mobile-discover-title">今晚看什么</h1>
          <p>基于待看片库、偏好题材与预计时长的智能推荐</p>
        </div>
      </header>

      {/* Filter Chips - Duration */}
      <section className="mobile-discover-filters" aria-label="片长限制">
        <div className="mobile-discover-filter-scroll">
          {DURATION_OPTIONS.map(opt => (
            <button
              key={opt.limit}
              type="button"
              className={`mobile-chip ${filters.durationLimit === opt.limit ? 'selected' : ''}`}
              aria-pressed={filters.durationLimit === opt.limit}
              onClick={() => updateFilters({ durationLimit: opt.limit })}
            >
              {opt.label}
            </button>
          ))}
        </div>
      </section>

      {/* Filter Chips - Media Types */}
      <section className="mobile-discover-filters" aria-label="媒体类型">
        <div className="mobile-discover-filter-scroll">
          {(['全部', ...options.mediaTypes] as DiscoveryMediaFilter[]).map(type => (
            <button
              key={type}
              type="button"
              className={`mobile-chip ${filters.mediaType === type ? 'selected' : ''}`}
              aria-pressed={filters.mediaType === type}
              onClick={() => updateFilters({ mediaType: type })}
            >
              {type}
            </button>
          ))}
          {/* Ended Only Toggle */}
          <button
            type="button"
            className={`mobile-chip ${filters.endedOnly ? 'selected' : ''}`}
            aria-pressed={filters.endedOnly}
            onClick={() => updateFilters({ endedOnly: !filters.endedOnly })}
          >
            {filters.endedOnly ? '✓ 仅看已完结' : '仅看已完结'}
          </button>
        </div>
      </section>

      {/* Recommendation Main Stage */}
      {currentCandidate && currentRecord ? (
        <article className="mobile-discover-card" data-record-id={currentRecord.id}>
          <div className="mobile-discover-card-hero">
            <div className="mobile-discover-poster-wrap">
              {currentRecord.posterPath ? (
                <SafePosterImage
                  posterPath={currentRecord.posterPath}
                  alt={`${currentTitle} 海报`}
                  className="mobile-discover-poster"
                  size="w342"
                  compact
                  autoDownload={false}
                />
              ) : (
                <div className="mobile-discover-poster-placeholder">
                  <span>🎬</span>
                  <small>{currentTitle}</small>
                </div>
              )}
              {/* Score pill */}
              <div className="mobile-discover-score-badge">
                <span className="score-val">{currentCandidate.score}</span>
                <span className="score-lbl">契合分</span>
              </div>
            </div>

            <div className="mobile-discover-info">
              <h2 className="mobile-discover-title">{currentTitle}</h2>
              {displayTitlesOf(currentRecord).secondary && (
                <p className="mobile-discover-subtitle">{displayTitlesOf(currentRecord).secondary}</p>
              )}

              <div className="mobile-discover-tags">
                <span className="mobile-discover-pill">{currentRecord.mediaType}</span>
                {currentRecord.releaseYear && (
                  <span className="mobile-discover-pill">{currentRecord.releaseYear}</span>
                )}
                {currentRecord.platform && (
                  <span className="mobile-discover-pill">{currentRecord.platform}</span>
                )}
                {currentCandidate.viewing && (
                  <span className="mobile-discover-pill highlight">
                    ⏱️ 预计 {currentCandidate.viewing.minutes} 分钟
                  </span>
                )}
                {currentRecord.imdbRating != null && (
                  <span
                    role={currentRecord.imdbId ? 'button' : undefined}
                    tabIndex={currentRecord.imdbId ? 0 : undefined}
                    className={`mobile-discover-pill imdb ${currentRecord.imdbId ? 'clickable' : ''}`}
                    title={currentRecord.imdbId ? '在 IMDb 中打开' : undefined}
                    onClick={() => {
                      if (currentRecord.imdbId) {
                        openExternalUrl(`https://www.imdb.com/title/${currentRecord.imdbId}/`);
                      }
                    }}
                  >
                    ⭐ IMDb {currentRecord.imdbRating.toFixed(1)} {currentRecord.imdbId ? '↗' : ''}
                  </span>
                )}
              </div>

              {/* Recommendation Reasons */}
              {currentCandidate.reasons.length > 0 && (
                <div className="mobile-discover-reasons">
                  <div className="reasons-heading">推荐理由</div>
                  <ul>
                    {currentCandidate.reasons.map(reason => (
                      <li key={reason}>✨ {reason}</li>
                    ))}
                  </ul>
                </div>
              )}

              {/* Notes / Plot snippet if available */}
              {currentRecord.notes && (
                <p className="mobile-discover-notes">
                  <strong>备注：</strong>{currentRecord.notes}
                </p>
              )}
            </div>
          </div>

          {/* Action Buttons */}
          <div className="mobile-discover-actions">
            <button
              type="button"
              className="mobile-primary-button"
              style={{ flex: 1.5 }}
              onClick={() => onDetail(currentRecord)}
            >
              🎬 查看详情与记录
            </button>
            <button
              type="button"
              className="mobile-quiet-button"
              style={{ flex: 1 }}
              onClick={handleNext}
            >
              🎲 换一部 ({activeCandidates.length})
            </button>
            <button
              type="button"
              className="mobile-quiet-button mobile-danger"
              onClick={handleSkip}
              title="本轮跳过此片"
            >
              跳过
            </button>
          </div>
        </article>
      ) : (
        <section className="mobile-discover-empty" aria-live="polite">
          <span className="mobile-discover-empty-icon" aria-hidden="true">🍿</span>
          <h2>{discoveryEmptyMessage(queue.emptyReason)}</h2>
          <p>
            {queue.emptyReason === 'skipped'
              ? '当前条件下的待看候选已全部翻阅完毕。可以重新轮换或尝试放宽时长与类型限制。'
              : '建议在片库中添加更多待看记录，或尝试放宽时长与类型限制。'}
          </p>
          <div className="mobile-discover-empty-actions">
            <button
              type="button"
              className="mobile-primary-button"
              onClick={handleResetRound}
            >
              🔄 重新轮换推荐候选
            </button>
            <button
              type="button"
              className="mobile-quiet-button"
              onClick={() => updateFilters(DEFAULT_FILTERS)}
            >
              重置所有筛选
            </button>
          </div>
        </section>
      )}

      {/* Queue Peek list */}
      {activeCandidates.length > 1 && (
        <section className="mobile-discover-queue-peek" aria-label="更多候选预览">
          <div className="queue-peek-header">
            <h3>更多匹配佳作 ({activeCandidates.length - 1} 部)</h3>
            <small>按契合度智能排序</small>
          </div>
          <div className="queue-peek-scroll">
            {activeCandidates.slice(1, 10).map(c => {
              const itemTitle = titleOf(c.record);
              return (
                <button
                  key={c.record.id}
                  type="button"
                  className="queue-peek-item"
                  onClick={() => onDetail(c.record)}
                  title={itemTitle}
                >
                  {c.record.posterPath ? (
                    <SafePosterImage
                      posterPath={c.record.posterPath}
                      alt={itemTitle}
                      className="queue-peek-poster"
                      size="w92"
                      compact
                      autoDownload={false}
                    />
                  ) : (
                    <div className="queue-peek-poster-placeholder">🎬</div>
                  )}
                  <span className="queue-peek-title">{itemTitle}</span>
                  <span className="queue-peek-score">{c.score}分</span>
                </button>
              );
            })}
          </div>
        </section>
      )}
    </div>
  );
}
