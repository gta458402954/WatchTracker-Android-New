import { useCallback, useEffect, useMemo, useState } from 'react';
import type { EpisodeCompletion, WatchRecord } from '../../../shared/types/index.ts';
import { getEpisodeTracking, type EpisodeTracking } from '../../../shared/lib/database.ts';
import {
  episodeAdjustmentAction,
  mobileEpisodeOptions,
  mobileEpisodeSummary,
  nextEpisodeAfterCompletion,
  resumableNextEpisode,
  type MobileEpisodeAction,
} from '../mobileEpisodeTracking.ts';

export type MobileEpisodeActionHandler = (
  record: WatchRecord,
  nextEpisode: number | null,
  action: MobileEpisodeAction,
) => Promise<EpisodeTracking | null>;

function historyText(item: EpisodeCompletion): string {
  if (!item.completedAt) return `第 ${item.episodeNumber} 集 · 完成时间未知（跳集记录）`;
  const date = new Date(item.completedAt);
  return `第 ${item.episodeNumber} 集 · ${Number.isNaN(date.getTime()) ? item.completedAt : date.toLocaleString('zh-CN')}`;
}

export function MobileEpisodeCardStatus({ record }: { record: WatchRecord }) {
  const summary = mobileEpisodeSummary(record);
  return summary ? <p className="mobile-episode-card-status">{summary}</p> : null;
}

export function MobileEpisodeQuickAction({ record, onAction, onOpen }: { record: WatchRecord; onAction: MobileEpisodeActionHandler; onOpen: () => void }) {
  const [busy, setBusy] = useState(false);
  const current = record.episodeTrackingEnabled ? record.nextEpisode : null;
  if (record.isLocked) return null;
  if (!record.episodeTrackingEnabled && record.status === '已看') return null;
  if (typeof current !== 'number') {
    if (record.episodeTrackingEnabled) return null;
    return <button type="button" className="mobile-quiet-button" onClick={onOpen}>逐集设置</button>;
  }
  if (!mobileEpisodeOptions(record).includes(current)) return null;
  const target = nextEpisodeAfterCompletion(record);
  return <button
    type="button"
    className="mobile-primary-button mobile-episode-quick-button"
    aria-label={`完成第 ${current} 集`}
    disabled={busy}
    onClick={async () => {
      setBusy(true);
      await onAction(record, target, 'complete');
      setBusy(false);
    }}
  >{busy ? '记录中…' : `完成第 ${current} 集`}</button>;
}

export default function MobileEpisodeTracking({ record, onAction }: { record: WatchRecord; onAction: MobileEpisodeActionHandler }) {
  const options = useMemo(() => mobileEpisodeOptions(record), [record]);
  const initialSelection = record.episodeTrackingEnabled && typeof record.nextEpisode === 'number' ? record.nextEpisode : 1;
  const [selectedEpisode, setSelectedEpisode] = useState(initialSelection);
  const [completions, setCompletions] = useState<EpisodeCompletion[] | null>(null);
  const [historyError, setHistoryError] = useState(false);
  const [historyLoading, setHistoryLoading] = useState(false);
  const [busyAction, setBusyAction] = useState<MobileEpisodeAction | null>(null);
  const selected = options.includes(selectedEpisode) ? selectedEpisode : (options[0] ?? 1);

  const loadHistory = useCallback(async () => {
    if (!record.episodeTrackingEnabled) {
      setCompletions([]);
      setHistoryError(false);
      return;
    }
    setHistoryLoading(true);
    setHistoryError(false);
    try {
      const tracking = await getEpisodeTracking(record.id);
      setCompletions(tracking.completions);
    } catch {
      setHistoryError(true);
    } finally {
      setHistoryLoading(false);
    }
  }, [record.episodeTrackingEnabled, record.id]);

  useEffect(() => {
    if (!record.episodeTrackingEnabled) return;
    // The effect intentionally starts an external Rust IPC read.
    // eslint-disable-next-line react-hooks/set-state-in-effect
    void loadHistory();
  }, [loadHistory, record.episodeTrackingEnabled, record.rev]);

  const run = async (nextEpisode: number | null, action: MobileEpisodeAction) => {
    setBusyAction(action);
    const tracking = await onAction(record, nextEpisode, action);
    if (tracking) {
      setCompletions(tracking.completions);
      if (typeof tracking.record.nextEpisode === 'number') setSelectedEpisode(tracking.record.nextEpisode);
    }
    setBusyAction(null);
  };

  const summary = mobileEpisodeSummary(record);
  if (!summary) return null;
  const locked = Boolean(record.isLocked);
  const current = record.episodeTrackingEnabled ? record.nextEpisode : null;
  const currentIsValid = typeof current === 'number' && options.includes(current);
  const resumeAt = completions ? resumableNextEpisode(record, completions) : null;
  const adjustment = episodeAdjustmentAction(record, selected);
  const canAdjust = record.episodeTrackingEnabled && selected !== current;
  const adjustmentLabel = adjustment === 'retreat' ? `回退至第 ${selected} 集` : `跳至第 ${selected} 集`;

  return <section className="mobile-episode-panel" aria-labelledby="mobile-episode-title">
    <div className="mobile-episode-heading"><div><p className="mobile-eyebrow">EPISODE TRACKING</p><h2 id="mobile-episode-title">逐集跟踪</h2></div><span>{record.totalEpisodes} 集</span></div>
    <p className="mobile-episode-summary" aria-live="polite">{summary}</p>
    {record.progress && !record.episodeTrackingEnabled && <p className="mobile-episode-legacy">旧进度“{record.progress}”会原样保留，不会转换为逐集历史。</p>}
    {locked && <p className="mobile-locked-hint">🔒 记录已锁定，逐集进度和历史均为只读。</p>}

    {!record.episodeTrackingEnabled && record.status !== '已看' && <div className="mobile-episode-control">
      <label>初始下一集<select aria-label="初始下一集" value={selected} disabled={locked || busyAction !== null} onChange={event => setSelectedEpisode(Number(event.target.value))}>{options.map(episode => <option key={episode} value={episode}>第 {episode} 集</option>)}</select></label>
      <button type="button" className="mobile-primary-button" disabled={locked || busyAction !== null} onClick={() => void run(selected, 'enable')}>{busyAction === 'enable' ? '启用中…' : '启用逐集跟踪'}</button>
    </div>}

    {!record.episodeTrackingEnabled && record.status === '已看' && <p className="mobile-episode-note">这条记录已经完成且没有逐集历史。为避免补造历史，不会自动启用跟踪。</p>}

    {record.episodeTrackingEnabled && current !== null && !currentIsValid && <p className="mobile-episode-note" role="alert">当前下一集超出总集数或缺失，逐集写入已停用。请先检查记录的总集数。</p>}

    {record.episodeTrackingEnabled && currentIsValid && <>
      <button type="button" className="mobile-primary-button mobile-episode-complete-button" disabled={locked || busyAction !== null} onClick={() => void run(nextEpisodeAfterCompletion(record), 'complete')}>{busyAction === 'complete' ? '记录中…' : `完成第 ${current} 集`}</button>
      <div className="mobile-episode-control">
        <label>调整下一集<select aria-label="调整下一集" value={selected} disabled={locked || busyAction !== null} onChange={event => setSelectedEpisode(Number(event.target.value))}>{options.map(episode => <option key={episode} value={episode}>第 {episode} 集</option>)}</select></label>
        <button type="button" className="mobile-quiet-button" disabled={locked || busyAction !== null || !canAdjust} onClick={() => void run(selected, adjustment)}>{adjustmentLabel}</button>
      </div>
      <button type="button" className="mobile-quiet-button mobile-episode-finish-button" disabled={locked || busyAction !== null} onClick={() => void run(null, 'finish')}>{busyAction === 'finish' ? '完结中…' : '将全部集数标为完结'}</button>
    </>}

    {record.episodeTrackingEnabled && current === null && <>
      {resumeAt !== null && <div className="mobile-episode-resume"><p>发现新增 {Number(record.totalEpisodes) - resumeAt + 1} 集，可以从第 {resumeAt} 集继续追更。</p><button type="button" className="mobile-primary-button" disabled={locked || busyAction !== null} onClick={() => void run(resumeAt, 'resume')}>{busyAction === 'resume' ? '恢复中…' : `继续追更第 ${resumeAt} 集`}</button></div>}
      <div className="mobile-episode-control">
        <label>回退下一集<select aria-label="回退下一集" value={selected} disabled={locked || busyAction !== null} onChange={event => setSelectedEpisode(Number(event.target.value))}>{options.map(episode => <option key={episode} value={episode}>第 {episode} 集</option>)}</select></label>
        <button type="button" className="mobile-quiet-button" disabled={locked || busyAction !== null} onClick={() => void run(selected, 'retreat')}>{`回退至第 ${selected} 集`}</button>
      </div>
    </>}

    {record.episodeTrackingEnabled && <div className="mobile-episode-history">
      <div className="mobile-episode-history-heading"><h3>逐集历史</h3><button type="button" className="mobile-quiet-button" disabled={historyLoading} onClick={() => void loadHistory()}>{historyLoading ? '读取中…' : '重新读取'}</button></div>
      {historyError && <p role="alert">逐集历史读取失败。当前页面内容已保留，可以重新读取。</p>}
      {!historyError && completions !== null && (completions.length === 0 ? <p>尚无逐集历史。</p> : <ol>{completions.map(item => <li key={item.id}>{historyText(item)}</li>)}</ol>)}
    </div>}
  </section>;
}
