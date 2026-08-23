export type NoticeTone = 'success' | 'error' | 'warning' | 'info';

export interface NoticeInput {
  tone: NoticeTone;
  message: string;
}

export type NoticeSink = (tone: NoticeTone, message: string) => void;

export function publicFailureMessage(action: string): string {
  return `${action}失败，请稍后重试。`;
}

export function errorCategory(error: unknown): string {
  if (error instanceof Error) return error.name || 'Error';
  if (error === null) return 'null';
  return typeof error;
}

export function reportOperationFailure(scope: string, error: unknown): void {
  console.error(`[${scope}] operation failed`, { errorCategory: errorCategory(error) });
}

export function notifyOperationFailure(
  scope: string,
  action: string,
  error: unknown,
  notify: NoticeSink,
): string {
  reportOperationFailure(scope, error);
  const message = publicFailureMessage(action);
  notify('error', message);
  return message;
}
