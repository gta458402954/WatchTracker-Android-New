import { useEffect, useMemo, useRef, useState } from 'react';
import type { CollectionMember, WatchCollection, WatchRecord } from '../../../shared/types';
import { displayTitlesOf } from '../../../shared/lib/displayTitle';
import { useAccessibleDialog } from '../../../shared/lib/useAccessibleDialog';
import SafePosterImage from '../../watchlist/components/SafePosterImage';
import { getSettingAsync, getTmdbDetailAsync, getTmdbSeasonDetailAsync, searchTmdbAsync, setSettingAsync, type CompleteMovieCollectionInput, type CompleteMovieCollectionResult } from '../../../shared/lib/database';
import type { TmdbMedia, TmdbSeason } from '../../../shared/lib/classification';
import { getEmptyRecord } from '../../../shared/lib/constants';
import { chronologicalRecords, defaultMissingSeasonNumbers, locallyKnownSeries, readIdentityCache, seasonNumberOf, seriesBaseName, tvSourceKey, writeIdentityCache } from '../lib/seriesDiscovery';
import { movieRecordMetadata, seasonRecordMetadata, tmdbOriginalLanguageLocale } from '../lib/tmdbRecordMapping';
import { classifyMovieCollectionPart, type MovieCollectionCandidate } from '../lib/movieCollectionIdentity';
import {
  collectionCandidateDescription,
  movieCollectionCandidateEligibility,
  normalizeCollectionSearchMatches,
  type CollectionMatchSeed,
} from '../lib/collectionCandidateEligibility';
import {
  COLLECTION_SUGGESTION_DISMISSALS_KEY,
  parseSuggestionDismissals,
  serializeSuggestionDismissals,
  suggestionIsCovered,
  tvSuggestionEligibility,
  upsertSuggestionDismissal,
  type CollectionSuggestionDismissal,
} from '../lib/collectionSuggestionPolicy';

interface Props {
  records: WatchRecord[];
  collections: WatchCollection[];
  members: CollectionMember[];
  onCreate: (name: string, description: string | null, collectionKind?: WatchCollection['collectionKind']) => Promise<WatchCollection>;
  onUpdate: (collection: WatchCollection, name: string, description: string | null) => Promise<WatchCollection>;
  onSetOrderMode: (collection: WatchCollection, mode: WatchCollection['orderMode']) => Promise<WatchCollection>;
  onBindSource: (collection: WatchCollection, sourceKind: WatchCollection['sourceKind'], sourceKey: string, collectionKind: WatchCollection['collectionKind']) => Promise<WatchCollection>;
  onDelete: (collection: WatchCollection) => Promise<void>;
  onAddMembers: (collection: WatchCollection, recordIds: string[]) => Promise<void>;
  onRemoveMember: (member: CollectionMember) => Promise<void>;
  onReorder: (collection: WatchCollection, recordIds: string[]) => Promise<void>;
  onApplySuggestion: (name: string, sourceKind: 'tmdb-movie-collection' | 'tmdb-tv-show', sourceKey: string, recordIds: string[], targetCollectionId?: string) => Promise<void>;
  onCreateMissingSeasons: (collection: WatchCollection, records: WatchRecord[]) => Promise<void>;
  onCompleteMovieCollection: (input: CompleteMovieCollectionInput) => Promise<CompleteMovieCollectionResult>;
  onEditRecord: (record: WatchRecord, collectionId: string) => void;
  initialSelectedId?: string | null;
  onClose: () => void;
  onNotify: (tone: 'success' | 'warning' | 'error' | 'info', message: string) => void;
}

interface CollectionSuggestion {
  name: string;
  sourceKind: 'manual' | 'tmdb-movie-collection' | 'tmdb-tv-show';
  sourceKey: string;
  recordIds: string[];
  targetCollectionId?: string;
  requiresBinding?: boolean;
}
interface ScanMatchChoice extends CollectionMatchSeed {
  sourceId: number;
  sourceName: string;
  sourceKey: string;
  sourceKind: 'tmdb-movie-collection' | 'tmdb-tv-show';
  description: string;
}
interface ScanAmbiguity { imdbId: string; recordIds: string[]; choices: ScanMatchChoice[] }
interface ScanSummary { actionable: number; complete: number; covered: number; ignored: number; ambiguous: number; ineligible: number; unavailable: number }

async function mapWithConcurrency<T, R>(values: T[], limit: number, mapper: (value: T) => Promise<R>): Promise<R[]> {
  const results = new Array<R>(values.length);
  let nextIndex = 0;
  async function worker() {
    while (nextIndex < values.length) {
      const index = nextIndex++;
      results[index] = await mapper(values[index]);
    }
  }
  await Promise.all(Array.from({ length: Math.min(limit, values.length) }, worker));
  return results;
}

function errorText(error: unknown): string {
  const value = String(error);
  if (value.includes('collection_name_duplicate')) return '已经存在同名收藏集。';
  if (value.includes('stale_collection')) return '收藏集已在其他操作中更新，请重新尝试。';
  if (value.includes('stale_collection_member')) return '成员关系已变化，请重新尝试。';
  return '收藏集操作失败，请稍后重试。';
}

export default function CollectionCenter(props: Props) {
  const { records, collections, members, onClose, onNotify } = props;
  const [selectedId, setSelectedId] = useState<string | null>(() => (
    props.initialSelectedId && collections.some(item => item.id === props.initialSelectedId)
      ? props.initialSelectedId
      : collections[0]?.id ?? null
  ));
  const [search, setSearch] = useState('');
  const [creating, setCreating] = useState(false);
  const [editing, setEditing] = useState(false);
  const [adding, setAdding] = useState(false);
  const [name, setName] = useState('');
  const [description, setDescription] = useState('');
  const [newCollectionKind, setNewCollectionKind] = useState<WatchCollection['collectionKind']>('manual');
  const [recordSearch, setRecordSearch] = useState('');
  const [selectedRecords, setSelectedRecords] = useState<Set<string>>(() => new Set());
  const [busy, setBusy] = useState(false);
  const [scanning, setScanning] = useState(false);
  const [scanProgress, setScanProgress] = useState({ done: 0, total: 0 });
  const cancelScanRef = useRef(false);
  const [suggestions, setSuggestions] = useState<CollectionSuggestion[]>([]);
  const [scanAmbiguities, setScanAmbiguities] = useState<ScanAmbiguity[]>([]);
  const [scanSummary, setScanSummary] = useState<ScanSummary | null>(null);
  const [dismissals, setDismissals] = useState<CollectionSuggestionDismissal[]>([]);
  const [dismissalsLoaded, setDismissalsLoaded] = useState(false);
  const [showDismissedSuggestions, setShowDismissedSuggestions] = useState(false);
  const [seasonDetail, setSeasonDetail] = useState<TmdbMedia | null>(null);
  const [selectedSeasonNumbers, setSelectedSeasonNumbers] = useState<Set<number>>(() => new Set());
  const [showSpecials, setShowSpecials] = useState(false);
  const [movieCollectionDetail, setMovieCollectionDetail] = useState<TmdbMedia | null>(null);
  const [movieCandidates, setMovieCandidates] = useState<MovieCollectionCandidate[]>([]);
  const [selectedMovieIds, setSelectedMovieIds] = useState<Set<number>>(() => new Set());
  const [fillMissingMovieIdentity, setFillMissingMovieIdentity] = useState(true);
  const [parentChoices, setParentChoices] = useState<Array<{ id: number; name: string }>>([]);
  const [seriesBindingChoices, setSeriesBindingChoices] = useState<Array<{ id: number; name: string; year: string }>>([]);
  const [linkingRelated, setLinkingRelated] = useState(false);
  const [relatedSearch, setRelatedSearch] = useState('');
  const [relatedResults, setRelatedResults] = useState<TmdbMedia[]>([]);
  const closeRef = useRef<HTMLButtonElement>(null);
  const collectionButtonRefs = useRef(new Map<string, HTMLButtonElement>());
  const dialogRef = useAccessibleDialog<HTMLDivElement>({ onEscape: onClose, initialFocusRef: closeRef });

  useEffect(() => {
    if (selectedId === null || collections.some(item => item.id === selectedId)) return;
    // The selected entity can disappear after a confirmed delete or remote refresh.
    // eslint-disable-next-line react-hooks/set-state-in-effect
    setSelectedId(collections[0]?.id ?? null);
  }, [collections, selectedId]);

  useEffect(() => {
    let active = true;
    void getSettingAsync(COLLECTION_SUGGESTION_DISMISSALS_KEY)
      .then(raw => { if (active) setDismissals(parseSuggestionDismissals(raw)); })
      .catch(() => { if (active) onNotify('warning', '无法读取已忽略建议，本次仍可扫描。'); })
      .finally(() => { if (active) setDismissalsLoaded(true); });
    return () => { active = false; };
  }, [onNotify]);

  const selected = collections.find(item => item.id === selectedId) ?? null;
  const recordById = useMemo(() => new Map(records.map(record => [record.id, record])), [records]);
  const selectedMembers = useMemo(() => {
    const values = members.filter(member => member.collectionId === selectedId);
    const collection = collections.find(item => item.id === selectedId);
    if (collection?.orderMode !== 'chronological') return values.sort((left, right) => left.position - right.position || left.id.localeCompare(right.id));
    const orderedRecords = chronologicalRecords(values.map(member => recordById.get(member.recordId)).filter(Boolean) as WatchRecord[]);
    const rank = new Map(orderedRecords.map((record, index) => [record.id, index]));
    return values.sort((left, right) => (rank.get(left.recordId) ?? Number.MAX_SAFE_INTEGER) - (rank.get(right.recordId) ?? Number.MAX_SAFE_INTEGER) || left.id.localeCompare(right.id));
  }, [collections, members, recordById, selectedId]);
  const memberRecordIds = new Set(selectedMembers.map(item => item.recordId));
  const availableRecords = records.filter(record => {
    if (memberRecordIds.has(record.id)) return false;
    const text = `${record.chineseName} ${record.originalName}`.toLowerCase();
    return !recordSearch.trim() || text.includes(recordSearch.trim().toLowerCase());
  });
  const visibleCollections = collections.filter(item => item.name.toLowerCase().includes(search.trim().toLowerCase()));

  function selectAndFocusCollection(index: number) {
    const collection = visibleCollections[index];
    if (!collection) return;
    setSelectedId(collection.id);
    const button = collectionButtonRefs.current.get(collection.id);
    button?.focus({ preventScroll: true });
    button?.scrollIntoView({ block: 'nearest' });
  }

  function handleCollectionKeyDown(event: React.KeyboardEvent<HTMLButtonElement>, index: number) {
    let targetIndex: number | null = null;
    if (event.key === 'ArrowDown') targetIndex = Math.min(index + 1, visibleCollections.length - 1);
    else if (event.key === 'ArrowUp') targetIndex = Math.max(index - 1, 0);
    else if (event.key === 'Home') targetIndex = 0;
    else if (event.key === 'End') targetIndex = visibleCollections.length - 1;
    if (targetIndex === null) return;
    event.preventDefault();
    selectAndFocusCollection(targetIndex);
  }
  const watchedCount = selectedMembers.filter(item => recordById.get(item.recordId)?.status === '已看').length;
  const selectedParentIds = [...new Set(selectedMembers
    .map(member => recordById.get(member.recordId)?.tmdbParentId)
    .filter((id): id is number => Number.isInteger(id) && (id ?? 0) > 0))];
  const selectedMemberRecords = selectedMembers.map(member => recordById.get(member.recordId)).filter(Boolean) as WatchRecord[];
  const legacySeriesImdb = (() => {
    if (selected?.collectionKind !== 'manual' || selected.sourceKey || selectedMemberRecords.length < 2) return null;
    if (selectedMemberRecords.some(record => record.mediaType === '电影' || seasonNumberOf(record) == null)) return null;
    const ids = selectedMemberRecords.map(record => record.imdbId?.trim().toLowerCase() ?? '');
    return ids.every(id => /^tt\d+$/.test(id) && id === ids[0]) ? ids[0] : null;
  })();

  function reconcileSuggestion(item: CollectionSuggestion): CollectionSuggestion | null {
    if (suggestionIsCovered(item.recordIds, members)) return null;
    const itemIds = new Set(item.recordIds);
    const parentId = item.sourceKey.startsWith('tmdb:tv-show:') ? Number(item.sourceKey.slice('tmdb:tv-show:'.length)) : null;
    const existingCollection = collections.find(collection => {
      if (collection.sourceKey === item.sourceKey) return true;
      const collectionRecords = members.filter(member => member.collectionId === collection.id).map(member => recordById.get(member.recordId)).filter(Boolean) as WatchRecord[];
      const collectionParentIds = [...new Set(collectionRecords.map(record => record.tmdbParentId).filter((id): id is number => Number.isInteger(id) && (id ?? 0) > 0))];
      if (parentId != null && collectionParentIds.length === 1 && collectionParentIds[0] === parentId) return true;
      return collectionRecords.some(record => itemIds.has(record.id)) && collection.normalizedName === item.name.trim().toLowerCase();
    });
    if (!existingCollection) return item;
    const existingIds = new Set(members.filter(member => member.collectionId === existingCollection.id).map(member => member.recordId));
    const requiresBinding = item.sourceKind !== 'manual' && existingCollection.sourceKey !== item.sourceKey;
    const value = { ...item, targetCollectionId: existingCollection.id, requiresBinding, recordIds: item.recordIds.filter(id => !existingIds.has(id)) };
    return value.recordIds.length > 0 || value.requiresBinding ? value : null;
  }

  async function run(action: () => Promise<void>, success: string) {
    setBusy(true);
    try {
      await action();
      onNotify('success', success);
    } catch (error) {
      onNotify('error', errorText(error));
    } finally {
      setBusy(false);
    }
  }

  async function create() {
    if (!name.trim()) return;
    setBusy(true);
    try {
      const value = await props.onCreate(name, description || null, newCollectionKind);
      setSelectedId(value.id);
      setCreating(false); setName(''); setDescription(''); setNewCollectionKind('manual');
      onNotify('success', '收藏集已创建。');
    } catch (error) { onNotify('error', errorText(error)); }
    finally { setBusy(false); }
  }

  function startEdit() {
    if (!selected) return;
    setName(selected.name); setDescription(selected.description ?? ''); setEditing(true);
  }

  async function saveEdit() {
    if (!selected || !name.trim()) return;
    await run(async () => {
      const value = await props.onUpdate(selected, name, description || null);
      setSelectedId(value.id); setEditing(false);
    }, '收藏集已更新。');
  }

  async function deleteSelected() {
    if (!selected) return;
    if (!confirm(`确定删除“${selected.name}”吗？\n\n将删除 ${selectedMembers.length} 个归组关系，不会删除任何影视记录。`)) return;
    await run(async () => { await props.onDelete(selected); }, '收藏集已删除，影视记录均已保留。');
  }

  async function addSelectedRecords() {
    if (!selected || selectedRecords.size === 0) return;
    await run(async () => {
      await props.onAddMembers(selected, [...selectedRecords]);
      setSelectedRecords(new Set()); setAdding(false); setRecordSearch('');
    }, `已加入 ${selectedRecords.size} 条记录。`);
  }

  async function scanSuggestions() {
    // Discovery belongs to the whole library, not to the selected collection.
    setSelectedId(null);
    cancelScanRef.current = false;
    setScanAmbiguities([]);
    setScanSummary(null);
    setScanning(true);
    let activeDismissals = dismissals;
    if (!dismissalsLoaded) {
      try {
        activeDismissals = parseSuggestionDismissals(await getSettingAsync(COLLECTION_SUGGESTION_DISMISSALS_KEY));
        setDismissals(activeDismissals);
        setDismissalsLoaded(true);
      } catch { /* a read failure must not turn the scan into a write or block local discovery */ }
    }
    const dismissedKeys = new Set(activeDismissals.map(entry => entry.key));
    const grouped = new Map<string, CollectionSuggestion>();
    const tvDetails = new Map<string, TmdbMedia>();
    let failures = 0;
    let ambiguous = 0;
    let ineligible = 0;
    let precheckedComplete = 0;
    let precheckedCovered = 0;
    let precheckedIgnored = 0;
    for (const candidate of locallyKnownSeries(records)) {
      const stable = candidate.tmdbParentId != null;
      if (!stable && candidate.recordIds.length < 2) continue;
      const sourceKey = stable ? tvSourceKey(candidate.tmdbParentId!) : `local:${candidate.key}`;
      const current = grouped.get(sourceKey) ?? {
        name: candidate.name,
        sourceKind: stable ? 'tmdb-tv-show' as const : 'manual' as const,
        sourceKey,
        recordIds: [],
      };
      for (const recordId of candidate.recordIds) if (!current.recordIds.includes(recordId)) current.recordIds.push(recordId);
      grouped.set(sourceKey, current);
    }
    const byImdb = new Map<string, WatchRecord[]>();
    for (const record of records.filter(item => item.imdbId && !item.tmdbParentId)) {
      const key = record.imdbId!.trim().toLowerCase();
      byImdb.set(key, [...(byImdb.get(key) ?? []), record]);
    }
    const entries = [...byImdb.entries()];
    setScanProgress({ done: 0, total: entries.length });
    let cursor = 0;
    let completed = 0;
    const worker = async () => {
      while (cursor < entries.length) {
        if (cancelScanRef.current) return;
        const [imdbId, matchingRecords] = entries[cursor++];
      try {
        if (suggestionIsCovered(matchingRecords.map(record => record.id), members)) {
          grouped.delete(`local:${imdbId}`);
          precheckedCovered += 1;
          continue;
        }
        const cached = readIdentityCache<{ searchResult: Awaited<ReturnType<typeof searchTmdbAsync>> }>(imdbId);
        const searchResult = cached?.searchResult ?? await searchTmdbAsync({ query: imdbId, language: 'zh-CN' });
        writeIdentityCache(imdbId, { searchResult }, (searchResult.results?.length ?? 0) > 0);
        const seeds = normalizeCollectionSearchMatches(searchResult.results);
        if (seeds.length === 0) {
          failures += 1;
          continue;
        }

        const checked = await mapWithConcurrency(seeds, 4, async seed => {
          const detailCacheKey = `candidate-detail:${seed.mediaType}:${seed.id}`;
          let detail = readIdentityCache<TmdbMedia>(detailCacheKey);
          if (!detail) {
            const detailResult = await getTmdbDetailAsync({ id: seed.id, mediaType: seed.mediaType, language: 'zh-CN' });
            if (!detailResult.success || !detailResult.data) return { reason: 'unavailable' as const };
            detail = detailResult.data;
            writeIdentityCache(detailCacheKey, detail, true);
          }

          const hasIdentityConflict = matchingRecords.some(record => seed.mediaType === 'movie'
            ? (record.tmdbMediaKind != null && record.tmdbMediaKind !== 'movie') || (record.tmdbMediaKind === 'movie' && record.tmdbId != null && record.tmdbId !== seed.id)
            : (record.tmdbMediaKind === 'movie') || (record.tmdbParentId != null && record.tmdbParentId !== seed.id));
          if (hasIdentityConflict) return { reason: 'ineligible' as const };

          const sourceId = seed.mediaType === 'tv' ? seed.id : detail.belongs_to_collection?.id;
          const sourceName = seed.mediaType === 'tv' ? (detail.name || detail.title) : detail.belongs_to_collection?.name;
          if (sourceId == null || !sourceName) return { reason: 'ineligible' as const };
          const sourceKind = seed.mediaType === 'tv' ? 'tmdb-tv-show' as const : 'tmdb-movie-collection' as const;
          const sourceKey = seed.mediaType === 'tv' ? tvSourceKey(sourceId) : `tmdb:movie-collection:${sourceId}`;
          if (dismissedKeys.has(sourceKey)) return { reason: 'ignored' as const };

          let missingCount: number | undefined;
          if (seed.mediaType === 'tv') {
            const eligibility = tvSuggestionEligibility(detail, sourceId, matchingRecords.map(record => record.id), records);
            if (eligibility === 'complete') return { reason: 'complete' as const };
            if (eligibility === 'unknown') return { reason: 'unavailable' as const };
            tvDetails.set(sourceKey, detail);
            writeIdentityCache(`tv-series-detail:${sourceId}`, detail, true, Date.now(), 7);
          } else {
            const collectionCacheKey = `movie-collection:${sourceId}`;
            let collectionDetail = readIdentityCache<TmdbMedia>(collectionCacheKey);
            if (!collectionDetail) {
              const collectionResult = await getTmdbDetailAsync({ id: sourceId, mediaType: 'collection', language: 'zh-CN' });
              if (!collectionResult.success || !collectionResult.data) return { reason: 'unavailable' as const };
              collectionDetail = collectionResult.data;
              writeIdentityCache(collectionCacheKey, collectionDetail, true, Date.now(), 7);
            }
            const eligibility = movieCollectionCandidateEligibility(collectionDetail);
            if (eligibility === 'complete') return { reason: 'complete' as const };
            if (eligibility === 'unknown') return { reason: 'unavailable' as const };
            const localMovieIds = new Set(records.map(record => record.tmdbId).filter((value): value is number => Number.isInteger(value) && (value ?? 0) > 0));
            missingCount = [...new Set((collectionDetail.parts ?? [])
              .filter(part => Number.isInteger(part.id) && (part.id ?? 0) > 0 && part.release_date && new Date(`${part.release_date}T00:00:00Z`) <= new Date())
              .map(part => part.id as number))].filter(partId => !localMovieIds.has(partId)).length;
          }

          const enrichedSeed: CollectionMatchSeed = {
            ...seed,
            originalLabel: seed.originalLabel || detail.original_title || detail.original_name || null,
            year: seed.year || (detail.release_date || detail.first_air_date)?.slice(0, 4) || null,
            posterPath: seed.posterPath || detail.poster_path || null,
          };
          const choice: ScanMatchChoice = {
            ...enrichedSeed,
            sourceId,
            sourceName,
            sourceKey,
            sourceKind,
            description: collectionCandidateDescription(enrichedSeed, sourceName, missingCount),
          };
          return { reason: 'qualified' as const, choice };
        });
        const choices = checked.flatMap(result => result.reason === 'qualified' ? [result.choice] : []);
        precheckedComplete += checked.filter(result => result.reason === 'complete').length;
        precheckedIgnored += checked.filter(result => result.reason === 'ignored').length;
        ineligible += checked.filter(result => result.reason === 'ineligible').length;
        failures += checked.filter(result => result.reason === 'unavailable').length;
        if (choices.length === 0) continue;

        grouped.delete(`local:${imdbId}`);
        if (choices.length > 1) {
          ambiguous += 1;
          setScanAmbiguities(current => current.some(item => item.imdbId === imdbId) ? current : [...current, { imdbId, recordIds: matchingRecords.map(record => record.id), choices }]);
          continue;
        }
        const match = choices[0];
        const { sourceKey, sourceName, sourceKind } = match;
        const current = grouped.get(sourceKey) ?? { name: sourceName, sourceKind, sourceKey, recordIds: [] };
        for (const record of matchingRecords) if (!current.recordIds.includes(record.id)) current.recordIds.push(record.id);
        grouped.set(sourceKey, current);
      } catch { failures += 1; }
        finally { completed += 1; setScanProgress(current => ({ ...current, done: current.done + 1 })); }
      }
    };
    await Promise.all(Array.from({ length: Math.min(4, entries.length) }, worker));
    const summary: ScanSummary = { actionable: 0, complete: precheckedComplete, covered: precheckedCovered, ignored: precheckedIgnored, ambiguous, ineligible, unavailable: failures };
    const candidates: CollectionSuggestion[] = [];
    for (const item of grouped.values()) {
      if (dismissedKeys.has(item.sourceKey)) { summary.ignored += 1; continue; }
      if (suggestionIsCovered(item.recordIds, members)) { summary.covered += 1; continue; }
      const reconciled = reconcileSuggestion(item);
      if (!reconciled) { summary.covered += 1; continue; }
      candidates.push(reconciled);
    }
    const verified = await mapWithConcurrency(candidates, 4, async item => {
      if (item.sourceKind !== 'tmdb-tv-show' || item.recordIds.length !== 1) return item;
      const parentId = Number(item.sourceKey.slice('tmdb:tv-show:'.length));
      if (!Number.isInteger(parentId) || parentId <= 0) return item;
      const cacheKey = `tv-series-detail:${parentId}`;
      let detail = tvDetails.get(item.sourceKey) ?? readIdentityCache<TmdbMedia>(cacheKey);
      if (!detail) {
        try {
          const result = await getTmdbDetailAsync({ id: parentId, mediaType: 'tv', language: 'zh-CN' });
          if (!result.success || !result.data) { summary.unavailable += 1; return item; }
          detail = result.data;
          writeIdentityCache(cacheKey, detail, true, Date.now(), 7);
        } catch {
          summary.unavailable += 1;
          return item;
        }
      }
      const eligibility = tvSuggestionEligibility(detail, parentId, item.recordIds, records);
      if (eligibility === 'complete') { summary.complete += 1; return null; }
      if (eligibility === 'unknown') summary.unavailable += 1;
      return item;
    });
    const found = verified.filter((item): item is CollectionSuggestion => item !== null);
    summary.actionable = found.length;
    setSuggestions(found);
    setScanSummary(summary);
    setScanning(false);
    if (cancelScanRef.current) onNotify('info', `已停止发现系列；保留已完成的 ${completed} 项只读结果。`);
    else onNotify(found.length ? 'info' : 'warning', found.length
      ? `找到 ${found.length} 组可处理建议；已排除完整条目 ${summary.complete}、无合集或无差异 ${summary.ineligible}、已有归组 ${summary.covered}。`
      : `没有可直接应用的归组建议；待确认 ${summary.ambiguous}、完整排除 ${summary.complete}、无合集或无差异 ${summary.ineligible}。`);
  }

  async function resolveScanAmbiguity(ambiguity: ScanAmbiguity, choice: ScanMatchChoice) {
    setBusy(true);
    try {
      const resolved = reconcileSuggestion({ name: choice.sourceName, sourceKind: choice.sourceKind, sourceKey: choice.sourceKey, recordIds: ambiguity.recordIds });
      if (resolved) setSuggestions(current => {
        const prior = current.find(item => item.sourceKey === resolved.sourceKey);
        if (!prior) return [...current, resolved];
        return current.map(item => item.sourceKey === resolved.sourceKey ? { ...item, recordIds: [...new Set([...item.recordIds, ...resolved.recordIds])] } : item);
      });
      setScanAmbiguities(current => current.filter(item => item.imdbId !== ambiguity.imdbId));
      onNotify('info', resolved ? `已确认 ${choice.label}，建议已更新。` : `${choice.label} 已经完整归组，无需再次建议。`);
    } catch {
      onNotify('error', '无法使用所选 TMDB 匹配，请重试。');
    } finally { setBusy(false); }
  }

  async function applySuggestion(suggestion: CollectionSuggestion) {
    await run(async () => {
      if (suggestion.sourceKind === 'manual') {
        let collection = collections.find(item => item.normalizedName === suggestion.name.trim().toLowerCase());
        if (!collection) collection = await props.onCreate(suggestion.name, null);
        await props.onAddMembers(collection, suggestion.recordIds);
      } else {
        await props.onApplySuggestion(suggestion.name, suggestion.sourceKind, suggestion.sourceKey, suggestion.recordIds, suggestion.targetCollectionId);
      }
      setSuggestions(current => current.filter(item => item.sourceKey !== suggestion.sourceKey));
    }, `已应用“${suggestion.name}”归组建议。`);
  }

  async function dismissSuggestion(suggestion: CollectionSuggestion) {
    const next = upsertSuggestionDismissal(dismissals, {
      key: suggestion.sourceKey,
      name: suggestion.name,
      sourceKind: suggestion.sourceKind,
      dismissedAt: new Date().toISOString(),
    });
    try {
      await setSettingAsync(COLLECTION_SUGGESTION_DISMISSALS_KEY, serializeSuggestionDismissals(next));
      setDismissals(next);
      setSuggestions(current => current.filter(item => item.sourceKey !== suggestion.sourceKey));
      setScanSummary(current => current ? { ...current, actionable: Math.max(0, current.actionable - 1), ignored: current.ignored + 1 } : current);
      onNotify('info', `以后不再推荐“${suggestion.name}”；可在已忽略建议中恢复。`);
    } catch {
      onNotify('error', '无法保存忽略决定，建议仍然保留。');
    }
  }

  async function restoreDismissal(key: string) {
    const next = dismissals.filter(entry => entry.key !== key);
    try {
      await setSettingAsync(COLLECTION_SUGGESTION_DISMISSALS_KEY, serializeSuggestionDismissals(next));
      setDismissals(next);
      onNotify('success', '已恢复该建议；重新扫描后会按最新 TMDB 数据判断。');
    } catch { onNotify('error', '无法恢复已忽略建议。'); }
  }

  async function restoreAllDismissals() {
    try {
      await setSettingAsync(COLLECTION_SUGGESTION_DISMISSALS_KEY, serializeSuggestionDismissals([]));
      setDismissals([]);
      setShowDismissedSuggestions(false);
      onNotify('success', '已恢复全部建议；重新扫描后会按最新 TMDB 数据判断。');
    } catch { onNotify('error', '无法恢复已忽略建议。'); }
  }

  async function inspectMissingSeasons(parentId?: number, forceRefresh = false, sourceAlreadyBound = false) {
    if (!selected) return;
    const sourceId = selected.sourceKey?.startsWith('tmdb:tv-show:')
      ? Number(selected.sourceKey.slice('tmdb:tv-show:'.length))
      : null;
    const memberParentIds = selectedParentIds;
    const id = parentId ?? sourceId ?? (memberParentIds.length === 1 ? memberParentIds[0] : null);
    if (id == null && memberParentIds.length > 1) {
      setParentChoices(memberParentIds.map(parentId => {
        const record = selectedMembers.map(member => recordById.get(member.recordId)).find(item => item?.tmdbParentId === parentId);
        return { id: parentId, name: seriesBaseName(record?.chineseName) || seriesBaseName(record?.originalName) || `TMDB ${parentId}` };
      }));
      return;
    }
    if (id === null || !Number.isInteger(id) || id <= 0) return;
    if (!sourceAlreadyBound && !selected.sourceKey && selected.collectionKind !== 'universe') {
      if (!confirm(`已从收藏集成员识别到唯一的 TMDB 父剧 ID ${id}。\n\n确认后会把“${selected.name}”标记为电视剧系列并检查缺失季；已有条目不会被修改。`)) return;
      try {
        await props.onBindSource(selected, 'tmdb-tv-show', tvSourceKey(id), 'tv-series');
      } catch (error) {
        onNotify('error', errorText(error));
        return;
      }
    }
    setBusy(true);
    try {
      const cacheKey = `tv-detail:${id}:zh-CN`;
      const cached = forceRefresh ? undefined : readIdentityCache<Awaited<ReturnType<typeof getTmdbDetailAsync>>>(cacheKey);
      const result = cached ?? await getTmdbDetailAsync({ id, mediaType: 'tv', language: 'zh-CN' });
      if (!result.success || !result.data?.seasons) throw new Error('missing seasons');
      if (!cached) writeIdentityCache(cacheKey, result, true, undefined, 7);
      const existing = new Set(selectedMembers.map(member => recordById.get(member.recordId)).filter(Boolean).map(record => (record as WatchRecord).tmdbSeasonNumber ?? seasonNumberOf(record as WatchRecord)).filter((value): value is number => value != null));
      setSeasonDetail(result.data);
      setSelectedSeasonNumbers(new Set(defaultMissingSeasonNumbers(result.data.seasons, existing)));
      setShowSpecials(false);
    } catch (error) {
      console.warn('[CollectionCenter] could not inspect missing seasons', error instanceof Error ? error.name : 'unknown');
      onNotify('error', '读取 TMDB 全部季失败，请稍后重试。');
    } finally { setBusy(false); }
  }

  async function bindLegacySeriesChoice(choice: { id: number; name: string; year: string }) {
    if (!selected) return;
    if (!confirm(`已识别到电视剧“${choice.name}”${choice.year ? `（${choice.year}）` : ''}，TMDB 父剧 ID ${choice.id}。\n\n确认后只会把“${selected.name}”绑定为电视剧系列，不会修改现有季度记录。`)) return;
    setBusy(true);
    try {
      await props.onBindSource(selected, 'tmdb-tv-show', tvSourceKey(choice.id), 'tv-series');
      setSeriesBindingChoices([]);
      onNotify('success', '已绑定 TMDB 电视剧系列，正在读取全部季度。');
      await inspectMissingSeasons(choice.id, false, true);
    } catch (error) {
      onNotify('error', errorText(error));
    } finally { setBusy(false); }
  }

  async function identifyLegacySeries() {
    if (!legacySeriesImdb) return;
    setBusy(true);
    try {
      const result = await searchTmdbAsync({ query: legacySeriesImdb, language: 'zh-CN' });
      if (!result.success) throw new Error('search failed');
      const choices = (result.results ?? []).flatMap(item => {
        const id = item.media_type === 'tv' ? item.id : item.media_type === 'tv_season' ? item.show_id : null;
        if (!id) return [];
        return [{ id, name: item.name || item.original_name || `TMDB ${id}`, year: (item.first_air_date || '').slice(0, 4) }];
      }).filter((choice, index, all) => all.findIndex(item => item.id === choice.id) === index);
      if (!choices.length) {
        onNotify('warning', `IMDb ${legacySeriesImdb} 没有匹配到可用的 TMDB 电视剧。`);
      } else if (choices.length === 1) {
        await bindLegacySeriesChoice(choices[0]);
      } else {
        setSeriesBindingChoices(choices);
      }
    } catch {
      onNotify('error', '确认 TMDB 电视剧系列失败，请稍后重试。');
    } finally { setBusy(false); }
  }

  async function createSelectedSeasons() {
    if (!selected || !seasonDetail?.seasons || selectedSeasonNumbers.size === 0) return;
    const now = new Date().toISOString();
    setBusy(true);
    try {
      const selectedSeasons = seasonDetail.seasons.filter(season => selectedSeasonNumbers.has(season.season_number ?? -1));
      const originalDetails = new Map<number, TmdbSeason>();
      const locale = tmdbOriginalLanguageLocale(seasonDetail.original_language);
      for (let offset = 0; offset < selectedSeasons.length; offset += 4) {
        const batch = selectedSeasons.slice(offset, offset + 4);
        await Promise.all(batch.map(async season => {
          const number = season.season_number;
          if (number == null || seasonDetail.id == null) return;
          const cacheKey = `tv-season-detail:${seasonDetail.id}:${number}:${locale}`;
          const cached = readIdentityCache<Awaited<ReturnType<typeof getTmdbSeasonDetailAsync>>>(cacheKey);
          const result = cached ?? await getTmdbSeasonDetailAsync({ seriesId: seasonDetail.id, seasonNumber: number, language: locale });
          if (!cached && result.success && result.data) writeIdentityCache(cacheKey, result, true, undefined, 30);
          if (result.success && result.data) originalDetails.set(number, result.data);
        }));
      }
      const recordsToCreate = selectedSeasons.map(season => ({
        ...getEmptyRecord(),
        ...seasonRecordMetadata(seasonDetail, season, undefined, originalDetails.get(season.season_number ?? -1)),
        id: crypto.randomUUID(),
        createdAt: now,
        updatedAt: now,
      }));
      await props.onCreateMissingSeasons(selected, recordsToCreate);
      setSeasonDetail(null);
      onNotify('success', `已补充 ${recordsToCreate.length} 个缺少的季，已有条目未被覆盖。`);
    } catch (error) {
      onNotify('error', errorText(error));
    } finally { setBusy(false); }
  }

  async function bindSelectedSeries() {
    if (!selected) return;
    const parentIds = selectedParentIds;
    if (parentIds.length !== 1) {
      onNotify('warning', parentIds.length ? '成员属于多个 TMDB 父剧，请先修正成员或改为影视宇宙。' : '现有成员缺少稳定 TMDB 父剧身份，请先为任一季确认 TMDB 元数据。');
      return;
    }
    await run(async () => { await props.onBindSource(selected, 'tmdb-tv-show', `tmdb:tv-show:${parentIds[0]}`, 'tv-series'); }, '已绑定 TMDB 电视剧系列，现在可以检查缺失季。');
  }

  async function inspectMissingMovies(forceRefresh = false) {
    if (!selected) return;
    let collectionId = selected.sourceKey?.startsWith('tmdb:movie-collection:')
      ? Number(selected.sourceKey.slice('tmdb:movie-collection:'.length))
      : null;
    if (collectionId == null && selected.collectionKind === 'movie-series') {
      setBusy(true);
      try {
        const movieIds = [...new Set(selectedMembers.map(member => recordById.get(member.recordId)).filter(record => record?.tmdbMediaKind === 'movie' && record.tmdbId).map(record => record!.tmdbId!))];
        const details = await Promise.all(movieIds.map(id => getTmdbDetailAsync({ id, mediaType: 'movie', language: 'zh-CN' })));
        const candidates = [...new Set(details.map(result => result.data?.belongs_to_collection?.id).filter((id): id is number => Number.isInteger(id) && (id ?? 0) > 0))];
        if (candidates.length !== 1) {
          onNotify('warning', candidates.length > 1 ? '成员属于多个 TMDB 电影合集，请先修正成员。' : '现有成员没有可确认的 TMDB 电影合集身份。');
          return;
        }
        collectionId = candidates[0];
        if (!confirm(`已识别到唯一的 TMDB 电影合集 ID ${collectionId}。\n\n确认后会绑定“${selected.name}”并检查缺失电影；已有条目不会被修改。`)) return;
        await props.onBindSource(selected, 'tmdb-movie-collection', `tmdb:movie-collection:${collectionId}`, 'movie-series');
      } catch {
        onNotify('error', '确认 TMDB 电影合集失败，请稍后重试。');
        return;
      } finally { setBusy(false); }
    }
    if (collectionId == null || !Number.isInteger(collectionId) || collectionId <= 0) return;
    setBusy(true);
    try {
      const cacheKey = `movie-collection:${collectionId}:zh-CN`;
      const cached = forceRefresh ? undefined : readIdentityCache<Awaited<ReturnType<typeof getTmdbDetailAsync>>>(cacheKey);
      const result = cached ?? await getTmdbDetailAsync({ id: collectionId, mediaType: 'collection', language: 'zh-CN' });
      if (!result.success || !result.data?.parts) throw new Error('missing collection parts');
      if (!cached) writeIdentityCache(cacheKey, result, true, undefined, 7);
      const memberIds = new Set(selectedMembers.map(member => member.recordId));
      const detailed = await mapWithConcurrency(result.data.parts, 4, async movie => {
        if (!movie.id) return { movie, resolved: false };
        const detailKey = `movie-detail:${movie.id}:zh-CN`;
        const detailCached = forceRefresh ? undefined : readIdentityCache<Awaited<ReturnType<typeof getTmdbDetailAsync>>>(detailKey);
        const detailResult = detailCached ?? await getTmdbDetailAsync({ id: movie.id, mediaType: 'movie', language: 'zh-CN' });
        if (!detailCached && detailResult.success && detailResult.data) writeIdentityCache(detailKey, detailResult, true, undefined, 30);
        return { movie: detailResult.success && detailResult.data ? { ...movie, ...detailResult.data } : movie, resolved: !!(detailResult.success && detailResult.data) };
      });
      const candidates = detailed.map(item => item.resolved
        ? classifyMovieCollectionPart(item.movie, records, memberIds)
        : ({ movie: item.movie, status: 'unresolved' } as MovieCollectionCandidate));
      const today = new Date();
      setMovieCollectionDetail({ ...result.data, parts: detailed.map(item => item.movie) });
      setMovieCandidates(candidates);
      setSelectedMovieIds(new Set(candidates.filter(candidate => {
        const movie = candidate.movie;
        if (movie.id == null || !['library', 'missing'].includes(candidate.status) || !movie.release_date) return false;
        const release = new Date(`${movie.release_date}T00:00:00Z`);
        return !Number.isNaN(release.valueOf()) && release <= today;
      }).map(candidate => candidate.movie.id as number)));
    } catch {
      onNotify('error', '读取 TMDB 电影合集失败，请稍后重试。');
    } finally { setBusy(false); }
  }

  async function createSelectedMovies() {
    if (!selected || !movieCollectionDetail?.parts || selectedMovieIds.size === 0) return;
    setBusy(true);
    try {
      const chosen = movieCandidates.filter(candidate => candidate.movie.id != null && selectedMovieIds.has(candidate.movie.id));
      if (chosen.some(candidate => !['library', 'missing'].includes(candidate.status))) throw new Error('movie identity unresolved');
      const now = new Date().toISOString();
      const newRecords = chosen.filter(candidate => candidate.status === 'missing').map(candidate => ({
        ...getEmptyRecord(),
        ...movieRecordMetadata(candidate.movie),
        id: crypto.randomUUID(),
        createdAt: now,
        updatedAt: now,
      }));
      const matchedCandidates = [
        ...movieCandidates.filter(candidate => candidate.status === 'member'),
        ...chosen.filter(candidate => candidate.status === 'library'),
      ];
      const matches = [...new Map(matchedCandidates.filter(candidate => candidate.recordId).map(candidate => [candidate.recordId!, candidate])).values()].flatMap(candidate => {
        const record = candidate.recordId ? recordById.get(candidate.recordId) : undefined;
        const tmdbId = candidate.movie.id;
        if (!record || !tmdbId) return [];
        return [{ recordId: record.id, expectedRev: record.rev ?? 0, tmdbId, imdbId: candidate.movie.external_ids?.imdb_id || candidate.movie.imdb_id || null }];
      });
      const completion = await props.onCompleteMovieCollection({
        collectionId: selected.id,
        expectedRev: selected.rev,
        matches,
        newRecords,
        fillMissingIdentity: fillMissingMovieIdentity,
      });
      setMovieCollectionDetail(null);
      setMovieCandidates([]);
      onNotify('success', `已新增 ${completion.createdRecordIds.length} 部、复用 ${completion.reusedRecordIds.length} 部${completion.identityUpdatedRecordIds.length ? `，补全 ${completion.identityUpdatedRecordIds.length} 条旧身份` : ''}。`);
    } catch (error) {
      onNotify('error', String(error).includes('movie_identity_conflict') ? '检测到电影身份冲突，请先修正对应条目的 TMDB/IMDb 信息；本次没有写入。' : '补充电影失败；片库与收藏集均未更改。');
    } finally { setBusy(false); }
  }

  async function searchRelatedWorks() {
    const query = relatedSearch.trim();
    if (!query) return;
    setBusy(true);
    try {
      const result = await searchTmdbAsync({ query, language: 'zh-CN' });
      setRelatedResults((result.results ?? []).filter(item => item.id != null && ['movie', 'tv'].includes(item.media_type ?? '')).slice(0, 20));
    } catch { onNotify('error', '搜索相关作品失败，请稍后重试。'); }
    finally { setBusy(false); }
  }

  async function linkRelatedWork(item: TmdbMedia) {
    if (!selected || item.id == null || !['movie', 'tv'].includes(item.media_type ?? '')) return;
    setBusy(true);
    try {
      const kind = item.media_type as 'movie' | 'tv';
      const detailResult = await getTmdbDetailAsync({ id: item.id, mediaType: kind, language: 'zh-CN' });
      if (!detailResult.success || !detailResult.data) throw new Error('missing detail');
      const detail = detailResult.data;
      const now = new Date().toISOString();
      const name = detail.name || detail.title || item.name || item.title || '未命名作品';
      const original = detail.original_name || detail.original_title || item.original_name || item.original_title || name;
      const releaseDate = detail.release_date || detail.first_air_date || item.release_date || item.first_air_date;
      const record: WatchRecord = {
        ...getEmptyRecord(), id: crypto.randomUUID(), createdAt: now, updatedAt: now,
        chineseName: name, originalName: original, releaseYear: releaseDate?.slice(0, 4) || null,
        posterPath: detail.poster_path || item.poster_path || null,
        imdbId: detail.external_ids?.imdb_id || detail.imdb_id || null,
        imdbRating: detail.vote_average || null, tmdbStatus: detail.status || null,
        mediaType: kind === 'movie' ? '电影' : '剧集',
        tmdbMediaKind: kind,
        tmdbId: detail.id ?? item.id,
        tmdbParentId: null,
        tmdbSeasonNumber: null,
        seriesRecordKind: kind === 'movie' ? 'single-work' : 'whole-series',
        totalEpisodes: kind === 'tv' ? detail.number_of_episodes || null : null,
        movieDuration: kind === 'movie' && detail.runtime ? detail.runtime * 60 : null,
      };
      await props.onCreateMissingSeasons(selected, [record]);
      setRelatedResults(current => current.filter(result => result.id !== item.id || result.media_type !== item.media_type));
      onNotify('success', `已将“${name}”作为相关作品加入片库和当前影视宇宙。`);
    } catch { onNotify('error', '关联相关作品失败；片库与收藏集均未更改。'); }
    finally { setBusy(false); }
  }

  const hasDiscoveryView = scanning || scanSummary !== null || suggestions.length > 0 || scanAmbiguities.length > 0;
  const discoveryView = <div className="flex h-full flex-col">
    <div className="border-b border-gray-100 p-5 sm:p-7">
      <div className="flex items-start justify-between gap-4">
        <div><h3 className="text-3xl font-black text-gray-900">片库归组建议</h3><p className="mt-2 text-sm text-gray-500">这是全片库的只读发现结果；选择左侧收藏集后只显示该收藏集内容</p></div>
        <button onClick={onClose} aria-label="关闭收藏集中心" className="hidden rounded-xl px-3 py-2 text-xl text-gray-400 md:block">×</button>
      </div>
    </div>
    <div className="flex-1 overflow-y-auto p-5 sm:p-7">
      {scanning && <div className="rounded-2xl border border-indigo-100 bg-indigo-50/60 p-4 text-sm font-semibold text-indigo-700">正在扫描片库 {scanProgress.done}/{scanProgress.total}</div>}
      {suggestions.length > 0 && <div className="rounded-2xl border border-indigo-100 bg-indigo-50/60 p-3"><p className="text-xs font-bold text-indigo-800">TMDB 只读建议 · 确认前不会修改数据</p><div className="mt-2 flex flex-wrap gap-2">{suggestions.map(item => <div key={item.sourceKey} className="flex items-center overflow-hidden rounded-xl border border-indigo-200 bg-white text-xs text-indigo-700"><button disabled={busy} onClick={() => void applySuggestion(item)} className="px-3 py-2 text-left hover:bg-indigo-50 disabled:opacity-50"><b>{item.name}</b><span className="ml-2 text-indigo-400">{item.requiresBinding ? '确认系列身份' : `${item.recordIds.length} 部`} · 应用</span></button><button disabled={busy} onClick={() => void dismissSuggestion(item)} aria-label={`不再推荐 ${item.name}`} title="不再推荐" className="self-stretch border-l border-indigo-100 px-2.5 text-indigo-300 hover:bg-red-50 hover:text-red-500 disabled:opacity-50">×</button></div>)}</div></div>}
      {scanAmbiguities.length > 0 && <div className="mt-3 rounded-2xl border border-amber-200 bg-amber-50/70 p-3"><p className="text-xs font-bold text-amber-800">{scanAmbiguities.length} 项存在多个有效 TMDB 匹配，请选择</p><div className="mt-2 space-y-2">{scanAmbiguities.map(ambiguity => <div key={ambiguity.imdbId} className="rounded-xl bg-white p-2"><p className="text-[11px] text-gray-500">IMDb {ambiguity.imdbId}</p><p className="mt-1 truncate text-xs font-semibold text-gray-700">本地：{ambiguity.recordIds.map(id => recordById.get(id)).filter(Boolean).map(record => displayTitlesOf(record!).primary).join('、')}</p><div className="mt-2 grid gap-2 sm:grid-cols-2">{ambiguity.choices.map(choice => <button key={`${choice.mediaType}:${choice.id}`} disabled={busy} onClick={() => void resolveScanAmbiguity(ambiguity, choice)} className="flex min-w-0 items-center gap-2 rounded-xl border border-amber-200 px-3 py-2 text-left text-xs text-amber-900 hover:bg-amber-50 disabled:opacity-50"><SafePosterImage posterPath={choice.posterPath || ''} alt="" compact className="h-12 w-8 shrink-0 rounded object-cover" /><span className="min-w-0"><span className="block truncate font-bold">{choice.label} · {choice.mediaType === 'tv' ? '剧集' : '电影'}</span><span className="mt-1 block text-[10px] leading-4 text-amber-700">{choice.description}</span></span></button>)}</div></div>)}</div></div>}
      {!scanning && scanSummary && suggestions.length === 0 && scanAmbiguities.length === 0 && <div className="py-20 text-center"><p className="text-4xl">✓</p><p className="mt-3 font-bold text-gray-700">没有可处理的归组建议</p><p className="mt-2 text-sm text-gray-400">可以从左侧选择收藏集查看具体条目</p></div>}
    </div>
  </div>;

  return <div className="fixed inset-0 z-[90] flex items-center justify-center bg-slate-900/35 p-3 sm:p-6">
    <div ref={dialogRef} role="dialog" aria-modal="true" aria-labelledby="collections-title" tabIndex={-1} className="flex h-[min(900px,94vh)] w-full max-w-7xl overflow-hidden rounded-[28px] bg-white shadow-2xl">
      <aside className={`${(selected || hasDiscoveryView) && 'max-md:hidden'} flex w-full flex-col border-r border-gray-100 bg-gray-50/70 md:w-[31%]`}>
        <div className="border-b border-gray-100 p-5">
          <div className="flex items-center justify-between gap-3">
            <h2 id="collections-title" className="text-2xl font-black text-gray-900">系列与收藏集</h2>
            <button ref={closeRef} onClick={onClose} aria-label="关闭收藏集中心" className="rounded-xl px-3 py-2 text-xl text-gray-400 hover:bg-white">×</button>
          </div>
          <input value={search} onChange={event => setSearch(event.target.value)} onKeyDown={event => { if (event.key === 'ArrowDown' && visibleCollections.length) { event.preventDefault(); const selectedIndex = visibleCollections.findIndex(item => item.id === selectedId); selectAndFocusCollection(selectedIndex >= 0 ? selectedIndex : 0); } }} placeholder="搜索收藏集" className="mt-4 w-full rounded-xl border border-gray-200 bg-white px-4 py-2.5 text-sm outline-none focus:border-indigo-400" />
          <button onClick={() => { setCreating(true); setName(''); setDescription(''); setNewCollectionKind('manual'); }} className="mt-3 w-full rounded-xl border border-indigo-200 bg-white py-2.5 text-sm font-bold text-indigo-600 hover:bg-indigo-50">＋ 新建收藏集</button>
          {creating && <div className="mt-3 space-y-2 rounded-2xl border border-indigo-100 bg-white p-3">
            <input autoFocus value={name} onChange={event => setName(event.target.value)} maxLength={80} placeholder="收藏集名称" className="w-full rounded-lg border border-gray-200 px-3 py-2 text-sm" />
            <textarea value={description} onChange={event => setDescription(event.target.value)} maxLength={500} placeholder="说明（可选）" className="w-full resize-none rounded-lg border border-gray-200 px-3 py-2 text-sm" />
            <select value={newCollectionKind} onChange={event => setNewCollectionKind(event.target.value as WatchCollection['collectionKind'])} className="w-full rounded-lg border border-gray-200 px-3 py-2 text-sm"><option value="manual">普通收藏集（手工排序）</option><option value="tv-series">电视剧系列（绑定后可检查缺失季）</option><option value="movie-series">电影系列（绑定后可检查缺失电影）</option><option value="universe">影视宇宙（年代排序，可关联相关作品）</option></select>
            <div className="flex gap-2"><button disabled={busy || !name.trim()} onClick={() => void create()} className="flex-1 rounded-lg bg-indigo-600 py-2 text-xs font-bold text-white disabled:bg-gray-300">创建</button><button onClick={() => setCreating(false)} className="rounded-lg border px-3 text-xs">取消</button></div>
          </div>}
          <button onClick={() => scanning ? (cancelScanRef.current = true) : void scanSuggestions()} className="mt-3 w-full rounded-xl border border-indigo-100 bg-white py-2.5 text-sm font-bold text-indigo-600">{scanning ? `停止全库扫描 ${scanProgress.done}/${scanProgress.total}` : '扫描片库归组建议'}</button>
          <button disabled={!dismissalsLoaded || dismissals.length === 0} onClick={() => setShowDismissedSuggestions(true)} className="mt-2 w-full rounded-xl border border-gray-200 bg-white py-2 text-xs font-semibold text-gray-500 disabled:opacity-40">已忽略建议（{dismissals.length}）</button>
          {scanSummary && <div className="mt-2 rounded-xl bg-white px-3 py-2 text-[10px] leading-5 text-gray-500"><p>可处理 {scanSummary.actionable} · 完整排除 {scanSummary.complete} · 已归组 {scanSummary.covered}</p><p>非合集/无差异 {scanSummary.ineligible} · 已忽略 {scanSummary.ignored} · 待确认 {scanSummary.ambiguous} · 无法确认 {scanSummary.unavailable}</p></div>}
        </div>
        <div className="flex-1 space-y-2 overflow-y-auto p-3">
          {visibleCollections.map((collection, index) => {
            const items = members.filter(item => item.collectionId === collection.id);
            const posters = items.map(item => recordById.get(item.recordId)).filter(Boolean).slice(0, 3) as WatchRecord[];
            return <button key={collection.id} ref={element => { if (element) collectionButtonRefs.current.set(collection.id, element); else collectionButtonRefs.current.delete(collection.id); }} aria-pressed={selectedId === collection.id} onClick={() => setSelectedId(collection.id)} onKeyDown={event => handleCollectionKeyDown(event, index)} className={`flex w-full items-center gap-3 rounded-2xl border p-3 text-left ${selectedId === collection.id ? 'border-indigo-300 bg-indigo-50' : 'border-gray-100 bg-white hover:border-gray-200'}`}>
              <div className="flex h-14 w-20 overflow-hidden rounded-lg bg-gray-100">{posters.length ? posters.map(record => <SafePosterImage key={record.id} posterPath={record.posterPath || ''} alt="" compact className="h-full min-w-0 flex-1 object-cover" />) : <span className="m-auto text-2xl">🎞️</span>}</div>
              <div className="min-w-0"><p className="truncate text-sm font-bold text-gray-800">{collection.name}</p><p className="mt-1 text-xs text-gray-400">{items.length} 部</p></div>
            </button>;
          })}
          {!visibleCollections.length && <p className="py-10 text-center text-sm text-gray-400">尚无匹配的收藏集</p>}
        </div>
      </aside>

      <main className={`${!selected && !hasDiscoveryView && 'max-md:hidden'} flex min-w-0 flex-1 flex-col bg-white`}>
        {!selected ? (hasDiscoveryView ? discoveryView : <div className="m-auto text-center"><p className="text-5xl">🎬</p><p className="mt-4 font-bold text-gray-700">创建第一个收藏集</p><p className="mt-1 text-sm text-gray-400">按系列、导演或个人主题整理片库</p></div>) : <>
          <div className="border-b border-gray-100 p-5 sm:p-7">
            <div className="flex flex-col items-stretch gap-4 sm:flex-row sm:items-start sm:justify-between">
              <div className="min-w-0">
                <button onClick={() => setSelectedId(null)} className="mb-3 text-xs font-bold text-indigo-600 md:hidden">← 返回收藏集</button>
                {editing ? <div className="space-y-2"><input value={name} onChange={event => setName(event.target.value)} maxLength={80} className="w-full rounded-xl border px-3 py-2 text-xl font-black" /><textarea value={description} onChange={event => setDescription(event.target.value)} maxLength={500} className="w-full resize-none rounded-xl border px-3 py-2 text-sm" /><div className="flex gap-2"><button disabled={busy || !name.trim()} onClick={() => void saveEdit()} className="rounded-lg bg-indigo-600 px-4 py-2 text-xs font-bold text-white">保存</button><button onClick={() => setEditing(false)} className="rounded-lg border px-4 text-xs">取消</button></div></div> : <><h3 className="truncate text-3xl font-black text-gray-900">{selected.name}</h3><p className="mt-2 text-sm text-gray-500">{selected.description || '尚未添加说明'}</p></>}
                <div className="mt-4 flex flex-wrap gap-2 text-xs"><span className="rounded-lg border px-3 py-1.5">🎬 {selectedMembers.length} 部作品</span><span className="rounded-lg border border-emerald-100 bg-emerald-50 px-3 py-1.5 text-emerald-700">✓ 已看 {watchedCount}</span><span className="rounded-lg border border-amber-100 bg-amber-50 px-3 py-1.5 text-amber-700">○ 未看 {selectedMembers.length - watchedCount}</span></div>
              </div>
              <div className="grid w-full grid-cols-2 gap-2 sm:flex sm:w-auto sm:shrink-0 sm:flex-wrap sm:justify-end"><button onClick={() => setAdding(true)} className="rounded-xl bg-indigo-600 px-3 py-2.5 text-sm font-bold text-white sm:px-4">＋ 从片库添加</button>{legacySeriesImdb && <button disabled={busy} onClick={() => void identifyLegacySeries()} className="rounded-xl border border-indigo-100 px-3 py-2 text-sm font-semibold text-indigo-600 disabled:opacity-50">确认 TMDB 系列</button>}{selected.collectionKind === 'tv-series' && !selected.sourceKey && selectedParentIds.length !== 1 && <button disabled={busy} onClick={() => void bindSelectedSeries()} className="rounded-xl border border-indigo-100 px-3 py-2 text-sm font-semibold text-indigo-600 disabled:opacity-50">确认 TMDB 系列</button>}{(selected.sourceKey?.startsWith('tmdb:tv-show:') || selectedParentIds.length === 1 || selected.collectionKind === 'universe' && selectedParentIds.length > 0) && <button disabled={busy} onClick={() => void inspectMissingSeasons()} className="rounded-xl border border-indigo-100 px-3 py-2 text-sm font-semibold text-indigo-600 disabled:opacity-50">{selected.collectionKind === 'universe' ? '检查缺失条目' : '检查缺失季'}</button>}{selected.collectionKind === 'movie-series' && <button disabled={busy} onClick={() => void inspectMissingMovies()} className="rounded-xl border border-indigo-100 px-3 py-2 text-sm font-semibold text-indigo-600 disabled:opacity-50">检查缺失电影</button>}{selected.collectionKind === 'universe' && <button disabled={busy} onClick={() => setLinkingRelated(true)} className="rounded-xl border border-indigo-100 px-3 py-2 text-sm font-semibold text-indigo-600 disabled:opacity-50">关联相关作品</button>}<button onClick={startEdit} className="rounded-xl border px-3 py-2 text-sm">编辑</button><button onClick={() => void deleteSelected()} className="rounded-xl border border-red-100 px-3 py-2 text-sm text-red-500">删除</button><button onClick={onClose} aria-label="关闭收藏集中心" className="hidden rounded-xl px-3 py-2 text-xl text-gray-400 md:block">×</button></div>
            </div>
          </div>
          <div className="flex-1 space-y-3 overflow-y-auto p-5 sm:p-7">
            {selectedMembers.map((member, index) => {
              const record = recordById.get(member.recordId);
              if (!record) return null;
              const titles = displayTitlesOf(record);
              return <div key={member.id} className="flex flex-wrap items-center gap-3 rounded-2xl border border-gray-100 p-3 hover:border-gray-200">
                <span className="text-gray-300" aria-hidden="true">⠿</span><span className="w-6 text-sm font-bold text-gray-500">{index + 1}</span>
                <SafePosterImage posterPath={record.posterPath || ''} alt="" compact className="h-16 w-11 rounded-lg object-cover" />
                <button onClick={() => props.onEditRecord(record, selected.id)} className="min-w-[7rem] flex-1 text-left"><p className="truncate font-bold text-gray-800">{titles.primary}{record.releaseYear ? ` · ${record.releaseYear.slice(0, 4)}` : ''}</p><p className="mt-1 truncate text-xs text-gray-400">{titles.secondary || record.mediaType}</p></button>
                <span className={`rounded-full px-3 py-1 text-xs font-bold ${record.status === '已看' ? 'bg-emerald-50 text-emerald-700' : record.status === '在看' ? 'bg-blue-50 text-blue-600' : 'bg-gray-100 text-gray-500'}`}>{record.status}</span>
                <div className="ml-auto flex">{selected.orderMode !== 'chronological' && <><button disabled={busy || index === 0} onClick={() => { const next = selectedMembers.map(item => item.recordId); [next[index - 1], next[index]] = [next[index], next[index - 1]]; void run(() => props.onReorder(selected, next), '成员顺序已更新。'); }} aria-label={`上移 ${titles.primary}`} className="rounded-lg px-2 py-1 text-gray-400 disabled:opacity-20">↑</button><button disabled={busy || index === selectedMembers.length - 1} onClick={() => { const next = selectedMembers.map(item => item.recordId); [next[index], next[index + 1]] = [next[index + 1], next[index]]; void run(() => props.onReorder(selected, next), '成员顺序已更新。'); }} aria-label={`下移 ${titles.primary}`} className="rounded-lg px-2 py-1 text-gray-400 disabled:opacity-20">↓</button></>}<button disabled={busy} onClick={() => void run(() => props.onRemoveMember(member), '已从收藏集移除，影视记录仍保留。')} aria-label={`从收藏集移除 ${titles.primary}`} className="rounded-lg px-2 py-1 text-red-400 hover:bg-red-50">×</button></div>
              </div>;
            })}
            {!selectedMembers.length && <div className="py-20 text-center"><p className="text-4xl">🎞️</p><p className="mt-3 font-bold text-gray-600">这个收藏集还是空的</p><button onClick={() => setAdding(true)} className="mt-4 rounded-xl bg-indigo-600 px-5 py-2 text-sm font-bold text-white">从片库添加</button></div>}
          </div>
          <div className="flex flex-wrap items-center justify-between gap-2 border-t border-gray-100 px-5 py-4 text-xs text-gray-400 sm:px-7"><span>ⓘ {selected.orderMode === 'chronological' ? '当前按年代从老到新排列，未知年份排在最后' : '当前为手工顺序，可使用上下按钮调整'} · 删除收藏集不会删除影视记录</span>{selected.orderMode !== 'chronological' && <button disabled={busy} onClick={() => void run(() => props.onSetOrderMode(selected, 'chronological').then(() => undefined), '已改为按年代从老到新排列。')} className="rounded-lg border border-indigo-100 px-3 py-1.5 font-bold text-indigo-600">按年代排列</button>}</div>
        </>}
      </main>
    </div>

    {adding && selected && <div className="fixed inset-0 z-[100] flex items-center justify-center bg-slate-900/35 p-4"><div role="dialog" aria-modal="true" aria-label="从片库添加" className="flex max-h-[80vh] w-full max-w-xl flex-col rounded-3xl bg-white p-5 shadow-2xl">
      <div className="flex items-center justify-between"><h3 className="text-xl font-black">从片库添加</h3><button onClick={() => setAdding(false)} className="text-xl text-gray-400">×</button></div>
      <input autoFocus value={recordSearch} onChange={event => setRecordSearch(event.target.value)} placeholder="搜索片名" className="mt-4 rounded-xl border px-4 py-2.5 text-sm" />
      <div className="mt-3 flex-1 space-y-1 overflow-y-auto">{availableRecords.map(record => <label key={record.id} className="flex cursor-pointer items-center gap-3 rounded-xl p-2 hover:bg-gray-50"><input type="checkbox" checked={selectedRecords.has(record.id)} onChange={event => setSelectedRecords(current => { const next = new Set(current); if (event.target.checked) next.add(record.id); else next.delete(record.id); return next; })} /><SafePosterImage posterPath={record.posterPath || ''} alt="" compact className="h-12 w-8 rounded object-cover" /><span className="min-w-0 flex-1 truncate text-sm font-semibold">{displayTitlesOf(record).primary}</span><span className="text-xs text-gray-400">{record.status}</span></label>)}</div>
      <div className="mt-4 flex items-center justify-between border-t pt-4"><span className="text-xs text-gray-500">已选择 {selectedRecords.size} 条</span><div className="flex gap-2"><button onClick={() => setAdding(false)} className="rounded-xl border px-4 py-2 text-sm">取消</button><button disabled={busy || !selectedRecords.size} onClick={() => void addSelectedRecords()} className="rounded-xl bg-indigo-600 px-5 py-2 text-sm font-bold text-white disabled:bg-gray-300">加入收藏集</button></div></div>
    </div></div>}
    {seriesBindingChoices.length > 0 && <div className="fixed inset-0 z-[106] flex items-center justify-center bg-slate-900/45 p-4"><div role="dialog" aria-modal="true" aria-label="选择 TMDB 电视剧系列" className="w-full max-w-md rounded-3xl bg-white p-5 shadow-2xl"><div className="flex items-start justify-between"><div><h3 className="text-xl font-black">选择 TMDB 电视剧系列</h3><p className="mt-1 text-xs text-gray-400">共享 IMDb 匹配到多个电视剧，必须明确选择后才能绑定</p></div><button onClick={() => setSeriesBindingChoices([])} className="text-xl text-gray-400">×</button></div><div className="mt-4 space-y-2">{seriesBindingChoices.map(choice => <button key={choice.id} disabled={busy} onClick={() => void bindLegacySeriesChoice(choice)} className="flex w-full items-center justify-between rounded-2xl border px-4 py-3 text-left hover:border-indigo-200 disabled:opacity-50"><span><b className="block text-gray-800">{choice.name}</b><span className="text-xs text-gray-400">{choice.year || '年份未知'}</span></span><span className="text-xs text-gray-400">TMDB {choice.id} →</span></button>)}</div></div></div>}
    {parentChoices.length > 0 && <div className="fixed inset-0 z-[105] flex items-center justify-center bg-slate-900/45 p-4"><div role="dialog" aria-modal="true" aria-label="选择要检查的系列" className="w-full max-w-md rounded-3xl bg-white p-5 shadow-2xl"><div className="flex items-start justify-between"><div><h3 className="text-xl font-black">选择要检查的系列</h3><p className="mt-1 text-xs text-gray-400">影视宇宙包含多个父剧，每次只读取所选系列的 TMDB 全部季</p></div><button onClick={() => setParentChoices([])} className="text-xl text-gray-400">×</button></div><div className="mt-4 space-y-2">{parentChoices.map(choice => <button key={choice.id} onClick={() => { setParentChoices([]); void inspectMissingSeasons(choice.id); }} className="flex w-full items-center justify-between rounded-2xl border px-4 py-3 text-left hover:border-indigo-200"><span className="font-bold text-gray-800">{choice.name}</span><span className="text-xs text-gray-400">TMDB {choice.id} →</span></button>)}</div></div></div>}
    {seasonDetail?.seasons && selected && <div className="fixed inset-0 z-[105] flex items-center justify-center bg-slate-900/45 p-4"><div role="dialog" aria-modal="true" aria-label="检查缺失季" className="flex max-h-[86vh] w-full max-w-2xl flex-col rounded-3xl bg-white p-5 shadow-2xl">
      <div className="flex items-start justify-between gap-4"><div><h3 className="text-xl font-black text-gray-900">查看 TMDB 全部季</h3><p className="mt-1 text-xs text-gray-400">只会新增勾选的缺少季，不覆盖片库已有条目</p></div><div className="flex items-center gap-2"><button disabled={busy} onClick={() => void inspectMissingSeasons(undefined, true)} className="rounded-lg border px-3 py-1.5 text-xs font-bold text-indigo-600 disabled:opacity-50">刷新 TMDB</button><button onClick={() => setSeasonDetail(null)} className="text-xl text-gray-400">×</button></div></div>
      <label className="mt-4 flex items-center gap-2 text-xs text-gray-500"><input type="checkbox" checked={showSpecials} onChange={event => setShowSpecials(event.target.checked)} />显示第 0 季 / 特别篇（默认不选）</label>
      <div className="mt-3 flex-1 space-y-2 overflow-y-auto">{seasonDetail.seasons.filter(season => showSpecials || (season.season_number ?? 0) > 0).map((season: TmdbSeason) => {
        const number = season.season_number ?? 0;
        const existing = selectedMembers.some(member => { const record = recordById.get(member.recordId); return record ? (record.tmdbSeasonNumber ?? seasonNumberOf(record)) === number : false; });
        const checked = selectedSeasonNumbers.has(number);
        const future = !!season.air_date && new Date(`${season.air_date}T00:00:00Z`) > new Date();
        return <label key={season.id ?? number} className={`flex items-center gap-3 rounded-2xl border p-3 ${existing ? 'bg-gray-50 opacity-60' : 'cursor-pointer hover:border-indigo-200'}`}><input type="checkbox" disabled={existing} checked={checked} onChange={() => setSelectedSeasonNumbers(current => { const next = new Set(current); if (checked) next.delete(number); else next.add(number); return next; })} /><div className="min-w-0 flex-1"><p className="truncate text-sm font-bold text-gray-800">{season.name || `第 ${number} 季`} · {season.air_date?.slice(0, 4) || '年份未知'}</p><p className="mt-1 text-xs text-gray-400">{season.episode_count ?? '未知'} 集{existing ? ' · 已在片库' : future ? ' · 尚未播出' : !season.air_date ? ' · 播出日期未知' : ''}</p></div></label>;
      })}</div>
      <div className="mt-4 flex items-center justify-between border-t pt-4"><span className="text-xs text-gray-500">已选择 {selectedSeasonNumbers.size} 季</span><div className="flex gap-2"><button onClick={() => setSeasonDetail(null)} className="rounded-xl border px-4 py-2 text-sm">取消</button><button disabled={busy || selectedSeasonNumbers.size === 0} onClick={() => void createSelectedSeasons()} className="rounded-xl bg-indigo-600 px-5 py-2 text-sm font-bold text-white disabled:bg-gray-300">补充到片库</button></div></div>
    </div></div>}
    {movieCollectionDetail?.parts && selected && <div className="fixed inset-0 z-[105] flex items-center justify-center bg-slate-900/45 p-4"><div role="dialog" aria-modal="true" aria-label="检查缺失电影" className="flex max-h-[86vh] w-full max-w-2xl flex-col rounded-3xl bg-white p-5 shadow-2xl">
      <div className="flex items-start justify-between gap-4"><div><h3 className="text-xl font-black text-gray-900">查看 TMDB 电影合集</h3><p className="mt-1 text-xs text-gray-400">按 TMDB 与 IMDb 双身份核验；确认前不会修改片库</p></div><div className="flex items-center gap-2"><button disabled={busy} onClick={() => void inspectMissingMovies(true)} className="rounded-lg border px-3 py-1.5 text-xs font-bold text-indigo-600 disabled:opacity-50">刷新 TMDB</button><button onClick={() => { setMovieCollectionDetail(null); setMovieCandidates([]); }} className="text-xl text-gray-400">×</button></div></div>
      <label className="mt-4 flex items-center gap-2 rounded-xl bg-indigo-50 px-3 py-2 text-xs text-indigo-700"><input type="checkbox" checked={fillMissingMovieIdentity} onChange={event => setFillMissingMovieIdentity(event.target.checked)} />为已确认的旧电影仅补全缺失的 TMDB 身份（不覆盖已有值）</label>
      <div className="mt-3 flex-1 space-y-2 overflow-y-auto">{movieCandidates.map(candidate => {
        const movie = candidate.movie;
        const id = movie.id ?? 0;
        const checked = selectedMovieIds.has(id);
        const future = !!movie.release_date && new Date(`${movie.release_date}T00:00:00Z`) > new Date();
        const selectable = ['library', 'missing'].includes(candidate.status);
        const statusText = candidate.status === 'member' ? '已在当前收藏集' : candidate.status === 'library' ? '已在片库，可加入收藏集' : candidate.status === 'conflict' ? '身份冲突，需要先修正条目' : candidate.status === 'unresolved' ? '无法确认身份，请刷新重试' : future ? '尚未上映' : !movie.release_date ? '上映日期未知' : '片库缺失';
        return <label key={id} className={`flex items-center gap-3 rounded-2xl border p-3 ${selectable ? 'cursor-pointer hover:border-indigo-200' : 'bg-gray-50 opacity-70'}`}><input type="checkbox" disabled={!selectable || !id} checked={checked} onChange={() => setSelectedMovieIds(current => { const next = new Set(current); if (checked) next.delete(id); else next.add(id); return next; })} /><SafePosterImage posterPath={movie.poster_path || ''} alt="" compact className="h-16 w-11 rounded-lg object-cover" /><div className="min-w-0 flex-1"><p className="truncate text-sm font-bold text-gray-800">{movie.title || movie.name || '未命名电影'} · {movie.release_date?.slice(0, 4) || '年份未知'}</p><p className={`mt-1 text-xs ${candidate.status === 'conflict' ? 'font-semibold text-red-500' : 'text-gray-400'}`}>{statusText}</p></div></label>;
      })}</div>
      <div className="mt-4 flex items-center justify-between border-t pt-4"><span className="text-xs text-gray-500">已选择 {selectedMovieIds.size} 部</span><div className="flex gap-2"><button onClick={() => { setMovieCollectionDetail(null); setMovieCandidates([]); }} className="rounded-xl border px-4 py-2 text-sm">取消</button><button disabled={busy || selectedMovieIds.size === 0} onClick={() => void createSelectedMovies()} className="rounded-xl bg-indigo-600 px-5 py-2 text-sm font-bold text-white disabled:bg-gray-300">补充到片库</button></div></div>
    </div></div>}
    {linkingRelated && selected && <div className="fixed inset-0 z-[105] flex items-center justify-center bg-slate-900/45 p-4"><div role="dialog" aria-modal="true" aria-label="关联相关作品" className="flex max-h-[82vh] w-full max-w-xl flex-col rounded-3xl bg-white p-5 shadow-2xl">
      <div className="flex items-start justify-between"><div><h3 className="text-xl font-black">关联相关作品</h3><p className="mt-1 text-xs text-gray-400">搜索电影、衍生剧或特别篇；只有点击加入后才会写入</p></div><button onClick={() => setLinkingRelated(false)} className="text-xl text-gray-400">×</button></div>
      <div className="mt-4 flex gap-2"><input autoFocus value={relatedSearch} onChange={event => setRelatedSearch(event.target.value)} onKeyDown={event => { if (event.key === 'Enter') { event.preventDefault(); void searchRelatedWorks(); } }} placeholder="输入片名" className="min-w-0 flex-1 rounded-xl border px-4 py-2.5 text-sm" /><button disabled={busy || !relatedSearch.trim()} onClick={() => void searchRelatedWorks()} className="rounded-xl bg-indigo-600 px-4 text-sm font-bold text-white disabled:bg-gray-300">搜索</button></div>
      <div className="mt-3 flex-1 space-y-2 overflow-y-auto">{relatedResults.map(item => <div key={`${item.media_type}:${item.id}`} className="flex items-center gap-3 rounded-2xl border p-3"><SafePosterImage posterPath={item.poster_path || ''} alt="" compact className="h-16 w-11 rounded-lg object-cover" /><div className="min-w-0 flex-1"><p className="truncate text-sm font-bold">{item.name || item.title}</p><p className="mt-1 truncate text-xs text-gray-400">{item.media_type === 'movie' ? '电影' : '剧集'} · {(item.release_date || item.first_air_date)?.slice(0, 4) || '年份未知'}</p></div><button disabled={busy} onClick={() => void linkRelatedWork(item)} className="rounded-xl border border-indigo-200 px-3 py-2 text-xs font-bold text-indigo-600">加入</button></div>)}{!relatedResults.length && <p className="py-10 text-center text-sm text-gray-400">搜索后在这里选择需要关联的作品</p>}</div>
    </div></div>}
    {showDismissedSuggestions && <div className="fixed inset-0 z-[110] flex items-center justify-center bg-slate-900/45 p-4"><div role="dialog" aria-modal="true" aria-label="已忽略建议" className="flex max-h-[80vh] w-full max-w-lg flex-col rounded-3xl bg-white p-5 shadow-2xl">
      <div className="flex items-start justify-between gap-4"><div><h3 className="text-xl font-black text-gray-900">已忽略建议</h3><p className="mt-1 text-xs text-gray-400">恢复后需要重新扫描，并会重新核对最新 TMDB 数据</p></div><button onClick={() => setShowDismissedSuggestions(false)} aria-label="关闭已忽略建议" className="text-xl text-gray-400">×</button></div>
      <div className="mt-4 flex-1 space-y-2 overflow-y-auto">{dismissals.map(entry => <div key={entry.key} className="flex items-center gap-3 rounded-2xl border border-gray-100 p-3"><div className="min-w-0 flex-1"><p className="truncate text-sm font-bold text-gray-800">{entry.name}</p><p className="mt-1 text-[10px] text-gray-400">{entry.sourceKind === 'tmdb-tv-show' ? '电视剧系列' : entry.sourceKind === 'tmdb-movie-collection' ? '电影系列' : '本地建议'} · {new Date(entry.dismissedAt).toLocaleString()}</p></div><button disabled={busy} onClick={() => void restoreDismissal(entry.key)} className="rounded-xl border border-indigo-100 px-3 py-2 text-xs font-bold text-indigo-600 disabled:opacity-50">恢复</button></div>)}{dismissals.length === 0 && <p className="py-10 text-center text-sm text-gray-400">没有已忽略的建议</p>}</div>
      <div className="mt-4 flex justify-end gap-2 border-t pt-4"><button onClick={() => setShowDismissedSuggestions(false)} className="rounded-xl border px-4 py-2 text-sm">关闭</button><button disabled={busy || dismissals.length === 0} onClick={() => void restoreAllDismissals()} className="rounded-xl border border-red-100 px-4 py-2 text-sm font-bold text-red-500 disabled:opacity-40">恢复全部</button></div>
    </div></div>}
  </div>;
}
