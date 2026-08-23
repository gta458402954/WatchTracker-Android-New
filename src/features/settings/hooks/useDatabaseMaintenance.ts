import { useState } from 'react';
import { vacuumDbAsync } from '../../../shared/lib/database';

interface Options {
  showFailure: (scope: string, action: string, error: unknown, setStatus?: (message: string) => void) => void;
  showSuccess: (message: string) => void;
}

export function useDatabaseMaintenance({ showFailure, showSuccess }: Options) {
  const [vacuumStatus, setVacuumStatus] = useState('');
  async function handleVacuum() {
    setVacuumStatus('正在压缩数据库...');
    try { await vacuumDbAsync(); setVacuumStatus('✅ 数据库压缩完成'); showSuccess('数据库压缩完成。'); }
    catch (error) { showFailure('Settings.Vacuum', '压缩数据库', error, setVacuumStatus); }
    setTimeout(() => setVacuumStatus(''), 3000);
  }
  return { vacuumStatus, handleVacuum };
}
