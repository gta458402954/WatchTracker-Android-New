import { useState } from 'react';
import { WatchRecord } from '../../../shared/types';
import { downloadPosterAsync, getTmdbCredentialStatus, searchTmdbAsync, getTmdbDetailAsync, getTmdbSeasonDetailAsync } from '../../../shared/lib/database';
import {
  classifyTmdb,
  inferPlatformFromTmdb,
  mergeContentTags,
  positiveEpisodeRuntimeOf,
  TmdbMedia,
  TmdbSeason,
} from '../../../shared/lib/classification';
import { publicFailureMessage, reportOperationFailure, type NoticeTone } from '../../../shared/lib/feedback';
import { seasonRecordMetadata, tmdbOriginalLanguageLocale } from '../../collections/lib/tmdbRecordMapping';
import { formatMovieTime } from './recordFormModel';

export type RecordFormValues = Omit<WatchRecord, 'id' | 'createdAt'>;

interface UseTmdbRecordSearchOptions {
  form: RecordFormValues;
  setForm: React.Dispatch<React.SetStateAction<RecordFormValues>>;
  isEpisodic: boolean;
  onNotify?: (tone: NoticeTone, message: string) => void;
}

export function useTmdbRecordSearch({ form, setForm, isEpisodic, onNotify }: UseTmdbRecordSearchOptions) {
  const [isSearching, setIsSearching] = useState(false);
  const [searchResults, setSearchResults] = useState<TmdbMedia[]>([]);
  const [searchError, setSearchError] = useState<string | null>(null);
  const [showResults, setShowResults] = useState(false);
  const [seasons, setSeasons] = useState<TmdbSeason[]>([]);
  const [selectedSeries, setSelectedSeries] = useState<TmdbMedia | null>(null);

  function downloadPosterInBackground(poster: string) {
    void downloadPosterAsync(poster).catch(error => {
      reportOperationFailure('RecordForm.DownloadPoster', error);
      onNotify?.('warning', '元数据已保留，但海报下载失败，可以稍后重试。');
    });
  }

  async function handleTMDBSearch() {
    let query = form.imdbId || form.chineseName || form.originalName;
    if (!query) return;
    query = query.replace(/第\s*\d+\s*季/g, '').replace(/Season\s*\d+/gi, '').replace(/第\s*[一二三四五六七八九十]+\s*季/g, '').trim();
    if (!(await getTmdbCredentialStatus()).available) {
      setSearchError('请先在设置中配置 TMDB API KEY');
      setShowResults(true);
      return;
    }
    setIsSearching(true);
    setShowResults(true);
    setSearchError(null);
    setSeasons([]);
    setSelectedSeries(null);
    try {
      const result = await searchTmdbAsync({ query, language: 'zh-CN' });
      if (!result.success) {
        setSearchError(publicFailureMessage('搜索 TMDB'));
        return;
      }
      setSearchResults(result.results ?? []);
    } catch (error) {
      reportOperationFailure('RecordForm.SearchTmdb', error);
      setSearchError(publicFailureMessage('搜索 TMDB'));
    } finally {
      setIsSearching(false);
    }
  }

  async function handleSelectResult(item: TmdbMedia, setMovieDurationStr: (value: string) => void) {
    if (!(await getTmdbCredentialStatus()).available) return;
    const type = item.media_type || (isEpisodic ? 'tv' : 'movie');
    const isSeason = type === 'tv_season';
    const isEpisode = type === 'tv_episode';
    const isTV = type === 'tv' || isSeason || isEpisode;
    const fetchId = isSeason || isEpisode ? item.show_id : item.id;
    const fetchType: 'movie' | 'tv' = isTV ? 'tv' : 'movie';
    if (fetchId == null) {
      setSearchError('TMDB 结果缺少 ID');
      return;
    }
    try {
      setSearchError(null);
      const result = await getTmdbDetailAsync({ id: fetchId, mediaType: fetchType, language: 'zh-CN' });
      if (!result.success || !result.data) {
        setSearchError(publicFailureMessage('获取 TMDB 详情'));
        return;
      }
      const detail = result.data;
      if (isSeason || isEpisode) {
        const targetSeason = detail.seasons?.find((s: TmdbSeason) => s.season_number === item.season_number) || item;
        const originalSeasonResult = targetSeason.season_number == null ? null : await getTmdbSeasonDetailAsync({ seriesId: fetchId, seasonNumber: targetSeason.season_number, language: tmdbOriginalLanguageLocale(detail.original_language) });
        const originalSeason = originalSeasonResult?.success ? originalSeasonResult.data : undefined;
        const poster = targetSeason.poster_path || detail.poster_path || null;
        if (poster) downloadPosterInBackground(poster);
        const query = form.imdbId || form.chineseName || form.originalName || '';
        const updates: Partial<RecordFormValues> = {
          ...seasonRecordMetadata({ ...detail, id: detail.id ?? fetchId }, targetSeason, form, originalSeason),
          imdbId: query.startsWith('tt') ? query : (detail.external_ids?.imdb_id || detail.imdb_id || form.imdbId),
          tmdbParentId: fetchId,
        };
        if (isEpisode && item.episode_number) updates.progress = `S${String(item.season_number).padStart(2, '0')}E${String(item.episode_number).padStart(2, '0')}`;
        setForm(prev => ({ ...prev, ...updates }));
        setShowResults(false);
        return;
      }
      if (isTV && detail.seasons && detail.seasons.length > 0) {
        setSelectedSeries(detail);
        setSeasons(detail.seasons.filter((s: TmdbSeason) => (s.season_number ?? 0) > 0));
        return;
      }
      const dateStr = detail.release_date || detail.first_air_date || '';
      const year = dateStr.split('-')[0] || null;
      const poster = detail.poster_path || item.poster_path || null;
      if (poster) downloadPosterInBackground(poster);
      const classification = classifyTmdb(detail, isTV, form.mediaType);
      const networkName = inferPlatformFromTmdb(classification.originCountry, detail.networks?.[0]?.name || detail.production_companies?.[0]?.name);
      const updates: Partial<RecordFormValues> = {
        chineseName: detail.name || detail.title || item.name || item.title || form.chineseName,
        originalName: detail.original_name || detail.original_title || item.original_name || item.original_title || form.originalName,
        releaseYear: year, posterPath: poster, genres: classification.genres, originCountry: classification.originCountry,
        imdbRating: detail.vote_average || null, tmdbStatus: detail.status || null, mediaType: classification.mediaType,
        contentTags: mergeContentTags(form.contentTags, classification.contentTags), tmdbMediaKind: isTV ? 'tv' : 'movie',
        tmdbId: detail.id ?? fetchId, tmdbParentId: null, tmdbSeasonNumber: null, seriesRecordKind: isTV ? 'whole-series' : 'single-work',
      };
      if (networkName && (!form.platform || form.platform.trim() === '')) updates.platform = networkName;
      if (isTV) {
        updates.totalEpisodes = detail.number_of_episodes || null;
        const episodeRuntime = positiveEpisodeRuntimeOf(detail);
        if (episodeRuntime !== null) updates.episodeRuntime = episodeRuntime;
      } else {
        updates.movieDuration = detail.runtime ? detail.runtime * 60 : null;
        setMovieDurationStr(updates.movieDuration != null ? formatMovieTime(updates.movieDuration) : '');
      }
      if (detail.external_ids?.imdb_id) updates.imdbId = detail.external_ids.imdb_id;
      else if (detail.imdb_id) updates.imdbId = detail.imdb_id;
      setForm(prev => ({ ...prev, ...updates }));
      setShowResults(false);
    } catch (error) {
      reportOperationFailure('RecordForm.SelectTmdbResult', error);
      setSearchError(publicFailureMessage('读取 TMDB 详情'));
      onNotify?.('error', publicFailureMessage('读取 TMDB 详情'));
    }
  }

  async function handleSelectSeason(season: TmdbSeason) {
    if (!selectedSeries) return;
    const originalSeasonResult = selectedSeries.id == null || season.season_number == null ? null : await getTmdbSeasonDetailAsync({ seriesId: selectedSeries.id, seasonNumber: season.season_number, language: tmdbOriginalLanguageLocale(selectedSeries.original_language) });
    const originalSeason = originalSeasonResult?.success ? originalSeasonResult.data : undefined;
    const poster = season.poster_path || selectedSeries.poster_path || null;
    if (poster) downloadPosterInBackground(poster);
    setForm(prev => ({ ...prev, ...seasonRecordMetadata(selectedSeries, season, prev, originalSeason) }));
    setShowResults(false);
    setSeasons([]);
    setSelectedSeries(null);
  }

  return { isSearching, searchResults, searchError, showResults, setShowResults, seasons, setSeasons, selectedSeries, setSelectedSeries, handleTMDBSearch, handleSelectResult, handleSelectSeason };
}
