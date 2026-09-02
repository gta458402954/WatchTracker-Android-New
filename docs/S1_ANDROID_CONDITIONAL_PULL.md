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

## Full GET body and validator binding

任何 PUT validator 都必须证明它对应实际参与 merge 的完整 GET 正文：

- strong GET ETag 直接绑定正文，并使用 `If-Match`。
- weak 或 unquoted GET ETag 先安全规范化；读取后的 PROPFIND 必须返回相同 ETag，之后才使用 DAV `If`。
- GET ETag 与后置 PROPFIND 不同会丢弃该正文并重新完整 GET。
- GET 没有 ETag、后置 PROPFIND 有 ETag 时，该 ETag只是下一轮读取前观察值；客户端重新完整 GET，并再次 PROPFIND。只有前后 DAV 观察值相同才绑定新正文。
- validator 连续变化最多读取三次，之后返回 `remote_busy`。所有被丢弃的正文都不会用于 PUT 或 commit。

Range ETag 从不进入正文绑定状态机。PUT 后缺少 strong response ETag 时，verification GET 也使用同一绑定流程，并继续验证 `commitId`。

## Validatorless pull-only

完整 GET 的 payload、schema 和 domain 校验通过，但 GET 与后续 PROPFIND 都无法提供可靠 write validator 时，只要 merge 完全不需要 PUT，就允许 pull-only commit。该提交保存合并后的本地业务状态、baseline、conflicts、last commit、legacy fingerprint 与 scheduler success，并删除 target scoped remote ETag。下一轮因此不具备 fast-path eligibility，会重新完整检查远端。

`remoteEtag = null` 仅允许 clean pull-only。TS 在事务前拒绝 dirty 状态，Rust 在事务前及事务内再次检查 outbox、staging、publish intent 和 upload requirement。需要 PUT 却缺少 validator 时返回 `conditional_write_unsupported`，不会执行无条件 PUT。现有 `If-Match`、DAV `If`、`If-None-Match: *`、最多三次 412 retry 以及无 strong PUT ETag 时的 verification GET 均保持不变。

## Merge invariants

Frozen conflict 优先于业务等价快捷分支；未显式解决的 conflict 即使两端后来业务字段相同也继续保留。业务字段等价时，远端结果保留规范化后的远端记录，因此 `rev`、`revActor`、`updatedAt` 差异不会制造 PUT。unlocked 本机记录确定性选择较新副本；locked 本机记录保留本机副本，同时远端保持远端副本，不会把较旧 locked revision 发布到云端。

`startDate`、`endDate` 的 `null`、`undefined`、空串和空白字符串统一视为空日期，merge 输出统一为字符串。双方清除同一日期并修改不同字段可以自动合并；不同非空日期仍产生 conflict。实体数组顺序不属于业务变化，collection member `position` 仍属于业务变化。

## Legacy guard failure

所有 unchanged 路径先完成相同的 legacy guard。`records.json` 返回 200 时验证 ETag/fingerprint，404 记录 `missing`；401、403、5xx 和其他状态抛出对应 HTTP error，transport/network error 原样传播。guard 未完成时不会调用 `record_sync_remote_unchanged`、`commit_sync_result` 或 PUT，也不会记录成功。

## Automated evidence

本实现覆盖 clean eligibility、PROPFIND same/changed/unavailable、严格 Range 206 contract、Range ignored/malformed/body length/validator failures、401/403 与网络失败、conditional GET 304/200、validatorless pull-only、dirty-state bypass、narrow unchanged TOCTOU 和状态不变式、changed remote merge、legacy guard、412 retry、episode/collection 等价与 locked record/date normalization 回归。

Android M1.4 smoke 的受控本地 WebDAV 可以切换 PROPFIND 405，并返回严格的 `Range: bytes=0-0` 206、合法 Content-Range、单字节正文和 ETag。smoke 分别验证 clean Range unchanged 不执行完整 V3 GET/PUT，以及 Range changed 后执行完整 GET；输出包含 PROPFIND、Range、完整 V3 GET、conditional PUT 和 unconditional PUT 计数，并要求 `unconditionalPuts=0`。

完成代码后执行：`npm run gate:fast`、`npm run check:m0`、`npm run test:e2e`、`npm run android:build`，并在设备可用时执行 `npm run android:test` 与 `npm run android:m14-smoke`。

## Live WebDAV pending

Codex 不连接生产 WebDAV。手工验证应使用独立目录 `影视追踪-S1-Android-Test`：

1. unchanged：PROPFIND 207 后 Range 206/1 byte，执行 legacy guard，无完整 v3 GET、无 PUT。
2. changed：只安全修改测试 payload 尾部空白并保持 JSON 合法；Range ETag 改变后执行完整 GET，semantic no-op 不覆盖远端。
3. unchanged again：再次 Range 1 byte，无完整 v3 GET、无 PUT。

生产目录 `影视追踪` 不用于本轮验证。
