# WebDAV Conditional Pull Fast Path（S1-PC）

clean snapshot 的拉取顺序为：

```text
DAV PROPFIND records-v3.json
  ├─ getetag 与本地 remoteEtag 相同 → legacy guard → narrow unchanged
  │                                  （不下载或解析 v3 body）
  ├─ validator 改变 → 完整 GET → 三方 merge → 必要时条件 PUT
  └─ PROPFIND 不支持或没有可用 validator
       └─ GET + If-None-Match
            ├─ 304 → narrow unchanged
            ├─ 200 且可靠 validator 相同 → narrow unchanged
            └─ 200 且改变/无法确认 → 完整 GET merge 流程
```

只有快照同时满足以下条件才进入 clean preflight：已有 baseline、格式安全的
strong/weak remote ETag、outbox clean、staging 为空且没有 publish intent。dirty
local state 继续走原有完整 GET、合并和安全条件 PUT 协议。

坚果云实测会接受 GET `If-None-Match`，但 ETag 未变化时仍可能返回 HTTP 200；因此
WatchTracker 对坚果云主要使用 DAV `getetag` 作为 clean pull fast path。该行为不代表
所有 WebDAV 服务都如此。PROPFIND 同样返回不可用时，仍保留 HTTP conditional GET
fallback。

所有 unchanged shortcut 都继续检查 legacy `records.json` guard；因此目标存在 legacy
文件时仍可能下载 legacy body，并在 fingerprint 改变时报告 `legacy_remote_changed`。
unchanged 路径只更新 scheduler 成功状态和必要的 legacy fingerprint，不合并或替换
业务数据，不 ack outbox、不清理 staging/publish intent、不改 baseline、remote ETag 或
conflicts，也不创建 recovery point。窄范围 Rust 提交继续以 generation 做 TOCTOU 校验。

本轮未改变云端数据格式、资源路径、同步实体 contract、拉取周期或 Android 实现范围。
