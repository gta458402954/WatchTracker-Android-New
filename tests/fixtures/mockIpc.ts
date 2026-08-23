import type { Page } from '@playwright/test';
import type { WatchRecord } from '../../src/shared/types';
import type { CollectionMember, CollectionMemberTombstone, CollectionTombstone, EpisodeCompletion, WatchCollection } from '../../src/shared/types';
import type { TmdbMedia } from '../../src/shared/lib/classification';
import type { RecoveryPoint, SyncOutboxState, SyncSchedulerState } from '../../src/shared/lib/database';
import type { SyncConflictV3, SyncPayloadV3, SyncTombstoneV3 } from '../../src/shared/lib/syncMerge';

export interface MockIpcOptions {
  records?: WatchRecord[];
  episodeCompletions?: EpisodeCompletion[];
  collections?: WatchCollection[];
  collectionMembers?: CollectionMember[];
  failRecordLoads?: boolean;
  settings?: Record<string, string | null>;
  tmdbSearchResults?: TmdbMedia[];
  tmdbDetail?: TmdbMedia;
  tmdbDetails?: Record<string, TmdbMedia>;
  tmdbSeasonDetails?: Record<string, TmdbMedia>;
  tmdbDelayMs?: number;
  updateFailureCounts?: Record<string, number>;
  webdavRemote?: unknown;
  webdavV3Remote?: SyncPayloadV3 | null;
  webdavV3Etag?: string | null;
  webdavPreconditionFailures?: number;
  rotateEtagOnPreconditionFailure?: boolean;
  mutateLocalDuringPut?: boolean;
  omitPutEtag?: boolean;
  omitGetEtag?: boolean;
  webdavFailureStatus?: number;
  webdavFailureCount?: number;
  databaseCompatibilityIssue?: {
    code: 'unsupported_newer_database' | 'v19_downgrade_failed';
    detectedVersion: number;
    supportedVersion: number;
  } | null;
  recoveryPoints?: RecoveryPoint[];
  failSettingWrites?: boolean;
}

export interface MockSnapshot {
  calls: Array<{ command: string; args: Record<string, unknown> }>;
  records: WatchRecord[];
  failRecordLoads: boolean;
  settings: Record<string, string | null>;
  recoveryPoints: RecoveryPoint[];
  webdavV3Remote: SyncPayloadV3 | null;
  episodeCompletions: EpisodeCompletion[];
  collections: WatchCollection[];
  collectionMembers: CollectionMember[];
}

declare global {
  interface Window {
    __WATCHTRACKER_TEST__: MockSnapshot;
    __TAURI_INTERNALS__: {
      invoke: (command: string, args?: Record<string, unknown>) => Promise<unknown>;
      convertFileSrc: (filePath: string, protocol?: string) => string;
    };
  }
}

export async function setupMockIpc(page: Page, options: MockIpcOptions = {}) {
  await page.addInitScript(
    ({ records, episodeCompletions: initialEpisodeCompletions, collections: initialCollections, collectionMembers: initialCollectionMembers, failRecordLoads, settings, tmdbSearchResults, tmdbDetail, tmdbDetails, tmdbSeasonDetails, tmdbDelayMs, updateFailureCounts, webdavRemote, webdavV3Remote, webdavV3Etag, webdavPreconditionFailures, rotateEtagOnPreconditionFailure, mutateLocalDuringPut, omitPutEtag, omitGetEtag, webdavFailureStatus, webdavFailureCount, databaseCompatibilityIssue, recoveryPoints, failSettingWrites }) => {
      const controlledRecords = sessionStorage.getItem('__WATCHTRACKER_CONTROLLED_RECORDS__');
      const controlledRuntime = sessionStorage.getItem('__WATCHTRACKER_SYNC_RUNTIME__');
      const restoredRuntime = controlledRuntime ? JSON.parse(controlledRuntime) as {
        recordsGeneration: number; outbox: SyncOutboxState; scheduler: SyncSchedulerState;
      } : null;
      const snapshot: MockSnapshot = {
        calls: [],
        records: controlledRecords ? JSON.parse(controlledRecords) : structuredClone(records),
        failRecordLoads,
        settings: structuredClone(settings),
        recoveryPoints: structuredClone(recoveryPoints),
        webdavV3Remote: structuredClone(webdavV3Remote),
        episodeCompletions: structuredClone(initialEpisodeCompletions),
        collections: structuredClone(initialCollections),
        collectionMembers: structuredClone(initialCollectionMembers),
      };
      window.__WATCHTRACKER_TEST__ = snapshot;
      const remainingUpdateFailures = structuredClone(updateFailureCounts);
      const recoveryRecords: Record<string, WatchRecord[]> = {};
      let recoverySequence = snapshot.recoveryPoints.length;
      let recordsGeneration = restoredRuntime?.recordsGeneration ?? 0;
      let activeTargetId: string | null = snapshot.settings.webdav_creds ? 'a'.repeat(64) : null;
      let targetEpoch = activeTargetId ? 1 : 0;
      const outbox: SyncOutboxState = (() => {
        if (restoredRuntime) return structuredClone(restoredRuntime.outbox);
        try { return JSON.parse(snapshot.settings.sync_outbox_v1 || 'null') || {
          version: 1, pending: false, dirtyGeneration: 0, reasons: [], firstQueuedAt: null, lastQueuedAt: null,
        }; } catch { throw new Error('Invalid sync_outbox_v1'); }
      })();
      let scheduler: SyncSchedulerState = (() => {
        if (restoredRuntime) return structuredClone(restoredRuntime.scheduler);
        try { return JSON.parse(snapshot.settings.sync_scheduler_v1 || 'null') || {
          version: 1, paused: false, consecutiveFailures: 0, nextAttemptAt: null, lastAttemptAt: null,
          lastSuccessAt: null, lastErrorCode: null, lastRemoteCheckAt: null,
        }; } catch { throw new Error('Invalid sync_scheduler_v1'); }
      })();
      let v3Etag: string | null = webdavV3Etag;
      let remainingPreconditionFailures = webdavPreconditionFailures;
      let remainingWebdavFailures = webdavFailureCount;
      let shouldMutateLocalDuringPut = mutateLocalDuringPut;
      let tombstones: SyncTombstoneV3[] = (() => {
        try { return JSON.parse(snapshot.settings.sync_tombstones || '[]'); } catch { return []; }
      })();
      let episodeCompletions: EpisodeCompletion[] = snapshot.episodeCompletions;
      let collections: WatchCollection[] = snapshot.collections;
      let collectionMembers: CollectionMember[] = snapshot.collectionMembers;
      let collectionTombstones: CollectionTombstone[] = [];
      let collectionMemberTombstones: CollectionMemberTombstone[] = [];

      const bumpCollection = (collectionId: string) => {
        const index = collections.findIndex(item => item.id === collectionId);
        if (index < 0) throw new Error('collection_missing');
        collections[index] = {
          ...collections[index],
          updatedAt: new Date().toISOString(),
          rev: collections[index].rev + 1,
          revActor: 'mock-device',
        };
        snapshot.collections = collections;
        return collections[index];
      };

      const persistRuntime = () => {
        snapshot.settings.sync_outbox_v1 = JSON.stringify(outbox);
        snapshot.settings.sync_scheduler_v1 = JSON.stringify(scheduler);
        sessionStorage.setItem('__WATCHTRACKER_SYNC_RUNTIME__', JSON.stringify({ recordsGeneration, outbox, scheduler }));
        sessionStorage.setItem('__WATCHTRACKER_CONTROLLED_RECORDS__', JSON.stringify(snapshot.records));
      };

      const queueOutbox = (reason: string) => {
        const now = new Date().toISOString();
        if (!outbox.pending) {
          outbox.firstQueuedAt = now;
          outbox.reasons = [];
        }
        outbox.pending = true;
        outbox.dirtyGeneration = recordsGeneration;
        outbox.lastQueuedAt = now;
        if (!outbox.reasons.includes(reason)) outbox.reasons = [...outbox.reasons.slice(-7), reason];
        persistRuntime();
      };

      const makeRecoveryPoint = (reason: RecoveryPoint['reason']) => {
        recoverySequence += 1;
        const id = `watchtracker-recovery-test-${recoverySequence}-${reason}.db`;
        const point: RecoveryPoint = {
          id,
          createdAt: new Date(1785660000000 + recoverySequence * 1000).toISOString(),
          reason,
          databaseVersion: 18,
          recordCount: snapshot.records.length,
          sizeBytes: 4096,
          sha256: `TEST${recoverySequence}`,
          retained: false,
          integrityOk: true,
        };
        recoveryRecords[id] = structuredClone(snapshot.records);
        snapshot.recoveryPoints.unshift(point);
        return structuredClone(point);
      };

      const requireKeys = (
        command: string,
        args: Record<string, unknown>,
        required: string[],
      ) => {
        const actual = Object.keys(args).sort();
        const expected = [...required].sort();
        if (JSON.stringify(actual) !== JSON.stringify(expected)) {
          throw new Error(`${command} argument keys ${actual.join(',')} != ${expected.join(',')}`);
        }
      };

      window.__TAURI_INTERNALS__ = {
        convertFileSrc: (filePath, protocol = 'asset') =>
          `http://${protocol}.localhost/${encodeURIComponent(filePath)}`,
        invoke: async (command, rawArgs = {}) => {
          const args = structuredClone(rawArgs);
          snapshot.calls.push({
            command,
            args: command === 'webdav_request'
              ? structuredClone(args.request as Record<string, unknown>)
              : args,
          });

          switch (command) {
            case 'get_setting':
              requireKeys(command, args, ['key']);
              return snapshot.settings[args.key as string] ?? null;
            case 'get_database_compatibility':
              requireKeys(command, args, []);
              return structuredClone(databaseCompatibilityIssue);
            case 'get_all_records':
              requireKeys(command, args, []);
              if (snapshot.failRecordLoads) {
                throw new Error('injected database load failure');
              }
              return structuredClone(snapshot.records);
            case 'get_collections':
              requireKeys(command, args, []);
              return structuredClone(collections);
            case 'get_collection_members':
              requireKeys(command, args, []);
              return structuredClone(collectionMembers);
            case 'create_collection': {
              requireKeys(command, args, ['input']);
              const input = args.input as { name: string; description: string | null; sourceKind?: WatchCollection['sourceKind']; sourceKey?: string | null; collectionKind?: WatchCollection['collectionKind']; orderMode?: WatchCollection['orderMode'] };
              const now = new Date().toISOString();
              const collection: WatchCollection = {
                id: `collection-${collections.length + 1}`,
                name: input.name.trim(),
                normalizedName: input.name.trim().toLocaleLowerCase(),
                description: input.description,
                sourceKind: input.sourceKind ?? 'manual',
                sourceKey: input.sourceKey ?? null,
                collectionKind: input.collectionKind ?? (input.sourceKind === 'tmdb-tv-show' ? 'tv-series' : input.sourceKind === 'tmdb-movie-collection' ? 'movie-series' : 'manual'),
                orderMode: input.orderMode ?? (input.sourceKind && input.sourceKind !== 'manual' ? 'chronological' : 'manual'),
                createdAt: now,
                updatedAt: now,
                rev: 1,
                revActor: 'mock-device',
              };
              if (collections.some(item => item.normalizedName === collection.normalizedName)) throw new Error('collection_name_exists');
              collections = [...collections, collection];
              snapshot.collections = collections;
              recordsGeneration += 1; queueOutbox('collection-create');
              return structuredClone(collection);
            }
            case 'create_collection_for_record': {
              requireKeys(command, args, ['input', 'recordId']);
              const input = args.input as { name: string; description: string | null; sourceKind?: WatchCollection['sourceKind']; sourceKey?: string | null; collectionKind?: WatchCollection['collectionKind']; orderMode?: WatchCollection['orderMode'] };
              if (!snapshot.records.some(item => item.id === args.recordId)) throw new Error('collection_reference_invalid');
              const normalizedName = input.name.trim().toLocaleLowerCase();
              if (collections.some(item => item.normalizedName === normalizedName)) throw new Error('collection_name_duplicate');
              const now = new Date().toISOString();
              const collection: WatchCollection = { id: `collection-${collections.length + 1}`, name: input.name.trim(), normalizedName, description: input.description, sourceKind: input.sourceKind ?? 'manual', sourceKey: input.sourceKey ?? null, collectionKind: input.collectionKind ?? 'manual', orderMode: input.orderMode ?? 'manual', createdAt: now, updatedAt: now, rev: 1, revActor: 'mock-device' };
              const member: CollectionMember = { id: `member-${collection.id}-${args.recordId}`, collectionId: collection.id, recordId: args.recordId as string, position: 0, sourceKind: 'manual', createdAt: now, updatedAt: now, rev: 1, revActor: 'mock-device' };
              collections = [...collections, collection]; collectionMembers = [...collectionMembers, member];
              snapshot.collections = collections; snapshot.collectionMembers = collectionMembers;
              recordsGeneration += 1; queueOutbox('collection-create-for-record');
              return structuredClone(collection);
            }
            case 'update_collection': {
              requireKeys(command, args, ['id', 'input']);
              const input = args.input as { name: string; description: string | null; expectedRev: number; orderMode?: WatchCollection['orderMode']; sourceKind?: WatchCollection['sourceKind']; sourceKey?: string | null; collectionKind?: WatchCollection['collectionKind'] };
              const index = collections.findIndex(item => item.id === args.id);
              if (index < 0) throw new Error('collection_missing');
              if (collections[index].rev !== input.expectedRev) throw new Error('stale_collection');
              collections[index] = {
                ...collections[index],
                name: input.name.trim(),
                normalizedName: input.name.trim().toLocaleLowerCase(),
                description: input.description,
                sourceKind: input.sourceKind ?? collections[index].sourceKind,
                sourceKey: input.sourceKey ?? collections[index].sourceKey,
                collectionKind: input.collectionKind ?? collections[index].collectionKind,
                orderMode: input.orderMode ?? collections[index].orderMode,
                updatedAt: new Date().toISOString(),
                rev: input.expectedRev + 1,
                revActor: 'mock-device',
              };
              snapshot.collections = collections;
              recordsGeneration += 1; queueOutbox('collection-update');
              return structuredClone(collections[index]);
            }
            case 'apply_collection_suggestion': {
              requireKeys(command, args, ['input']);
              const input = args.input as { name: string; sourceKind: 'tmdb-movie-collection' | 'tmdb-tv-show'; sourceKey: string; recordIds: string[]; targetCollectionId: string | null; expectedRev: number | null };
              let target = input.targetCollectionId ? collections.find(item => item.id === input.targetCollectionId) : undefined;
              const now = new Date().toISOString();
              const isNew = !target;
              if (target && target.rev !== input.expectedRev) throw new Error('stale_collection');
              if (!target) {
                target = {
                  id: `collection-${collections.length + 1}`, name: input.name.trim(), normalizedName: input.name.trim().toLocaleLowerCase(), description: null,
                  sourceKind: input.sourceKind, sourceKey: input.sourceKey,
                  collectionKind: input.sourceKind === 'tmdb-tv-show' ? 'tv-series' : 'movie-series', orderMode: 'chronological',
                  createdAt: now, updatedAt: now, rev: 1, revActor: 'mock-device',
                };
                collections.push(target);
              } else {
                target.sourceKind = input.sourceKind;
                target.sourceKey = input.sourceKey;
                target.collectionKind = input.sourceKind === 'tmdb-tv-show' ? 'tv-series' : 'movie-series';
                target.orderMode = 'chronological';
              }
              let position = collectionMembers.filter(item => item.collectionId === target.id).length;
              for (const recordId of input.recordIds) {
                if (collectionMembers.some(item => item.collectionId === target!.id && item.recordId === recordId)) continue;
                collectionMembers.push({ id: `member-${target.id}-${recordId}`, collectionId: target.id, recordId, position: position++, sourceKind: 'tmdb', createdAt: now, updatedAt: now, rev: 1, revActor: 'mock-device' });
              }
              if (!isNew) bumpCollection(target.id);
              snapshot.collections = collections;
              snapshot.collectionMembers = collectionMembers;
              recordsGeneration += 1; queueOutbox('collection-suggestion-apply');
              return structuredClone(target);
            }
            case 'complete_movie_collection': {
              requireKeys(command, args, ['input']);
              const input = args.input as { collectionId: string; expectedRev: number; matches: Array<{ recordId: string; expectedRev: number; tmdbId: number; imdbId: string | null }>; newRecords: WatchRecord[]; fillMissingIdentity: boolean };
              const collection = collections.find(item => item.id === input.collectionId);
              if (!collection || collection.rev !== input.expectedRev) throw new Error('stale_collection');
              const normalizeImdb = (value: string | null | undefined) => /^tt\d+$/.test(value?.trim().toLowerCase() ?? '') ? value!.trim().toLowerCase() : null;
              const memberIds = new Set(collectionMembers.filter(item => item.collectionId === collection.id).map(item => item.recordId));
              const createdRecordIds: string[] = [];
              const reusedRecordIds: string[] = [];
              const identityUpdatedRecordIds: string[] = [];
              const now = new Date().toISOString();
              for (const match of input.matches) {
                const record = snapshot.records.find(item => item.id === match.recordId);
                if (!record || (record.rev ?? 0) !== match.expectedRev) throw new Error('stale_record');
                if (input.fillMissingIdentity && (!record.tmdbMediaKind || !record.tmdbId || !record.seriesRecordKind)) {
                  record.tmdbMediaKind ||= 'movie'; record.tmdbId ||= match.tmdbId; record.seriesRecordKind ||= 'single-work';
                  record.rev = (record.rev ?? 0) + 1; record.updatedAt = now; identityUpdatedRecordIds.push(record.id);
                }
                if (!memberIds.has(record.id)) { memberIds.add(record.id); reusedRecordIds.push(record.id); }
              }
              for (const proposed of input.newRecords) {
                const imdb = normalizeImdb(proposed.imdbId);
                const existing = snapshot.records.find(item => item.tmdbMediaKind === 'movie' && item.tmdbId === proposed.tmdbId || !!imdb && normalizeImdb(item.imdbId) === imdb);
                if (existing?.tmdbMediaKind === 'tv' || existing?.tmdbMediaKind === 'tv-season') throw new Error('movie_identity_conflict');
                if (existing) {
                  if (input.fillMissingIdentity && (!existing.tmdbMediaKind || !existing.tmdbId || !existing.seriesRecordKind)) {
                    existing.tmdbMediaKind ||= 'movie'; existing.tmdbId ||= proposed.tmdbId; existing.seriesRecordKind ||= 'single-work';
                    existing.rev = (existing.rev ?? 0) + 1; existing.updatedAt = now; identityUpdatedRecordIds.push(existing.id);
                  }
                  memberIds.add(existing.id); reusedRecordIds.push(existing.id);
                }
                else { snapshot.records.push(proposed); memberIds.add(proposed.id); createdRecordIds.push(proposed.id); }
              }
              let position = collectionMembers.filter(item => item.collectionId === collection.id).length;
              for (const recordId of memberIds) if (!collectionMembers.some(item => item.collectionId === collection.id && item.recordId === recordId)) collectionMembers.push({ id: `member-${collection.id}-${recordId}`, collectionId: collection.id, recordId, position: position++, sourceKind: 'tmdb', createdAt: now, updatedAt: now, rev: 1, revActor: 'mock-device' });
              bumpCollection(collection.id); snapshot.collections = collections; snapshot.collectionMembers = collectionMembers;
              recordsGeneration += 1; queueOutbox('movie-collection-completion');
              return { createdRecordIds, reusedRecordIds, identityUpdatedRecordIds };
            }
            case 'delete_collection': {
              requireKeys(command, args, ['expectedRev', 'id']);
              const collection = collections.find(item => item.id === args.id);
              if (!collection) throw new Error('collection_missing');
              if (collection.rev !== args.expectedRev) throw new Error('stale_collection');
              const now = new Date().toISOString();
              collectionTombstones.push({ id: collection.id, deletedAt: now, rev: collection.rev + 1, revActor: 'mock-device' });
              for (const member of collectionMembers.filter(item => item.collectionId === collection.id)) {
                collectionMemberTombstones.push({ id: member.id, collectionId: member.collectionId, recordId: member.recordId, deletedAt: now, rev: member.rev + 1, revActor: 'mock-device' });
              }
              collections = collections.filter(item => item.id !== collection.id);
              collectionMembers = collectionMembers.filter(item => item.collectionId !== collection.id);
              snapshot.collections = collections;
              snapshot.collectionMembers = collectionMembers;
              recordsGeneration += 1; queueOutbox('collection-delete');
              return null;
            }
            case 'add_collection_members': {
              requireKeys(command, args, ['collectionId', 'expectedRev', 'recordIds', 'sourceKind']);
              const collection = collections.find(item => item.id === args.collectionId);
              if (!collection) throw new Error('collection_missing');
              if (collection.rev !== args.expectedRev) throw new Error('stale_collection');
              let position = collectionMembers.filter(item => item.collectionId === collection.id).length;
              const now = new Date().toISOString();
              for (const recordId of args.recordIds as string[]) {
                if (collectionMembers.some(item => item.collectionId === collection.id && item.recordId === recordId)) continue;
                const member: CollectionMember = {
                  id: `member-${collection.id}-${recordId}`,
                  collectionId: collection.id,
                  recordId,
                  position: position++,
                  sourceKind: args.sourceKind as 'manual' | 'tmdb',
                  createdAt: now,
                  updatedAt: now,
                  rev: 1,
                  revActor: 'mock-device',
                };
                collectionMembers.push(member);
              }
              bumpCollection(collection.id);
              snapshot.collectionMembers = collectionMembers;
              recordsGeneration += 1; queueOutbox('collection-member-add');
              return structuredClone(collectionMembers.filter(item => item.collectionId === collection.id));
            }
            case 'remove_collection_member': {
              requireKeys(command, args, ['collectionId', 'expectedRev', 'recordId']);
              const collection = collections.find(item => item.id === args.collectionId);
              if (!collection) throw new Error('collection_missing');
              const member = collectionMembers.find(item => item.collectionId === collection.id && item.recordId === args.recordId);
              if (!member) throw new Error('collection_member_not_found');
              if (member.rev !== args.expectedRev) throw new Error('stale_collection_member');
              if (member) {
                collectionMemberTombstones.push({ id: member.id, collectionId: member.collectionId, recordId: member.recordId, deletedAt: new Date().toISOString(), rev: member.rev + 1, revActor: 'mock-device' });
                collectionMembers = collectionMembers.filter(item => item.id !== member.id);
              }
              collectionMembers.filter(item => item.collectionId === collection.id).sort((a, b) => a.position - b.position).forEach((item, index) => { item.position = index; });
              bumpCollection(collection.id);
              snapshot.collectionMembers = collectionMembers;
              recordsGeneration += 1; queueOutbox('collection-member-remove');
              return null;
            }
            case 'reorder_collection_members': {
              requireKeys(command, args, ['collectionId', 'expectedRev', 'recordIds']);
              const collection = collections.find(item => item.id === args.collectionId);
              if (!collection) throw new Error('collection_missing');
              if (collection.rev !== args.expectedRev) throw new Error('stale_collection');
              (args.recordIds as string[]).forEach((recordId, position) => {
                const member = collectionMembers.find(item => item.collectionId === collection.id && item.recordId === recordId);
                if (member) { member.position = position; member.updatedAt = new Date().toISOString(); member.rev += 1; }
              });
              bumpCollection(collection.id);
              snapshot.collectionMembers = collectionMembers;
              recordsGeneration += 1; queueOutbox('collection-member-reorder');
              return structuredClone(collectionMembers.filter(item => item.collectionId === collection.id).sort((a, b) => a.position - b.position));
            }
            case 'insert_record': {
              requireKeys(command, args, ['r']);
              const record = structuredClone(args.r as WatchRecord);
              const existingIndex = snapshot.records.findIndex(item => item.id === record.id);
              const previous = existingIndex >= 0 ? snapshot.records[existingIndex] : undefined;
              const persisted: WatchRecord = {
                ...record,
                updatedAt: new Date().toISOString(),
                rev: (previous?.rev ?? 0) + 1,
                revActor: 'mock-device',
              };
              if (existingIndex >= 0) snapshot.records[existingIndex] = persisted;
              else snapshot.records.unshift(persisted);
              recordsGeneration += 1;
              queueOutbox('record-insert');
              return structuredClone(persisted);
            }
            case 'update_record': {
              requireKeys(command, args, ['id', 'updates']);
              const id = args.id as string;
              if ((args.updates as Partial<WatchRecord>).episodeRuntime === 0) {
                throw new Error('Invalid episodeRuntime: must be greater than zero');
              }
              if ((remainingUpdateFailures[id] ?? 0) > 0) {
                remainingUpdateFailures[id] -= 1;
                throw new Error('Injected update failure');
              }
              const index = snapshot.records.findIndex((record) => record.id === id);
              if (index < 0) throw new Error('Record not found');
              const previous = snapshot.records[index];
              const persisted: WatchRecord = {
                ...previous,
                ...(args.updates as Partial<WatchRecord>),
                updatedAt: new Date().toISOString(),
                rev: (previous.rev ?? 0) + 1,
                revActor: 'mock-device',
              };
              snapshot.records[index] = persisted;
              recordsGeneration += 1;
              queueOutbox('record-update');
              return structuredClone(persisted);
            }
            case 'complete_missing_tmdb_identity': {
              requireKeys(command, args, ['input']);
              const input = args.input as { recordId: string; expectedRev: number; expectedImdbId: string; tmdbMediaKind: WatchRecord['tmdbMediaKind']; tmdbId: number; tmdbParentId: number | null; tmdbSeasonNumber: number | null; seriesRecordKind: WatchRecord['seriesRecordKind'] };
              const index = snapshot.records.findIndex(record => record.id === input.recordId);
              if (index < 0) throw new Error('record_not_found');
              const previous = snapshot.records[index];
              if ((previous.rev ?? 0) !== input.expectedRev) throw new Error('stale_record');
              if (previous.isLocked) throw new Error('record_locked');
              if (previous.imdbId?.trim().toLowerCase() !== input.expectedImdbId.trim().toLowerCase()) throw new Error('tmdb_identity_imdb_changed');
              const pairs = [['tmdbMediaKind', input.tmdbMediaKind], ['tmdbId', input.tmdbId], ['tmdbParentId', input.tmdbParentId], ['tmdbSeasonNumber', input.tmdbSeasonNumber], ['seriesRecordKind', input.seriesRecordKind]] as const;
              if (pairs.some(([field, value]) => previous[field] != null && previous[field] !== value)) throw new Error('tmdb_identity_conflict');
              const persisted = { ...previous };
              for (const [field, value] of pairs) if (persisted[field] == null && value != null) Object.assign(persisted, { [field]: value });
              persisted.rev = (previous.rev ?? 0) + 1; persisted.revActor = 'mock-device'; persisted.updatedAt = new Date().toISOString();
              snapshot.records[index] = persisted;
              recordsGeneration += 1; queueOutbox('tmdb-identity-completion');
              return structuredClone(persisted);
            }
            case 'delete_record':
              requireKeys(command, args, ['id']);
              snapshot.records = snapshot.records.filter((record) => record.id !== args.id);
              episodeCompletions = episodeCompletions.filter(item => item.recordId !== args.id);
              snapshot.episodeCompletions = episodeCompletions;
              collectionMembers = collectionMembers.filter(item => item.recordId !== args.id);
              snapshot.collectionMembers = collectionMembers;
              tombstones = tombstones.filter(item => item.id !== args.id);
              tombstones.push({ id: args.id as string, deletedAt: new Date().toISOString(), rev: 0, revActor: 'mock-device' });
              recordsGeneration += 1;
              queueOutbox('record-delete');
              return null;
            case 'get_episode_tracking': {
              requireKeys(command, args, ['recordId']);
              const record = snapshot.records.find(item => item.id === args.recordId);
              if (!record) throw new Error('episode_record_missing');
              return { record: structuredClone(record), completions: structuredClone(episodeCompletions.filter(item => item.recordId === record.id)) };
            }
            case 'get_all_episode_completions':
              requireKeys(command, args, []);
              return structuredClone(episodeCompletions);
            case 'replace_library': {
              requireKeys(command, args, ['episodeCompletions', 'records']);
              makeRecoveryPoint('import');
              snapshot.records = structuredClone(args.records as WatchRecord[]);
              episodeCompletions = structuredClone(args.episodeCompletions as EpisodeCompletion[]);
              recordsGeneration += 1; queueOutbox('library-import');
              return null;
            }
            case 'replace_library_v3': {
              requireKeys(command, args, ['collectionMembers', 'collections', 'episodeCompletions', 'records']);
              makeRecoveryPoint('import');
              snapshot.records = structuredClone(args.records as WatchRecord[]);
              episodeCompletions = structuredClone(args.episodeCompletions as EpisodeCompletion[]);
              collections = structuredClone(args.collections as WatchCollection[]);
              collectionMembers = structuredClone(args.collectionMembers as CollectionMember[]);
              snapshot.episodeCompletions = episodeCompletions;
              snapshot.collections = collections;
              snapshot.collectionMembers = collectionMembers;
              recordsGeneration += 1; queueOutbox('library-import');
              return null;
            }
            case 'enable_episode_tracking': {
              requireKeys(command, args, ['expectedRev', 'initialNextEpisode', 'recordId']);
              const index = snapshot.records.findIndex(item => item.id === args.recordId);
              if (index < 0) throw new Error('episode_record_missing');
              const previous = snapshot.records[index];
              if ((previous.rev ?? 0) !== args.expectedRev) throw new Error('stale_episode_progress');
              if (previous.status === '已看') throw new Error('episode_record_already_completed');
              snapshot.records[index] = { ...previous, episodeTrackingEnabled: true, nextEpisode: args.initialNextEpisode as number, status: '在看', rev: (previous.rev ?? 0) + 1, revActor: 'mock-device' };
              recordsGeneration += 1; queueOutbox('episode-tracking-enable');
              return { record: structuredClone(snapshot.records[index]), completions: [] };
            }
            case 'set_next_episode': {
              requireKeys(command, args, ['expectedRev', 'nextEpisode', 'recordId']);
              const index = snapshot.records.findIndex(item => item.id === args.recordId);
              if (index < 0) throw new Error('episode_record_missing');
              const previous = snapshot.records[index];
              if ((previous.rev ?? 0) !== args.expectedRev) throw new Error('stale_episode_progress');
              const current = previous.nextEpisode;
              const target = args.nextEpisode as number | null;
              if (typeof current === 'number' && (target === null || target > current)) {
                const boundary = target === null ? (previous.totalEpisodes as number) : target - 1;
                for (let episode = current; episode <= boundary; episode += 1) {
                  const existing = episodeCompletions.find(item => item.recordId === previous.id && item.episodeNumber === episode);
                  const completedAt = episode === boundary ? new Date().toISOString() : null;
                  if (!existing) episodeCompletions.push({ id: `${previous.id}-${episode}`, recordId: previous.id, episodeNumber: episode, completedAt, createdAt: new Date().toISOString(), updatedAt: new Date().toISOString(), rev: 1, revActor: 'mock-device' });
                  else if (existing.completedAt === null && completedAt) existing.completedAt = completedAt;
                }
              }
              snapshot.records[index] = { ...previous, nextEpisode: target, status: target === null ? '已看' : '在看', rev: (previous.rev ?? 0) + 1, revActor: 'mock-device' };
              recordsGeneration += 1; queueOutbox('episode-progress');
              return { record: structuredClone(snapshot.records[index]), completions: structuredClone(episodeCompletions.filter(item => item.recordId === previous.id)) };
            }
            case 'replace_all_records':
              requireKeys(command, args, ['reason', 'records']);
              makeRecoveryPoint(args.reason as 'import' | 'sync');
              {
                const tracking = new Map(snapshot.records.map(item => [item.id, {
                  episodeTrackingEnabled: item.episodeTrackingEnabled,
                  nextEpisode: item.nextEpisode,
                }]));
                snapshot.records = structuredClone(args.records as WatchRecord[]).map(item => ({
                  ...item,
                  ...(tracking.get(item.id) ?? {}),
                }));
                const retainedIds = new Set(snapshot.records.map(item => item.id));
                episodeCompletions = episodeCompletions.filter(item => retainedIds.has(item.recordId));
                snapshot.episodeCompletions = episodeCompletions;
              }
              recordsGeneration += 1;
              queueOutbox('records-replace');
              return null;
            case 'get_sync_snapshot': {
              requireKeys(command, args, []);
              const parse = (key: string) => {
                const raw = snapshot.settings[key];
                return raw ? JSON.parse(raw) : null;
              };
              return {
                targetId: activeTargetId,
                targetEpoch: activeTargetId ? targetEpoch : null,
                records: structuredClone(snapshot.records),
                tombstones: structuredClone(tombstones),
                episodeCompletions: structuredClone(episodeCompletions),
                collections: structuredClone(collections),
                collectionMembers: structuredClone(collectionMembers),
                collectionTombstones: structuredClone(collectionTombstones),
                collectionMemberTombstones: structuredClone(collectionMemberTombstones),
                recordsGeneration,
                baseline: parse('sync_v3_baseline'),
                deviceId: snapshot.settings.sync_device_id_v1 || 'mock-device',
                conflicts: parse('sync_v3_conflicts') || [],
                remoteEtag: snapshot.settings.sync_v3_remote_etag || null,
                lastCommit: parse('sync_v3_last_commit'),
                v2SourceFingerprint: snapshot.settings.sync_v2_source_fingerprint || null,
                outbox: structuredClone(outbox),
                scheduler: structuredClone(scheduler),
                staging: parse('sync_staging_v1') || { version: 2, entries: [] },
                publishIntent: parse('sync_publish_intent_v1'),
              };
            }
            case 'get_sync_runtime_state':
              requireKeys(command, args, []);
              return {
                targetId: activeTargetId, targetEpoch: activeTargetId ? targetEpoch : null,
                outbox: structuredClone(outbox), scheduler: structuredClone(scheduler),
                conflictCount: JSON.parse(snapshot.settings.sync_v3_conflicts || '[]').length,
                lastCommit: snapshot.settings.sync_v3_last_commit ? JSON.parse(snapshot.settings.sync_v3_last_commit) : null,
                stagedCount: JSON.parse(snapshot.settings.sync_staging_v1 || '{"entries":[]}').entries.length,
                publishPending: Boolean(snapshot.settings.sync_publish_intent_v1),
              };
            case 'set_auto_sync_paused':
              requireKeys(command, args, ['paused', 'targetEpoch', 'targetId']);
              scheduler.paused = args.paused as boolean;
              if (!scheduler.paused) scheduler.nextAttemptAt = null;
              persistRuntime();
              return {
                targetId: activeTargetId, targetEpoch: activeTargetId ? targetEpoch : null,
                outbox: structuredClone(outbox), scheduler: structuredClone(scheduler),
                conflictCount: JSON.parse(snapshot.settings.sync_v3_conflicts || '[]').length,
                lastCommit: snapshot.settings.sync_v3_last_commit ? JSON.parse(snapshot.settings.sync_v3_last_commit) : null,
                stagedCount: JSON.parse(snapshot.settings.sync_staging_v1 || '{"entries":[]}').entries.length,
                publishPending: Boolean(snapshot.settings.sync_publish_intent_v1),
              };
            case 'record_sync_failure':
              requireKeys(command, args, ['code', 'nextAttemptAt', 'targetEpoch', 'targetId']);
              scheduler.consecutiveFailures += 1;
              scheduler.lastAttemptAt = new Date().toISOString();
              scheduler.lastErrorCode = args.code as string;
              scheduler.nextAttemptAt = args.nextAttemptAt as string | null;
              persistRuntime();
              return {
                targetId: activeTargetId, targetEpoch: activeTargetId ? targetEpoch : null,
                outbox: structuredClone(outbox), scheduler: structuredClone(scheduler),
                conflictCount: JSON.parse(snapshot.settings.sync_v3_conflicts || '[]').length,
                lastCommit: snapshot.settings.sync_v3_last_commit ? JSON.parse(snapshot.settings.sync_v3_last_commit) : null,
                stagedCount: JSON.parse(snapshot.settings.sync_staging_v1 || '{"entries":[]}').entries.length,
                publishPending: Boolean(snapshot.settings.sync_publish_intent_v1),
              };
            case 'prepare_sync_publish_intent': {
              requireKeys(command, args, ['input']);
              const input = args.input as {
                targetId: string | null; targetEpoch: number | null;
                commitId: string; previousCommitId: string | null;
                expectedGeneration: number; payloadFingerprint: string;
              };
              if (input.targetId !== activeTargetId || input.targetEpoch !== (activeTargetId ? targetEpoch : null)) throw new Error('stale_sync_target');
              if (input.expectedGeneration !== recordsGeneration) throw new Error('stale_local_snapshot');
              const staging = JSON.parse(snapshot.settings.sync_staging_v1 || '{"entries":[]}') as {
                entries: Array<{ id: string; lastGeneration: number }>;
              };
              const intent = {
                version: 1, ...input,
                includedEntries: staging.entries
                  .filter(entry => entry.lastGeneration <= input.expectedGeneration)
                  .map(entry => ({ id: entry.id, lastGeneration: entry.lastGeneration })),
                createdAt: new Date().toISOString(),
              };
              snapshot.settings.sync_publish_intent_v1 = JSON.stringify(intent);
              return structuredClone(intent);
            }
            case 'commit_sync_result': {
              requireKeys(command, args, ['input']);
              const input = args.input as {
                targetId: string | null;
                targetEpoch: number | null;
                expectedGeneration: number;
                records: WatchRecord[];
                tombstones: SyncTombstoneV3[];
                episodeCompletions: EpisodeCompletion[];
                collections: WatchCollection[];
                collectionMembers: CollectionMember[];
                collectionTombstones: CollectionTombstone[];
                collectionMemberTombstones: CollectionMemberTombstone[];
                baseline: SyncPayloadV3;
                conflicts: SyncConflictV3[];
                remoteEtag: string;
                lastCommit: unknown;
                v2SourceFingerprint: string | null;
                acknowledgeOutbox: boolean;
              };
              if (input.targetId !== activeTargetId || input.targetEpoch !== (activeTargetId ? targetEpoch : null)) throw new Error('stale_sync_target');
              if (input.expectedGeneration !== recordsGeneration) throw new Error('stale_local_snapshot');
              const businessStateChanged = JSON.stringify(snapshot.records) !== JSON.stringify(input.records)
                || JSON.stringify(tombstones) !== JSON.stringify(input.tombstones)
                || JSON.stringify(episodeCompletions) !== JSON.stringify(input.episodeCompletions)
                || JSON.stringify(collections) !== JSON.stringify(input.collections)
                || JSON.stringify(collectionMembers) !== JSON.stringify(input.collectionMembers)
                || JSON.stringify(collectionTombstones) !== JSON.stringify(input.collectionTombstones)
                || JSON.stringify(collectionMemberTombstones) !== JSON.stringify(input.collectionMemberTombstones);
              if (businessStateChanged) makeRecoveryPoint('sync');
              snapshot.records = structuredClone(input.records);
              tombstones = structuredClone(input.tombstones);
              episodeCompletions = structuredClone(input.episodeCompletions);
              collections = structuredClone(input.collections);
              collectionMembers = structuredClone(input.collectionMembers);
              collectionTombstones = structuredClone(input.collectionTombstones);
              collectionMemberTombstones = structuredClone(input.collectionMemberTombstones);
              snapshot.episodeCompletions = episodeCompletions;
              snapshot.collections = collections;
              snapshot.collectionMembers = collectionMembers;
              snapshot.settings.sync_tombstones = JSON.stringify(tombstones);
              snapshot.settings.sync_v3_baseline = JSON.stringify(input.baseline);
              snapshot.settings.sync_v3_conflicts = JSON.stringify(input.conflicts);
              snapshot.settings.sync_v3_remote_etag = input.remoteEtag;
              snapshot.settings.sync_v3_last_commit = JSON.stringify(input.lastCommit);
              const intent = snapshot.settings.sync_publish_intent_v1
                ? JSON.parse(snapshot.settings.sync_publish_intent_v1) as { commitId: string }
                : null;
              if (intent?.commitId === input.baseline.commitId) delete snapshot.settings.sync_publish_intent_v1;
              if (input.v2SourceFingerprint) snapshot.settings.sync_v2_source_fingerprint = input.v2SourceFingerprint;
              if (businessStateChanged) recordsGeneration += 1;
              if (input.acknowledgeOutbox && outbox.pending && outbox.dirtyGeneration <= input.expectedGeneration) {
                outbox.pending = false;
                outbox.reasons = [];
                outbox.firstQueuedAt = null;
                outbox.lastQueuedAt = null;
              }
              if (input.acknowledgeOutbox) {
                outbox.dirtyGeneration = recordsGeneration;
                const now = new Date().toISOString();
                scheduler = {
                  ...scheduler, consecutiveFailures: 0, nextAttemptAt: null, lastAttemptAt: now,
                  lastSuccessAt: now, lastErrorCode: null, lastRemoteCheckAt: now,
                };
              }
              persistRuntime();
              return { recordsGeneration, recordCount: snapshot.records.length };
            }
            case 'resolve_sync_conflict': {
              requireKeys(command, args, ['id', 'resolution', 'targetEpoch', 'targetId']);
              const conflicts = JSON.parse(snapshot.settings.sync_v3_conflicts || '[]') as SyncConflictV3[];
              const conflict = conflicts.find(item => item.id === args.id);
              if (!conflict) throw new Error('Sync conflict not found');
              const resolution = args.resolution as 'local' | 'remote' | 'keep' | 'delete';
              const selected = resolution === 'local' ? conflict.local
                : resolution === 'remote' ? conflict.remote
                  : resolution === 'keep' ? (conflict.local || conflict.remote) : null;
              snapshot.records = snapshot.records.filter(item => item.id !== conflict.id);
              tombstones = tombstones.filter(item => item.id !== conflict.id);
              if (selected) snapshot.records.unshift(structuredClone(selected));
              else tombstones.push({ id: conflict.id, deletedAt: new Date().toISOString(), rev: 0, revActor: 'mock-device' });
              snapshot.settings.sync_v3_conflicts = JSON.stringify(conflicts.filter(item => item.id !== conflict.id));
              recordsGeneration += 1;
              queueOutbox('conflict-resolution');
              return null;
            }
            case 'create_recovery_point':
              requireKeys(command, args, ['reason']);
              return makeRecoveryPoint(args.reason as RecoveryPoint['reason']);
            case 'list_recovery_points': {
              requireKeys(command, args, []);
              const totalBytes = snapshot.recoveryPoints.reduce((sum, point) => sum + point.sizeBytes, 0);
              return {
                points: structuredClone(snapshot.recoveryPoints),
                totalBytes,
                capacityBytes: 500 * 1024 * 1024,
                capacityExceeded: totalBytes > 500 * 1024 * 1024,
              };
            }
            case 'set_recovery_point_retained': {
              requireKeys(command, args, ['id', 'retained']);
              const point = snapshot.recoveryPoints.find(item => item.id === args.id);
              if (!point) throw new Error('Recovery point not found');
              point.retained = args.retained as boolean;
              return null;
            }
            case 'delete_recovery_point':
              requireKeys(command, args, ['id']);
              snapshot.recoveryPoints = snapshot.recoveryPoints.filter(point => point.id !== args.id);
              delete recoveryRecords[args.id as string];
              return null;
            case 'restore_recovery_point': {
              requireKeys(command, args, ['id']);
              const restored = recoveryRecords[args.id as string];
              if (!restored) throw new Error('Recovery point not found');
              const preRestore = makeRecoveryPoint('pre-restore');
              snapshot.records = structuredClone(restored);
              recordsGeneration += 1;
              queueOutbox('recovery-restore');
              return { preRestorePointId: preRestore.id, recordCount: snapshot.records.length };
            }
            case 'open_backup_directory':
              requireKeys(command, args, []);
              return null;
            case 'get_active_sync_connection': {
              requireKeys(command, args, []);
              if (!activeTargetId || !snapshot.settings.webdav_creds) return null;
              const decrypted = String(snapshot.settings.webdav_creds).replace(/^encrypted:/, '');
              const separator = decrypted.indexOf(':');
              return {
                targetId: activeTargetId, targetEpoch, url: snapshot.settings.webdav_url,
                username: decrypted.slice(0, separator), credentialAvailable: true,
              };
            }
            case 'get_sync_targets':
              requireKeys(command, args, []);
              return {
                version: 1, activeTargetId, targetEpoch,
                targets: activeTargetId ? [{ id: activeTargetId, normalizedUrl: snapshot.settings.webdav_url, username: 'user', createdAt: new Date().toISOString(), lastActivatedAt: new Date().toISOString() }] : [],
              };
            case 'activate_sync_target': {
              requireKeys(command, args, ['input']);
              const input = args.input as { url: string; username: string; password: string };
              if (!activeTargetId) targetEpoch += 1;
              activeTargetId = 'a'.repeat(64);
              snapshot.settings.webdav_url = input.url;
              snapshot.settings.webdav_creds = `encrypted:${input.username}:${input.password}`;
              return { version: 1, activeTargetId, targetEpoch, targets: [{ id: activeTargetId, normalizedUrl: input.url, username: input.username, createdAt: new Date().toISOString(), lastActivatedAt: new Date().toISOString() }] };
            }
            case 'disconnect_sync_target':
              requireKeys(command, args, []);
              activeTargetId = null;
              targetEpoch += 1;
              snapshot.settings.webdav_creds = '';
              return { version: 1, activeTargetId, targetEpoch, targets: [] };
            case 'set_setting':
              requireKeys(command, args, ['key', 'value']);
              if (failSettingWrites) throw new Error('Injected setting write failure');
              snapshot.settings[args.key as string] = args.value as string;
              return true;
            case 'get_tmdb_credential_status':
              requireKeys(command, args, []);
              return { available: Boolean(snapshot.settings.tmdb_api_key), state: snapshot.settings.tmdb_api_key ? 'protected' : 'missing' };
            case 'save_tmdb_credential':
              requireKeys(command, args, ['secret']);
              snapshot.settings.tmdb_api_key = 'wincred:v1';
              return { available: true, state: 'protected' };
            case 'clear_tmdb_credential':
              requireKeys(command, args, []);
              snapshot.settings.tmdb_api_key = '';
              return { available: false, state: 'missing' };
            case 'search_tmdb':
              requireKeys(command, args, ['language', 'proxy', 'query']);
              if (tmdbDelayMs > 0) await new Promise(resolve => setTimeout(resolve, tmdbDelayMs));
              return { results: structuredClone(tmdbSearchResults) };
            case 'get_tmdb_detail':
              requireKeys(command, args, ['id', 'language', 'mediaType', 'proxy']);
              if (tmdbDelayMs > 0) await new Promise(resolve => setTimeout(resolve, tmdbDelayMs));
              return structuredClone(tmdbDetails[`${args.mediaType}:${args.id}`] ?? tmdbDetail);
            case 'get_tmdb_season_detail':
              requireKeys(command, args, ['language', 'proxy', 'seasonNumber', 'seriesId']);
              if (tmdbDelayMs > 0) await new Promise(resolve => setTimeout(resolve, tmdbDelayMs));
              return structuredClone(tmdbSeasonDetails[`${args.seriesId}:${args.seasonNumber}:${args.language}`] ?? {});
            case 'download_poster':
              requireKeys(command, args, ['path', 'proxy', 'size']);
              return { status: 'downloaded', fileName: `${args.size === 'w92' ? 'w92_' : ''}${String(args.path).replace(/^\//, '')}` };
            case 'get_poster_cache_stats':
              requireKeys(command, args, []);
              return { totalBytes: 0, validCount: 0, referencedCount: 0, orphanCount: 0, invalidCount: 0, temporaryCount: 0, capacityBytes: 524288000, capacityExceeded: false };
            case 'clean_poster_cache':
              requireKeys(command, args, ['mode']);
              return { totalBytes: 0, validCount: 0, referencedCount: 0, orphanCount: 0, invalidCount: 0, temporaryCount: 0, capacityBytes: 524288000, capacityExceeded: false };
            case 'webdav_request':
            case 'probe_webdav_request':
              requireKeys(command, args, ['request']);
              {
              const request = args.request as Record<string, unknown>;
              if (remainingWebdavFailures > 0) {
                remainingWebdavFailures -= 1;
                return { status: webdavFailureStatus, body: null, etag: null };
              }
              if (request.method === 'MKCOL') return { status: 405, body: null, etag: null };
              if (request.method === 'PROPFIND') {
                if (!snapshot.webdavV3Remote) return { status: 404, body: null, etag: null, text: null };
                const value = v3Etag ? `<d:multistatus xmlns:d="DAV:"><d:response><d:propstat><d:prop><d:getetag>${v3Etag.replaceAll('&', '&amp;').replaceAll('"', '&quot;')}</d:getetag></d:prop></d:propstat></d:response></d:multistatus>` : '<d:multistatus xmlns:d="DAV:" />';
                return { status: 207, body: null, etag: null, text: value };
              }
              if (request.method === 'GET') {
                if (String(request.url).endsWith('records-v3.json')) {
                  return snapshot.webdavV3Remote
                    ? { status: 200, body: structuredClone(snapshot.webdavV3Remote), etag: omitGetEtag ? null : v3Etag }
                    : { status: 404, body: null, etag: null };
                }
                return { status: 200, body: structuredClone(webdavRemote), etag: '"legacy-1"' };
              }
              if (request.method === 'PUT' && String(request.url).endsWith('records-v3.json')) {
                if (remainingPreconditionFailures > 0) {
                  remainingPreconditionFailures -= 1;
                  if (rotateEtagOnPreconditionFailure) v3Etag = `"external-${remainingPreconditionFailures}"`;
                  return { status: 412, body: null, etag: v3Etag };
                }
                const normalizedV3Etag = v3Etag && !v3Etag.includes('"') ? `"${v3Etag}"` : v3Etag;
                const existingPreconditionMatches = request.ifMatch === normalizedV3Etag || request.ifDavEtag === normalizedV3Etag;
                if (snapshot.webdavV3Remote && !existingPreconditionMatches) return { status: 412, body: null, etag: v3Etag };
                if (!snapshot.webdavV3Remote && request.ifNoneMatch !== '*') return { status: 412, body: null, etag: null };
                snapshot.webdavV3Remote = JSON.parse(String(request.body));
                v3Etag = `"v3-${snapshot.webdavV3Remote?.revision ?? 1}"`;
                if (shouldMutateLocalDuringPut) {
                  shouldMutateLocalDuringPut = false;
                  recordsGeneration += 1;
                  if (snapshot.records[0]) snapshot.records[0].notes = 'edited during sync';
                  queueOutbox('record-update');
                }
                return { status: 204, body: null, etag: omitPutEtag ? null : v3Etag };
              }
              return { status: 405, body: null, etag: null };
              }
            case 'vacuum_db':
              requireKeys(command, args, []);
              return null;
            default:
              throw new Error(`Unhandled mock IPC command: ${command}`);
          }
        },
      };
    },
    {
      records: options.records ?? [],
      episodeCompletions: options.episodeCompletions ?? [],
      collections: options.collections ?? [],
      collectionMembers: options.collectionMembers ?? [],
      failRecordLoads: options.failRecordLoads ?? false,
      settings: options.settings ?? {},
      tmdbSearchResults: options.tmdbSearchResults ?? [],
      tmdbDetail: options.tmdbDetail ?? {},
      tmdbDetails: options.tmdbDetails ?? {},
      tmdbSeasonDetails: options.tmdbSeasonDetails ?? {},
      tmdbDelayMs: options.tmdbDelayMs ?? 0,
      updateFailureCounts: options.updateFailureCounts ?? {},
      webdavRemote: options.webdavRemote ?? [],
      webdavV3Remote: options.webdavV3Remote ?? null,
      webdavV3Etag: options.webdavV3Etag === undefined ? '"v3-1"' : options.webdavV3Etag,
      webdavPreconditionFailures: options.webdavPreconditionFailures ?? 0,
      rotateEtagOnPreconditionFailure: options.rotateEtagOnPreconditionFailure ?? false,
      mutateLocalDuringPut: options.mutateLocalDuringPut ?? false,
      omitPutEtag: options.omitPutEtag ?? false,
      omitGetEtag: options.omitGetEtag ?? false,
      webdavFailureStatus: options.webdavFailureStatus ?? 503,
      webdavFailureCount: options.webdavFailureCount ?? 0,
      databaseCompatibilityIssue: options.databaseCompatibilityIssue ?? null,
      recoveryPoints: options.recoveryPoints ?? [],
      failSettingWrites: options.failSettingWrites ?? false,
    },
  );
}

export async function replaceMockRecords(page: Page, records: WatchRecord[]): Promise<void> {
  await page.evaluate((replacement) => {
    sessionStorage.setItem('__WATCHTRACKER_CONTROLLED_RECORDS__', JSON.stringify(replacement));
  }, records);
  await page.reload();
}

export async function mockSnapshot(page: Page): Promise<MockSnapshot> {
  return page.evaluate(() => structuredClone(window.__WATCHTRACKER_TEST__));
}
