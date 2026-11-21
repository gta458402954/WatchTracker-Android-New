import type { Status, WatchRecord } from '../../../shared/types';
import { displayTitlesOf } from '../../../shared/lib/displayTitle';
import SafePosterImage from './SafePosterImage';

interface MobilePosterWallProps {
  records: WatchRecord[];
  onOpen: (record: WatchRecord) => void;
  onMore: (record: WatchRecord, trigger: HTMLButtonElement) => void;
  onStatus: (record: WatchRecord, status: Status) => void;
}

function titleOf(record: WatchRecord): string {
  const titles = displayTitlesOf(record);
  return titles.primary || titles.secondary || '未命名记录';
}

export default function MobilePosterWall({ records, onOpen, onMore }: MobilePosterWallProps) {
  return (
    <div className="mobile-poster-grid" role="list" aria-label="海报墙列表">
      {records.map(record => {
        const title = titleOf(record);
        return (
          <article
            key={record.id}
            className="mobile-poster-card"
            role="listitem"
            data-record-id={record.id}
            tabIndex={0}
            onClick={() => onOpen(record)}
            onKeyDown={event => {
              if (event.key === 'Enter' || event.key === ' ') {
                event.preventDefault();
                onOpen(record);
              }
            }}
            aria-label={`${title}，${record.status}${record.rating != null ? `，评分 ${record.rating}` : ''}`}
          >
            <div className="mobile-poster-frame">
              {record.posterPath ? (
                <SafePosterImage
                  posterPath={record.posterPath}
                  alt={`${title} 海报`}
                  className="mobile-poster-img"
                  size="w342"
                  compact
                  autoDownload={false}
                />
              ) : (
                <div className="mobile-poster-placeholder">
                  <span className="mobile-poster-placeholder-icon" aria-hidden="true">🎬</span>
                  <span className="mobile-poster-placeholder-text">{title}</span>
                </div>
              )}

              {/* Top badges & more button */}
              <div className="mobile-poster-top">
                <div className="mobile-poster-badge-group">
                  {record.isLocked && (
                    <span className="mobile-poster-badge locked" title="已锁定">🔒</span>
                  )}
                  <span className={`mobile-poster-badge status-${record.status}`}>
                    {record.status}
                  </span>
                </div>
                <button
                  type="button"
                  className="mobile-poster-more-btn"
                  aria-label={`更多操作：${title}`}
                  title="更多操作"
                  onClick={event => {
                    event.stopPropagation();
                    onMore(record, event.currentTarget);
                  }}
                >
                  <span aria-hidden="true">⋮</span>
                </button>
              </div>

              {/* Bottom gradient and info */}
              <div className="mobile-poster-info">
                <div className="mobile-poster-title">{title}</div>
                <div className="mobile-poster-meta">
                  {record.releaseYear && <span>{record.releaseYear}</span>}
                  {record.rating != null && (
                    <span className="mobile-poster-rating">⭐ {record.rating}</span>
                  )}
                </div>
              </div>

              {/* Episode Progress Mini Bar */}
              {record.totalEpisodes && record.progress && (
                <div className="mobile-poster-progress-track" aria-hidden="true">
                  {(() => {
                    const match = record.progress.match(/\d+/);
                    if (match) {
                      const current = parseInt(match[0], 10);
                      const percent = Math.min((current / record.totalEpisodes) * 100, 100);
                      return (
                        <div
                          className="mobile-poster-progress-fill"
                          style={{ width: `${percent}%` }}
                        />
                      );
                    }
                    return null;
                  })()}
                </div>
              )}
            </div>
          </article>
        );
      })}
    </div>
  );
}
