import type { ReactNode } from 'react';
import type { SyncRuntimeState } from '../../../shared/lib/database';
import { BUILD_INFO } from '../../../shared/lib/buildInfo';
import MoreActionsMenu from './MoreActionsMenu';
import SyncStatusMenu from './SyncStatusMenu';

interface HeaderProps {
  searchText: string;
  onSearchChange: (value: string) => void;
  sortBy: 'createdAt' | 'endDate' | 'rating' | 'releaseYear' | 'watchValue';
  onSortByChange: (value: 'createdAt' | 'endDate' | 'rating' | 'releaseYear' | 'watchValue') => void;
  hasWebDAVCreds: boolean;
  syncing: boolean;
  syncMsg: string;
  syncRuntime: SyncRuntimeState | null;
  onQuickSync: () => void;
  isSyncPaused: boolean;
  onToggleSyncPause: () => void;
  viewMode: 'list' | 'poster';
  onViewModeChange: (value: 'list' | 'poster') => void;
  onShowDashboard: () => void;
  onShowCollections: () => void;
  onShowSettings: () => void;
  onShowSyncSettings: () => void;
  onShowForm: () => void;
  activeFilterCount: number;
  onShowAdvancedFilters: () => void;
  savedViewControl: ReactNode;
}

export default function Header({
  searchText,
  onSearchChange,
  sortBy,
  onSortByChange,
  hasWebDAVCreds,
  syncing,
  syncMsg,
  syncRuntime,
  onQuickSync,
  isSyncPaused,
  onToggleSyncPause,
  viewMode,
  onViewModeChange,
  onShowDashboard,
  onShowCollections,
  onShowSettings,
  onShowSyncSettings,
  onShowForm,
  activeFilterCount,
  onShowAdvancedFilters,
  savedViewControl,
}: HeaderProps) {
  return (
    <header className="sticky top-0 z-40 border-b border-gray-100 bg-white shadow-sm">
      <div
        aria-label="顶部工具栏"
        className="scrollbar-none mx-auto flex w-full max-w-7xl items-center gap-2 overflow-x-auto px-4 py-3"
      >
        <div className="flex shrink-0 items-center gap-2 whitespace-nowrap">
          <span className="text-2xl" aria-hidden="true">🎬</span>
          <span
            data-testid="build-commit"
            title={`Git 提交 ${BUILD_INFO.gitCommit}`}
            className="rounded-md bg-gray-100 px-1.5 py-0.5 font-mono text-[10px] text-gray-500"
          >
            git {BUILD_INFO.shortCommit}
          </span>
          <h1 className="text-lg font-bold text-gray-900">影视追踪</h1>
        </div>

        {savedViewControl}

        <div className="relative min-w-48 flex-1">
          <svg aria-hidden="true" className="absolute left-3 top-1/2 h-4 w-4 -translate-y-1/2 text-gray-400" fill="none" stroke="currentColor" viewBox="0 0 24 24">
            <path strokeLinecap="round" strokeLinejoin="round" strokeWidth={2} d="M21 21l-6-6m2-5a7 7 0 11-14 0 7 7 0 0114 0z" />
          </svg>
          <input
            type="search"
            placeholder="搜索电影、剧集..."
            value={searchText}
            onChange={event => onSearchChange(event.target.value)}
            className="h-9 w-full rounded-xl border border-gray-200 bg-gray-50 pl-9 pr-3 text-sm outline-none transition focus:border-indigo-400 focus:ring-2 focus:ring-indigo-100"
          />
        </div>

        <button
          type="button"
          onClick={onShowAdvancedFilters}
          className={`h-9 shrink-0 whitespace-nowrap rounded-xl border px-3 text-sm font-semibold ${activeFilterCount ? 'border-indigo-300 bg-indigo-50 text-indigo-700' : 'border-gray-200 text-gray-500 hover:bg-gray-50'}`}
        >
          高级筛选{activeFilterCount ? ` ${activeFilterCount}` : ''}
        </button>

        <select
          aria-label="排序方式"
          value={sortBy}
          onChange={event => onSortByChange(event.target.value as typeof sortBy)}
          className="h-9 shrink-0 cursor-pointer rounded-xl border border-gray-200 bg-white px-3 text-sm text-gray-600 outline-none focus:border-indigo-400"
        >
          <option value="createdAt">最新添加</option>
          <option value="endDate">完成时间</option>
          <option value="releaseYear">上映年份</option>
          <option value="rating">评分</option>
          <option value="watchValue">待看价值</option>
        </select>

        <SyncStatusMenu
          hasCredentials={hasWebDAVCreds}
          syncing={syncing}
          message={syncMsg}
          runtime={syncRuntime}
          paused={isSyncPaused}
          onSync={onQuickSync}
          onTogglePause={onToggleSyncPause}
          onOpenSettings={onShowSyncSettings}
        />

        <button
          type="button"
          onClick={onShowSettings}
          aria-label="设置"
          title="设置"
          className="flex h-9 w-9 shrink-0 items-center justify-center rounded-xl border border-gray-200 text-gray-500 transition-colors hover:bg-gray-50 hover:text-gray-700"
        >
          <svg aria-hidden="true" className="h-4 w-4" fill="none" stroke="currentColor" viewBox="0 0 24 24">
            <path strokeLinecap="round" strokeLinejoin="round" strokeWidth={2} d="M10.325 4.317c.426-1.756 2.924-1.756 3.35 0a1.724 1.724 0 002.573 1.066c1.543-.94 3.31.826 2.37 2.37a1.724 1.724 0 001.065 2.572c1.756.426 1.756 2.924 0 3.35a1.724 1.724 0 00-1.066 2.573c.94 1.543-.826 3.31-2.37 2.37a1.724 1.724 0 00-2.572 1.065c-.426 1.756-2.924 1.756-3.35 0a1.724 1.724 0 00-2.573-1.066c-1.543.94-3.31-.826-2.37-2.37a1.724 1.724 0 00-1.065-2.572c-1.756-.426-1.756-2.924 0-3.35a1.724 1.724 0 001.066-2.573c-.94-1.543.826-3.31 2.37-2.37.996.608 2.296.07 2.572-1.065z" />
            <path strokeLinecap="round" strokeLinejoin="round" strokeWidth={2} d="M15 12a3 3 0 11-6 0 3 3 0 016 0z" />
          </svg>
        </button>

        <MoreActionsMenu
          viewMode={viewMode}
          onViewModeChange={onViewModeChange}
          onShowDashboard={onShowDashboard}
          onShowCollections={onShowCollections}
          onShowForm={onShowForm}
        />
      </div>
    </header>
  );
}
