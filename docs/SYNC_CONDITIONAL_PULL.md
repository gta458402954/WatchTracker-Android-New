# WebDAV Conditional Pull Fast Path（S1-PC）

S1-PC 通过 HTTP `If-None-Match` / `304 Not Modified` 避免桌面客户端重复下载和解析未变化的 `records-v3.json`。它不改变 WebDAV schema、资源路径或任何同步实体 contract，也不是真正的 delta sync。

## Fast-path 条件

只有同步快照同时满足以下条件时，客户端才发送保存的 strong 或 weak ETag：

- 已有有效 baseline；
- 已有格式安全的 remote ETag；
- outbox clean；
- staging 为空；
- 没有 publish intent。

任一条件不满足（尤其是本地有待上传修改）时，继续使用原有完整流程：无条件 GET、三方合并，并在需要时执行安全的条件 PUT。

## 304 流程

```text
GET records-v3.json + If-None-Match
  → 304（不读取或解析 V3 body）
  → 检查 legacy records.json guard
  → 原子复核 target、generation、stored ETag 与 clean 状态
  → 仅记录 scheduler 成功状态和 legacy fingerprint
```

该路径不会合并或替换 records、逐集历史、收藏集，不会 PUT、创建恢复点、确认 outbox、结束 publish intent、清理 conflicts，也不会改写 baseline 或 remote ETag。服务器忽略 `If-None-Match` 并返回 200 时，客户端安全回到现有完整同步流程。

## 范围

本实现仅适用于 WatchTracker Windows/PC 客户端。Android 尚未实现 S1。S1 不修改 pull interval、focus cooldown、同步格式或远端资源布局。
