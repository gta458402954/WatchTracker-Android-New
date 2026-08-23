export interface BuildInfoInput {
  productVersion?: string | null;
  gitCommit?: string | null;
  gitCommitTime?: string | null;
}

export interface BuildInfo {
  productVersion: string;
  gitCommit: string;
  shortCommit: string;
  gitCommitTime: string | null;
  fallback: boolean;
}

const GIT_COMMIT_PATTERN = /^[0-9a-f]{7,40}$/i;

export function createBuildInfo(input: BuildInfoInput | null | undefined): BuildInfo {
  const productVersion = input?.productVersion?.trim() || '0.0.0-dev';
  const candidateCommit = input?.gitCommit?.trim() ?? '';
  const gitCommit = GIT_COMMIT_PATTERN.test(candidateCommit) ? candidateCommit.toLowerCase() : 'unknown';
  const candidateTime = input?.gitCommitTime?.trim() ?? '';
  const gitCommitTime = candidateTime && Number.isFinite(Date.parse(candidateTime)) ? candidateTime : null;
  return {
    productVersion,
    gitCommit,
    shortCommit: gitCommit === 'unknown' ? 'unknown' : gitCommit.slice(0, 8),
    gitCommitTime,
    fallback: gitCommit === 'unknown' || gitCommitTime === null,
  };
}

export function formatGitCommitTime(value: string | null, locale?: string): string {
  if (!value) return '开发环境不可用';
  const parsed = new Date(value);
  if (!Number.isFinite(parsed.getTime())) return '开发环境不可用';
  return new Intl.DateTimeFormat(locale, {
    year: 'numeric', month: '2-digit', day: '2-digit',
    hour: '2-digit', minute: '2-digit', second: '2-digit',
    timeZoneName: 'short',
  }).format(parsed);
}
