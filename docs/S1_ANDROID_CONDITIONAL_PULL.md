# S1 Android WebDAV Conditional Pull

S1 只优化 clean pull，不改变云端 schema、资源路径、实体 contract、冲突语义、调度周期、凭据存储或 Android UI。PC、当前 Android 与旧 Android 客户端继续使用相同的 `records-v3.json` 和 legacy `records.json` 协议，可以互操作。

## Eligibility

只有快照同时满足以下条件才进入 fast path：存在 baseline；target scoped `remoteEtag` 可规范化为安全的 strong 或 weak entity tag；outbox clean；staging 为空；没有 publish intent。任何 dirty 或不完整状态直接进入原有完整 GET、three-way merge 和条件写入流程。

## Decision tree

```text
PROPFIND Depth: 0 DAV:getetag
  ├─ safe ETag unchanged → legacy guard → narrow unchanged commit
  ├─ safe ETag changed   → full GET → merge
  └─ unavailable
       └─ GET Range: bytes=0-0
            ├─ valid 206 metadata + unchanged ETag → legacy guard → narrow unchanged
            ├─ valid 206 metadata + changed ETag   → full GET → merge
            └─ unavailable → GET If-None-Match
                 ├─ 304 → legacy guard → narrow unchanged
                 ├─ 200 + same reliable ETag → narrow unchanged
                 └─ 200 + changed/missing validator → parse and merge
```

所有 unchanged 分支共用同一 helper。它先执行 `records.json` legacy guard，再调用事务型 `record_sync_remote_unchanged`。窄提交复核 target id、target epoch、records generation 和 expected remote ETag，只更新 scheduler success 与必要的 legacy fingerprint；不会替换业务数据、修改 baseline/conflicts/remote ETag、ack outbox、清 staging/publish intent 或创建 recovery point。

## Range safety

Range 请求必须精确为 `bytes=0-0`。只有 HTTP 206、`Content-Range: bytes 0-0/<positive total>`、响应恰好 1 byte 且 ETag 可安全规范化时，结果才是有效 change detector。HTTP 200、缺失或畸形 Content-Range、零 total、正文长度不是 1、缺失或不安全 ETag 都会转入 conditional GET fallback；probe body 永不作为完整 payload 解析。

Range ETag 仅用于判断远端是否变化，不能作为 PUT `If-Match`、DAV `If`、upload validator 或 confirmed validator。Range 返回 401/403 时立即返回对应 HTTP error；网络异常也直接传播，不执行 fallback GET、PUT 或 commit。

## Validatorless pull-only

完整 GET 的 payload、schema 和 domain 校验通过，但 GET 与后续 PROPFIND 都无法提供可靠 write validator 时，只要 merge 完全不需要 PUT，就允许 pull-only commit。该提交保存合并后的本地业务状态、baseline、conflicts、last commit、legacy fingerprint 与 scheduler success，并删除 target scoped remote ETag。下一轮因此不具备 fast-path eligibility，会重新完整检查远端。

`remoteEtag = null` 仅允许 clean pull-only。TS 在事务前拒绝 dirty 状态，Rust 在事务前及事务内再次检查 outbox、staging、publish intent 和 upload requirement。需要 PUT 却缺少 validator 时返回 `conditional_write_unsupported`，不会执行无条件 PUT。现有 `If-Match`、DAV `If`、`If-None-Match: *`、最多三次 412 retry 以及无 strong PUT ETag 时的 verification GET 均保持不变。

## Automated evidence

本实现覆盖 clean eligibility、PROPFIND same/changed/unavailable、严格 Range 206 contract、Range ignored/malformed/body length/validator failures、401/403 与网络失败、conditional GET 304/200、validatorless pull-only、dirty-state bypass、narrow unchanged TOCTOU 和状态不变式、changed remote merge、legacy guard、412 retry、episode/collection 等价与 locked record/date normalization 回归。

完成代码后执行：`npm run gate:fast`、`npm run check:m0`、`npm run test:e2e`、`npm run android:build`，并在设备可用时执行 `npm run android:test` 与 `npm run android:m14-smoke`。

## Live WebDAV pending

Codex 不连接生产 WebDAV。手工验证应使用独立目录 `影视追踪-S1-Android-Test`：

1. unchanged：PROPFIND 207 后 Range 206/1 byte，执行 legacy guard，无完整 v3 GET、无 PUT。
2. changed：只安全修改测试 payload 尾部空白并保持 JSON 合法；Range ETag 改变后执行完整 GET，semantic no-op 不覆盖远端。
3. unchanged again：再次 Range 1 byte，无完整 v3 GET、无 PUT。

生产目录 `影视追踪` 不用于本轮验证。
