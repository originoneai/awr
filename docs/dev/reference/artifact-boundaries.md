# 产物导入与读取边界

导入产物并按大小、摘要与秘密策略校验后读取正文。元数据保留来源、MIME、大小和 SHA256。

## Use it

`artifact add` 将正文复制到 `.awr/artifacts/<id>`，SQLite 保存来源事件、相对路径、大小、MIME 和 SHA256。

### Import and read

```sh
awr artifact add --help
awr artifact cat --help
```

| 操作 | 默认预算 | 硬上限 |
| --- | --- | --- |
| `artifact add` | 64 MiB | 64 MiB |
| `artifact cat` | 64 KiB | 16 MiB |
| `evidence show --content` | 64 KiB | 16 MiB |

`--max-bytes` 收紧预算；合法值大于零且至多为硬上限。边界值允许，超过预算一字节即失败。元数据可引用更大的外部产物。

导入先核对大小，再读取完整受限快照、核对大小与修改时间、执行[秘密策略](secret-boundaries.md)，将同一份字节写入受管文件。复制与元数据登记成功后保留文件。正文返回前核对预算、实际大小、登记 SHA256 和秘密策略。

输入和报告位于项目或明确授权 root 内。解析授权路径后通过固定文件句柄打开；受管 `.awr`、`artifacts` 为真实目录。临时文件用持有目录句柄独占创建，失败清理同样使用该句柄。

JSON 正文要求 UTF-8；二进制通过省略 `--json` 的普通模式 `artifact cat` 输出。

大小、摘要、受管文件和秘密条件由 SEC-002 当前组件合同统一验证。

## How it behaves

| 阶段 | 校验结果 |
| --- | --- |
| 导入 | 完整受限快照与受管副本绑定 |
| 读取 | 预算、实际大小、摘要和秘密检查 |
| 分页 | 每页和完整产物分别校验 |

### Bounded Team artifact reads

Team `artifact.content` retains its exact, all-or-error full-read behavior. For
clients that truncate long tool results, use `artifact.segment` with the same
`work_id` and exactly one `artifact_id` or approved `export_id`, plus the required
`expected_sha256` from the authorized reference. Large review artifacts expose
an additive `segment_query`; existing `query` references remain valid.

Each page returns base64 bytes, a half-open range, segment digest, full digest/length and `next_query`. Raw pages default to 2 KiB; `max_context_bytes` selects 1–4096 bytes. The serialized MCP result, including text and structured copies, fits within 18,432 bytes. Retry oversized envelopes with a smaller budget and the same cursor.

Follow every `next_query`, decode and concatenate contiguous ranges from byte zero, then verify full length and SHA-256. Decode UTF-8 after reconstruction because page boundaries can split characters. Empty artifacts return `[0, 0)` with a terminal cursor.

Each page reauthenticates, rechecks work scope and verifies complete persisted bytes. Cursors bind member/client, permission version, source, contract, ownership, selector and digest. Export pages also check disclosure, approval and adoption. Changed bindings require a fresh authorized reference.

## Limits

- 默认 Context 和搜索只带摘要与引用；正文通过显式读取获取。
- 硬上限始终有效，即使实际文件更小。外部产物的元数据引用只登记引用，复制与正文读取另行检查。
- 受管文件在扫描成功后创建；证据报告省略登记摘要时，只能声明读取成功，摘要验证需已有摘要。
- 路径重定向保留原始文件/目录句柄边界，清理仅操作原目录；配置中明确授权的输入别名仍可使用。
- 元数据读取只读元数据；正文打开与校验属于显式内容读取。
- 组件合同是本地功能验证；E4 业务与正式发布分别验收。
- Oversized envelopes emit no content. Client formatting overhead and retention of the complete response require client-specific verification.
- `end_of_artifact` marks only the current range’s end; consumption and review of preceding pages require their own evidence.
- Page reads and replays grant no claim, dependency adoption, result approval or execution permission.

## Code and tests

- [Artifact implementation](../../../crates/awr-runtime/src/artifact.rs)
- [Security contract](../../../tests/security/payloads/contract.json)
