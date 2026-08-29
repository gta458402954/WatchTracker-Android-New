import { invoke } from '@tauri-apps/api/core';

export type DocumentExportResult =
  | { status: 'saved'; fileName: string }
  | { status: 'cancelled' };

interface AndroidDocumentExportDetail {
  requestId: string;
  status: 'saved' | 'cancelled' | 'error';
  fileName?: string;
  errorCode?: string;
}

interface AndroidDocumentExportBridge {
  exportJsonDocument(requestId: string, fileName: string, token: string): void;
}

declare global {
  interface Window {
    watchTrackerDocumentExport?: AndroidDocumentExportBridge;
  }
}

const RESULT_EVENT = 'watchtracker:document-export-result';

async function discardStage(token: string): Promise<void> {
  await invoke('discard_local_export_stage', { token }).catch(() => undefined);
}

export async function exportJsonDocument(
  suggestedName: string,
  json: string,
): Promise<DocumentExportResult> {
  const token = await invoke<string>('stage_local_export', { json });
  const bridge = window.watchTrackerDocumentExport;
  if (!bridge) {
    await discardStage(token);
    throw new Error('document_export_unavailable');
  }

  const requestId = crypto.randomUUID();
  return new Promise<DocumentExportResult>((resolve, reject) => {
    const onResult = (event: Event) => {
      const detail = (event as CustomEvent<AndroidDocumentExportDetail>).detail;
      if (!detail || detail.requestId !== requestId) return;
      window.removeEventListener(RESULT_EVENT, onResult);
      if (detail.status === 'saved') {
        resolve({ status: 'saved', fileName: detail.fileName || suggestedName });
      } else if (detail.status === 'cancelled') {
        resolve({ status: 'cancelled' });
      } else {
        reject(new Error(detail.errorCode || 'document_export_failed'));
      }
    };
    window.addEventListener(RESULT_EVENT, onResult);
    try {
      bridge.exportJsonDocument(requestId, suggestedName, token);
    } catch (error) {
      window.removeEventListener(RESULT_EVENT, onResult);
      void discardStage(token);
      reject(error);
    }
  });
}
