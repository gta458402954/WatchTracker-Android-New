export type { MediaType, SeriesRecordKind, Status, TmdbMediaKind, UpdateWatchRecord, WatchRecord } from './watchRecord.generated';
export { MEDIA_TYPE_VALUES, SERIES_RECORD_KIND_VALUES, STATUS_VALUES, TMDB_MEDIA_KIND_VALUES, WATCH_RECORD_UPDATE_FIELDS } from './watchRecord.generated';

export interface EpisodeCompletion {
  id: string;
  recordId: string;
  episodeNumber: number;
  completedAt: string | null;
  createdAt: string;
  updatedAt: string;
  rev: number;
  revActor: string;
}

export type CollectionSourceKind = 'manual' | 'tmdb-movie-collection' | 'tmdb-tv-show';
export interface WatchCollection {
  id: string;
  name: string;
  normalizedName: string;
  description: string | null;
  sourceKind: CollectionSourceKind;
  sourceKey: string | null;
  collectionKind: 'manual' | 'tv-series' | 'movie-series' | 'universe';
  orderMode: 'manual' | 'chronological';
  createdAt: string;
  updatedAt: string;
  rev: number;
  revActor: string;
}

export interface CollectionMember {
  id: string;
  collectionId: string;
  recordId: string;
  position: number;
  sourceKind: 'manual' | 'tmdb';
  createdAt: string;
  updatedAt: string;
  rev: number;
  revActor: string;
}

export interface CollectionDraft {
  temporaryId: string;
  name: string;
  description: string | null;
  collectionKind: WatchCollection['collectionKind'];
}

export interface CollectionTombstone { id: string; deletedAt: string; rev: number; revActor: string }
export interface CollectionMemberTombstone extends CollectionTombstone { collectionId: string; recordId: string }
