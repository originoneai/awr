# 产物导入与读取边界

`artifact add` 将正文复制到 `.awr/artifacts/<id>`，SQLite 保存来源事件、相对路径、大小、MIME 和 SHA256。默认 Context 与搜索使用摘要和引用，不载入产物正文。

| 操作 | 默认预算 | 硬上限 |
| --- | --- | --- |
| `artifact add` | 64 MiB | 64 MiB |
| `artifact cat` | 64 KiB | 16 MiB |
| `evidence show --content` | 64 KiB | 16 MiB |

`--max-bytes` 可以收紧预算，不能提高硬上限。零值或超过硬上限的参数会被拒绝，即使文件很小。边界值本身允许；超过预算一个字节即失败。元数据登记仍可引用更大的外部产物；这不会自动复制或开放其正文读取。

导入先检查文件大小，再读取完整且受预算限制的快照，核对大小与修改时间，执行[秘密策略](secret-boundaries.md)，最后将同一份字节写入受管文件。扫描前不创建受管文件；只有复制成功并登记元数据后才保留它。读取在返回任何正文之前核对预算、实际文件大小、登记的 SHA256 与秘密策略；没有登记摘要的证据报告只能证明读取成功，不能宣称完成摘要验证。

输入和报告路径必须位于项目或明确授权的 root 内。解析授权路径后通过不跟随替换符号链接的文件句柄打开。受管的 `.awr`、`artifacts` 必须是真实目录；临时文件通过持有的目录句柄独占创建，失败清理也使用该句柄。目录路径后来被重定向时，清理不会跟随新链接删除其他位置的文件。此规则不限制配置中明确授权的输入路径别名。

JSON 正文输出要求 UTF-8；二进制产物使用不带 `--json` 的 `artifact cat`。元数据读取不会打开正文。

大小、摘要、受管文件处理与秘密数据条件由 SEC-002 的当前组件合同统一验证；这些检查不计为 E4 或正式发布。

## Bounded Team artifact reads

Team `artifact.content` retains its exact, all-or-error full-read behavior. For
clients that truncate long tool results, use `artifact.segment` with the same
`work_id` and exactly one `artifact_id` or approved `export_id`, plus the required
`expected_sha256` from the authorized reference. Large review artifacts expose
an additive `segment_query`; existing `query` references remain valid.

Each page returns base64 bytes, a half-open byte range, the segment digest, the
full artifact digest and length, and `next_query`. The default raw page is 2 KiB;
`max_context_bytes` can select 1–4096 raw bytes. The actual MCP result, including
its text and structured copies, is limited to 18,432 serialized bytes. An
oversized envelope fails without emitting content; repeat the read with a smaller
page budget and the same cursor. This bound does not describe every client's
formatting overhead or guarantee that every client retains a whole response.

Follow all `next_query` values, decode and concatenate contiguous ranges starting
at byte zero, then verify the full length and SHA-256. Decode UTF-8 only after
reconstruction: a byte boundary can split a character. `end_of_artifact` means
this range reaches the end, not that earlier pages were consumed or reviewed.
Empty artifacts have the range `[0, 0)` and no continuation.

Every page reauthenticates and rechecks the current work scope and complete
persisted bytes. Cursors bind the member/client, permission version, source,
contract, ownership, selector and digest. Export pages additionally recheck the
current disclosure, approval and adoption state. Changed bindings require a new
read from the current authorized reference. Reading or replaying pages never
claims work, adopts a dependency, approves a result or grants execution.
