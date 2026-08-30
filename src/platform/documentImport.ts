import { invoke } from '@tauri-apps/api/core';

export type DocumentImportSelection =
  | { status: 'selected'; token: string; fileName: string; sizeBytes: number }
  | { status: 'cancelled' };

interface AndroidDocumentImportDetail {
  requestId: string;
  status: 'selected' | 'cancelled' | 'error';
  token?: string;
  fileName?: string;
  sizeBytes?: number;
  errorCode?: string;
}

interface AndroidDocumentImportBridge {
  selectJsonDocument(requestId: string): void;
}

declare global {
  interface Window {
    watchTrackerDocumentImport?: AndroidDocumentImportBridge;
  }
}

const RESULT_EVENT = 'watchtracker:document-import-result';

export async function discardImportedDocument(token: string): Promise<void> {
  await invoke('discard_local_import_stage', { token });
}

export async function selectJsonDocument(): Promise<DocumentImportSelection> {
  const bridge = window.watchTrackerDocumentImport;
  if (!bridge) throw new Error('document_import_unavailable');
  const requestId = crypto.randomUUID();
  return new Promise<DocumentImportSelection>((resolve, reject) => {
    const onResult = (event: Event) => {
      const detail = (event as CustomEvent<AndroidDocumentImportDetail>).detail;
      if (!detail || detail.requestId !== requestId) return;
      window.removeEventListener(RESULT_EVENT, onResult);
      if (detail.status === 'cancelled') {
        resolve({ status: 'cancelled' });
      } else if (
        detail.status === 'selected'
        && typeof detail.token === 'string'
        && typeof detail.fileName === 'string'
        && typeof detail.sizeBytes === 'number'
      ) {
        resolve({
          status: 'selected',
          token: detail.token,
          fileName: detail.fileName,
          sizeBytes: detail.sizeBytes,
        });
      } else {
        reject(new Error(detail.errorCode || 'document_import_failed'));
      }
    };
    window.addEventListener(RESULT_EVENT, onResult);
    try {
      bridge.selectJsonDocument(requestId);
    } catch (error) {
      window.removeEventListener(RESULT_EVENT, onResult);
      reject(error);
    }
  });
}
