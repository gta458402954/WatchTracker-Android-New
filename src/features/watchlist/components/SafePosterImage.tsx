import { useCallback, useMemo, useState } from 'react';
import { downloadPosterAsync } from '../../../shared/lib/database';
import { reportOperationFailure } from '../../../shared/lib/feedback';
import { posterSource } from '../../../shared/lib/posterSource';

interface SafePosterImageProps {
  posterPath: string;
  size?: 'w92' | 'w342';
  alt: string;
  className: string;
  compact?: boolean;
}

export default function SafePosterImage({ posterPath, size = 'w342', alt, className, compact = false }: SafePosterImageProps) {
  const [revision, setRevision] = useState(0);
  const [attempted, setAttempted] = useState(false);
  const [failed, setFailed] = useState(false);
  const [loading, setLoading] = useState(false);
  const source = useMemo(
    () => posterSource(posterPath, size, revision),
    [posterPath, revision, size],
  );

  const download = useCallback(async () => {
    if (loading) return;
    setLoading(true);
    setFailed(false);
    try {
      await downloadPosterAsync(posterPath, size);
      setRevision(value => value + 1);
    } catch (error) {
      reportOperationFailure('SafePosterImage.Download', error);
      setFailed(true);
    } finally {
      setLoading(false);
    }
  }, [loading, posterPath, size]);

  if (failed) {
    return (
      <div className={`${className} flex flex-col items-center justify-center bg-gray-100 text-gray-400`}>
        <span className={compact ? 'text-[10px]' : 'text-2xl'}>{compact ? '无图' : '🎬'}</span>
        {!compact && (
          <button
            type="button"
            onClick={event => { event.stopPropagation(); setAttempted(true); void download(); }}
            disabled={loading}
            className="mt-2 rounded-lg bg-white/90 px-2 py-1 text-[10px] font-bold text-gray-600 shadow disabled:opacity-50"
          >
            {loading ? '下载中…' : '重试海报'}
          </button>
        )}
      </div>
    );
  }

  return (
    <img
      src={source}
      alt={alt}
      className={className}
      onError={() => {
        if (attempted) {
          setFailed(true);
          return;
        }
        setAttempted(true);
        void download();
      }}
    />
  );
}
