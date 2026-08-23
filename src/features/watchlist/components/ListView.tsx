import { WatchRecord, Status } from '../../../shared/types';
import RecordCard from './RecordCard';

interface ListViewProps {
  filtered: WatchRecord[];
  onEdit: (record: WatchRecord) => void;
  onDelete: (id: string) => void;
  onLockToggle: (id: string) => void;
  onStatusChange: (id: string, status: Status) => void;
  onNextEpisodeChange: (record: WatchRecord, nextEpisode: number | null) => void;
  collectionNamesByRecord?: Map<string, string[]>;
}

export default function ListView({ filtered, onEdit, onDelete, onLockToggle, onStatusChange, onNextEpisodeChange, collectionNamesByRecord }: ListViewProps) {
  return (
    <div className="grid grid-cols-1 gap-3 sm:grid-cols-2 lg:grid-cols-3">
      {filtered.map(record => (
        <RecordCard key={record.id} record={record} collectionNames={collectionNamesByRecord?.get(record.id)} onEdit={onEdit} onDelete={onDelete} onLockToggle={onLockToggle} onStatusChange={onStatusChange} onNextEpisodeChange={onNextEpisodeChange} />
      ))}
    </div>
  );
}
