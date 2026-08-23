import type { UpdateWatchRecord } from '../types';

export function assertValidUpdateNumbers(updates: UpdateWatchRecord): void {
  for (const [field, value] of Object.entries(updates)) {
    if (typeof value === 'number' && !Number.isFinite(value)) {
      throw new Error(`Invalid non-finite number provided for field: ${field}`);
    }
  }
}
