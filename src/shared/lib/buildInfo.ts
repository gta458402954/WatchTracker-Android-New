import { createBuildInfo } from './buildInfoCore';

declare const __WATCHTRACKER_BUILD_INFO__: {
  productVersion?: string;
  gitCommit?: string;
  gitCommitTime?: string;
} | undefined;

const injectedBuildInfo = typeof __WATCHTRACKER_BUILD_INFO__ === 'undefined'
  ? undefined
  : __WATCHTRACKER_BUILD_INFO__;

/** The single frontend source for product and source-revision identity. */
export const BUILD_INFO = createBuildInfo(injectedBuildInfo);
