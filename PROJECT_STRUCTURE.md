# WatchTracker 项目结构

WatchTracker 是 React 19 + TypeScript + Vite + TailwindCSS 前端与 Tauri 2 + Rust + SQLite 后端组成的便携桌面应用。

## 目录

```text
WatchTracker/
├── src/
│   ├── app/App.tsx
│   ├── app/initialization.ts
│   ├── features/
│   │   ├── dashboard/components/Dashboard.tsx
│   │   ├── settings/components/SettingsModal.tsx
│   │   └── watchlist/
│   │       ├── components/
│   │       └── hooks/useWatchList.ts
│   ├── shared/
│   │   ├── components/ErrorBoundary.tsx
│   │   ├── components/NotificationRegion.tsx
│   │   ├── lib/
│   │   │   ├── analytics.ts
│   │   │   ├── classification.ts
│   │   │   ├── constants.ts
│   │   │   ├── database.ts
│   │   │   ├── feedback.ts
│   │   │   ├── filtering.ts
│   │   │   ├── importValidation.ts
│   │   │   └── webdav.ts
│   │   └── types/index.ts
│   ├── index.css
│   └── main.tsx
├── src-tauri/
│   ├── src/
│   │   ├── app_paths.rs
│   │   ├── auth.rs
│   │   ├── commands.rs
│   │   ├── db.rs
│   │   ├── db_atomic_crud.rs
│   │   ├── db_atomic_helpers.rs
│   │   ├── db_atomic_update.rs
│   │   ├── lib.rs
│   │   ├── main.rs
│   │   ├── models.rs
│   │   └── net.rs
│   ├── capabilities/
│   ├── Cargo.toml
│   └── tauri.conf.json
├── docs/
├── index.html
├── package.json
└── vite.config.ts
```

## 模块职责

- `src/app/App.tsx`：应用级弹窗、筛选、排序和页面布局。
- `src/app/initialization.ts`：`loading | ready | error` 初始化和重试边界。
- `features/watchlist`：记录表单、卡片、列表、海报墙及当前 `useWatchList` 状态/CRUD/同步调度。
- `features/dashboard`：时间范围统计、完成趋势、推荐与题材分布。
- `features/settings`：TMDB、代理、WebDAV、冲突恢复、备份和数据库工具。
- `shared/lib/classification.ts`：媒体类型、地区标签和 TMDB 分类的唯一规则来源。
- `shared/lib/database.ts`：前端到 Tauri 命令的类型化调用。
- `shared/lib/webdav.ts`：schema v2 凭据读写、墓碑删除、时间戳双端合并和冲突历史。
- `src-tauri/src/app_paths.rs`：数据库、日志、海报和备份共享的数据路径规则。
- `src-tauri/src/db.rs`：V18 SQLite 建表、迁移和基础访问。
- `src-tauri/src/db_atomic_*.rs`：本地记录、Tombstone 和 generation 的事务写入。
- `src-tauri/src/net.rs`：TMDB、海报下载和 HTTP 客户端。

## 数据约定

- `mediaType` 是业务分类的主字段：电影、剧集、纪录片、综艺、动画。
- `genres` 保存 TMDB 题材；`contentTags` 保存地区及用户自定义标签。
- 电影进度与总时长以秒存储；剧集单集时长以分钟存储。
- `updatedAt` 是 WebDAV 合并判断依据，删除通过墓碑同步。
- SQLite 物理列当前使用 V18 camelCase；当前源码不支持已经迁移为 V19 snake_case 的活动数据库。
- 当前没有 Zustand store；高级 ETag/generation 同步提交仍属于路线图。

## 验证命令

```powershell
npm run lint
npm run build
cargo test --manifest-path src-tauri/Cargo.toml --all-targets
cargo clippy --manifest-path src-tauri/Cargo.toml --all-targets -- -D warnings
npm run tauri build
```

构建产物位于 `src-tauri/target/release/app.exe`，发布时复制为便携目录中的 `watch-tracker.exe`。
