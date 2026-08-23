import { useState } from 'react';
import type { WatchRecord } from '../../../shared/types';
import { getAllEpisodeCompletions, getCollections, getCollectionMembers, replaceLibrary, replaceLibraryV3 } from '../../../shared/lib/database';
import { normalizeImportedRecords } from '../../../shared/lib/importValidation';
import type { NoticeTone } from '../../../shared/lib/feedback';

interface Options {
  records: WatchRecord[];
  onImport: (records: WatchRecord[]) => void | Promise<void>;
  onDatabaseRestored: () => Promise<WatchRecord[]>;
  onNotify?: (tone: NoticeTone, message: string) => void;
  showFailure: (scope: string, action: string, error: unknown, setStatus?: (message: string) => void) => void;
  showSuccess: (message: string) => void;
}

export function useImportExport({ records, onImport, onDatabaseRestored, onNotify, showFailure, showSuccess }: Options) {
  const [importStatus, setImportStatus] = useState('');
  async function handleExport() {
    try {
      const [episodeCompletions, collections, collectionMembers] = await Promise.all([getAllEpisodeCompletions(), getCollections(), getCollectionMembers()]);
      const data = JSON.stringify({ formatVersion: 4, exportedAt: new Date().toISOString(), records, episodeCompletions, collections, collectionMembers }, null, 2);
      const blob = new Blob([data], { type: 'application/json' }); const url = URL.createObjectURL(blob); const a = document.createElement('a');
      a.href = url; a.download = `影视追踪_${new Date().toISOString().slice(0, 10)}.json`; a.click(); a.remove(); URL.revokeObjectURL(url);
    } catch (error) { showFailure('Settings.ExportLocal', '导出本地数据', error, setImportStatus); }
  }
  function handleImportLocal() {
    const input = document.createElement('input'); input.type = 'file'; input.accept = '.json';
    input.onchange = event => {
      const file = (event.target as HTMLInputElement).files?.[0]; if (!file) return;
      const reader = new FileReader();
      reader.onload = async () => {
        try {
          if (typeof reader.result !== 'string') throw new Error('无法读取文件内容');
          const parsed: unknown = JSON.parse(reader.result);
          if (parsed && typeof parsed === 'object' && !Array.isArray(parsed) && [3, 4].includes((parsed as { formatVersion?: number }).formatVersion ?? -1)) {
            const envelope = parsed as { records?: unknown; episodeCompletions?: unknown; collections?: unknown; collectionMembers?: unknown };
            const completeData = normalizeImportedRecords(envelope.records);
            if (!Array.isArray(envelope.episodeCompletions) || !Array.isArray(envelope.collections) || !Array.isArray(envelope.collectionMembers)) throw new Error('收藏集备份格式无效');
            await replaceLibraryV3(completeData, envelope.episodeCompletions as import('../../../shared/types').EpisodeCompletion[], envelope.collections as import('../../../shared/types').WatchCollection[], envelope.collectionMembers as import('../../../shared/types').CollectionMember[]);
            await onDatabaseRestored(); showSuccess(`已导入 ${completeData.length} 条记录、${envelope.episodeCompletions.length} 条逐集历史和 ${envelope.collections.length} 个收藏集。`);
          } else if (parsed && typeof parsed === 'object' && !Array.isArray(parsed) && (parsed as { formatVersion?: unknown }).formatVersion === 2) {
            const envelope = parsed as { records?: unknown; episodeCompletions?: unknown }; const completeData = normalizeImportedRecords(envelope.records);
            if (!Array.isArray(envelope.episodeCompletions)) throw new Error('逐集历史格式无效');
            await replaceLibrary(completeData, envelope.episodeCompletions as import('../../../shared/types').EpisodeCompletion[]);
            await onDatabaseRestored(); showSuccess(`已导入 ${completeData.length} 条记录及 ${envelope.episodeCompletions.length} 条逐集历史。`);
          } else {
            const completeData = normalizeImportedRecords(parsed); await onImport(completeData);
            onNotify?.('info', '旧格式文件不含逐集历史；匹配条目的现有逐集历史已保留。'); showSuccess(`已导入 ${completeData.length} 条本地记录。`);
          }
        } catch (error) { showFailure('Settings.ImportLocal', '导入本地文件', error, setImportStatus); }
      }; reader.readAsText(file);
    }; input.click();
  }
  return { importStatus, setImportStatus, handleExport, handleImportLocal };
}
