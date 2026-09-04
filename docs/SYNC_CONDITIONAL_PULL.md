# WebDAV Conditional Pull Fast Path（S1 shared protocol）

clean snapshot 的拉取顺序为：

```text
DAV PROPFIND records-v3.json
  ├─ getetag 与本地 remoteEtag 相同 → legacy guard → narrow unchanged
  │                                  （不下载或解析 v3 body）
  ├─ validator 改变 → 完整 GET → 三方 merge → 必要时条件 PUT
  └─ PROPFIND 405/501，或有效响应没有可用 validator
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

PROPFIND 的 fallback 仅接受明确的 405/501 capability 响应，或成功且 XML 合法但没有
可用 validator 的响应。401、403、其他 HTTP 错误、transport/body-read 错误、缺失正文
和 XML parse 错误都会直接失败，不进入 Range 或 conditional GET，也不记录同步成功。

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

Rust IPC 和底层 network 层复用同一规则，强制 PUT 恰好携带 strong `If-Match`、合法
strong/weak ETag 的 DAV `If`，或 `If-None-Match: *`；数量或值不合法会在创建 HTTP
client 或连接远端前失败。

完整 GET 正文和写 validator 必须来自同一个稳定 representation。strong GET ETag 可直接
绑定正文并用于 `If-Match`。weak 或 unquoted GET ETag 规范化后，只有读取后的 PROPFIND
返回相同 ETag 才能用 DAV `If`。两者不同会丢弃正文并重新完整 GET。GET 没有 ETag、但
PROPFIND 有 ETag 时，第一次正文同样被丢弃；客户端重新 GET，并要求新 GET 前后的两个
DAV 观察值相同。连续三次仍无法稳定会返回 `remote_busy`，过程中不 PUT、不 commit。
Range ETag 不参与这一绑定。

所有 unchanged shortcut 都继续检查 legacy `records.json` guard；因此目标存在 legacy
文件时仍可能下载 legacy body，并在 fingerprint 改变时报告 `legacy_remote_changed`。
unchanged 路径只更新 scheduler 成功状态和必要的 legacy fingerprint，不合并或替换
业务数据，不 ack outbox、不清理 staging/publish intent、不改 baseline、remote ETag 或
conflicts，也不创建 recovery point。窄范围 Rust 提交继续以 generation 做 TOCTOU 校验。

legacy guard 只接受 `records.json` 的 200（验证 fingerprint）或 404（`missing`）。401、403、
5xx 及其他 HTTP 状态均按对应 HTTP error 失败，transport/network error 直接传播；guard
失败时不会执行 unchanged commit、普通 commit、PUT 或记录同步成功。

业务等价快捷分支位于 frozen conflict 检查之后，因此未显式解决的 conflict 不会静默消失。
业务字段相同但系统字段不同不会触发 PUT：远端结果保留规范化后的远端副本；unlocked
本地结果确定性选择较新副本；locked 本地结果保留本机副本，同时远端保留远端副本。
optional date 的 `null`、`undefined`、空串和空白字符串都规范化为空字符串。实体数组顺序
不构成变化，但 collection member `position` 等业务字段仍构成变化。

Android M1.4 受控本地服务器 smoke 覆盖 PROPFIND 405 后的严格 206 Range：clean unchanged
断言有 Range、无完整 V3 GET、无 PUT；Range changed 断言随后执行完整 GET。smoke 统计
PROPFIND、Range、完整 V3 GET 和 PUT，并持续要求 `unconditionalPuts=0`。

本轮未改变云端数据格式、资源路径、同步实体 contract 或拉取周期。
