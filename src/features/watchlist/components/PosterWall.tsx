import { WatchRecord } from '../../../shared/types';
import { displayTitlesOf } from '../../../shared/lib/displayTitle';
import SafePosterImage from './SafePosterImage';

interface PosterWallProps {
  filtered: WatchRecord[];
  onEdit: (record: WatchRecord) => void;
  collectionNamesByRecord?: Map<string, string[]>;
}

export default function PosterWall({ filtered, onEdit, collectionNamesByRecord }: PosterWallProps) {
  return (
    <div className="grid grid-cols-2 md:grid-cols-3 lg:grid-cols-4 xl:grid-cols-5 gap-4">
      {filtered.map(record => {
        const displayTitle = displayTitlesOf(record).primary;
        return (
        <div
          key={record.id}
          onClick={() => !record.isLocked && onEdit(record)}
          title={record.isLocked ? "条目已锁定" : ""}
          className={`group relative aspect-[2/3] bg-gray-200 rounded-2xl overflow-hidden shadow-sm hover:shadow-xl transition-all ${
            record.isLocked ? 'cursor-not-allowed opacity-90' : 'cursor-pointer hover:-translate-y-1'
          }`}
        >
          {record.posterPath ? (
            <SafePosterImage
              key={record.posterPath}
              posterPath={record.posterPath}
              alt={displayTitle}
              className="w-full h-full object-cover"
            />
          ) : (
            <div className="w-full h-full flex flex-col items-center justify-center p-4 text-center bg-gradient-to-br from-gray-100 to-gray-200">
              <span className="text-3xl mb-2">🎬</span>
              <span className="text-xs font-bold text-gray-500 line-clamp-3">{displayTitle}</span>
            </div>
          )}

          {/* Overlay */}
          <div className="absolute inset-0 bg-gradient-to-t from-black/90 via-transparent to-transparent opacity-0 group-hover:opacity-100 transition-opacity flex flex-col justify-end p-3">
            <div className="text-white">
              <div className="text-xs font-bold truncate">{displayTitle}</div>
              <div className="flex items-center justify-between mt-1">
                <span className="text-[10px] bg-white/20 px-1.5 py-0.5 rounded backdrop-blur-md">
                  {record.releaseYear || '未知'}
                </span>
                <span className="text-amber-400 text-[10px] font-bold">
                  ⭐ {record.rating || '-'}
                </span>
              </div>
            </div>
          </div>

          {/* Status Badge & Lock */}
          {(collectionNamesByRecord?.get(record.id)?.length ?? 0) > 0 && <div className="absolute left-2 top-2 max-w-[75%] truncate rounded-full bg-indigo-600/90 px-2 py-1 text-[9px] font-bold text-white shadow-sm backdrop-blur" title={collectionNamesByRecord?.get(record.id)?.join('、')}>🎞️ {collectionNamesByRecord?.get(record.id)?.[0]}{(collectionNamesByRecord?.get(record.id)?.length ?? 0) > 1 ? ` +${(collectionNamesByRecord?.get(record.id)?.length ?? 1) - 1}` : ''}</div>}
          <div className="absolute top-2 right-2 flex flex-col gap-1 items-end">
             {record.isLocked && (
               <div className="text-[10px] w-6 h-6 flex items-center justify-center rounded-full bg-amber-500/90 text-white backdrop-blur-md border border-white/20 shadow-sm" title="已锁定">
                 <svg className="w-3 h-3" fill="currentColor" viewBox="0 0 20 20">
                   <path fillRule="evenodd" d="M5 9V7a5 5 0 0110 0v2a2 2 0 012 2v5a2 2 0 01-2 2H5a2 2 0 01-2-2v-5a2 2 0 012-2zm8-2v2H7V7a3 3 0 016 0z" clipRule="evenodd" />
                 </svg>
               </div>
             )}
             <div className={`text-[10px] px-2 py-0.5 rounded-full font-bold backdrop-blur-md border border-white/20 shadow-sm ${
               record.status === '在看' ? 'bg-blue-500/80 text-white' :
               record.status === '已看' ? 'bg-green-500/80 text-white' :
               'bg-gray-500/80 text-white'
             }`}>
               {record.status}
             </div>
          </div>

          {/* Progress Mini Bar (for TV) */}
          {record.totalEpisodes && record.progress && (
            <div className="absolute bottom-0 left-0 right-0 h-1 bg-white/10">
              {(() => {
                const match = record.progress.match(/\d+/);
                if (match) {
                  const current = parseInt(match[0]);
                  const percent = Math.min((current / record.totalEpisodes) * 100, 100);
                  return <div className="h-full bg-indigo-500" style={{ width: `${percent}%` }} />;
                }
                return null;
              })()}
            </div>
          )}
        </div>
        );
      })}
    </div>
  );
}
