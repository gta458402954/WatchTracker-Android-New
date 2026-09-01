# WebDAV Conditional Pull Fast Path（S1-PC）

clean snapshot 的拉取顺序为：

```text
DAV PROPFIND records-v3.json
  ├─ getetag 与本地 remoteEtag 相同 → legacy guard → narrow unchanged
  │                                  （不下载或解析 v3 body）
  ├─ validator 改变 → 完整 GET → 三方 merge → 必要时条件 PUT
  └─ PROPFIND 不支持或没有可用 validator
       └─ GET Range: bytes=0-0 metadata probe
            ├─ 206 + 合法 Content-Range + 恰好 1 byte + 相同安全 ETag
            │    → legacy guard → narrow unchanged
            ├─ 206 + 合法 metadata + 改变 ETag → 完整 GET merge 流程
            └─ probe 不可用 → GET + If-None-Match
                 ├─ 304 → narrow unchanged
                 ├─ 200 且可靠 validator 相同 → narrow unchanged
                 └─ 200 且改变/无法确认 → 完整 GET merge 流程
```

只有快照同时满足以下条件才进入 clean preflight：已有 baseline、格式安全的
strong/weak remote ETag、outbox clean、staging 为空且没有 publish intent。dirty
local state 继续走原有完整 GET、合并和安全条件 PUT 协议。

坚果云实测会接受 GET `If-None-Match`，但 ETag 未变化时仍可能返回 HTTP 200；因此
WatchTracker 优先使用 DAV `getetag` 作为 clean pull fast path。当服务不提供
`DAV:getetag` 时，再使用标准的 `GET Range: bytes=0-0` metadata probe；只有 HTTP 206、
`Content-Range: bytes 0-0/<total>` 合法、响应正文恰好 1 byte 且 ETag 可安全规范化时，
才用它判断远端是否改变。Range probe 只是 change detector，不能直接作为 PUT validator，
也不用于解析同步 payload。该行为不代表所有 WebDAV 服务都支持 Range。Range probe
不可用时，仍保留 HTTP conditional GET fallback；服务可能忽略 Range 返回 200，此时不
解析 probe body，直接回到现有完整/conditional GET 路径。

如果完整 GET 成功并通过 payload/schema/domain 校验，但 GET 与后续 PROPFIND 都无法
提供可靠 validator，客户端仍可接受不需要修改远端的纯拉取 merge。该提交会保存新的
本地业务状态、baseline、conflicts、last commit、legacy fingerprint 和 scheduler 成功
状态，同时把当前 target scoped `remoteEtag` 删除而不是保留旧值或写入占位值。由于
快照中的 `remoteEtag` 随后为 `null`，下一次同步不会使用 clean conditional fast path，
而会重新执行完整远端检查；未来重新取得可靠 validator 后才会恢复 fast path。

这个降级只适用于 clean pull-only commit。只要 merge 需要 PUT，缺少可靠 validator 仍
返回 `conditional_write_unsupported`，绝不执行无条件写入。Rust 提交边界还会在事务
前和事务内复核 outbox 不 pending、staging 为空且不存在 publish intent，避免丢弃或
确认任何待上传本地状态。

所有 unchanged shortcut 都继续检查 legacy `records.json` guard；因此目标存在 legacy
文件时仍可能下载 legacy body，并在 fingerprint 改变时报告 `legacy_remote_changed`。
unchanged 路径只更新 scheduler 成功状态和必要的 legacy fingerprint，不合并或替换
业务数据，不 ack outbox、不清理 staging/publish intent、不改 baseline、remote ETag 或
conflicts，也不创建 recovery point。窄范围 Rust 提交继续以 generation 做 TOCTOU 校验。

本轮未改变云端数据格式、资源路径、同步实体 contract、拉取周期或 Android 实现范围。
