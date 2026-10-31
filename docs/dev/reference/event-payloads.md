# 事件载荷合同 v1

CLI `event append` 与 MCP `awr_event_append` 共用事件校验。事件用于记录过程；会话、claim、来源投影、checkpoint、提案和分支的生命周期回执由相应领域操作生成。通用追加不能伪造这些回执。

| 范围 | 限制 |
| --- | --- |
| 通用事件载荷 | 编码后的 JSON 不超过 1 MiB；必须是对象 |
| 领域事件载荷 | 编码后的 JSON 不超过 16 MiB；保留完整变化清单和 checkpoint 快照，超限时拒绝提交 |
| summary | 非空，最多 8192 UTF-8 字节 |
| event_type | 1–128 个 ASCII 字母、数字或 `.`、`_`、`-`、`:` |
| importance | `low`、`normal`、`high`、`critical` |

通用载荷只允许以下可选字段；提供时必须满足类型和限制，不能以 `null` 代替值。

| 字段 | 类型与限制 |
| --- | --- |
| status / error_code | 字符串，分别最多 128 字节 |
| tool / operation | 字符串，分别最多 256 字节 |
| command / report | 字符串，分别最多 4096 字节 |
| detail / body / stdout / stderr | 字符串，分别最多 1 MiB，仍受完整 JSON 上限约束 |
| source_id / artifact_id / checkpoint_id / evidence_id | 当前项目内已存在对象的 ULID |
| exit_code | 有符号 64 位整数 |
| duration_ms / count / attempt | 无符号 64 位整数 |
| changed_entities | 最多 256 个字符串，每个最多 512 字节；属于调用者的观察 |
| tags | 最多 32 个字符串，每个最多 128 字节 |
| metrics | 最多 32 项的对象，值为数字；名称为 1–64 个 ASCII 字母、数字或 `_`、`.`、`-` |

例如：

```json
{"status":"reviewed","count":2,"tags":["review"],"metrics":{"elapsed_ms":17.5},"body":"两处问题已记录，等待复核。"}
```

MCP 的 `tools/list` 使用同一字段定义生成 JSON Schema。JSON Schema 的 `maxLength` 统计字符；运行时执行更严格的 UTF-8 字节限制。MCP 的完整工具参数另有 1 MiB 上限，包含 summary、work、revision 等字段，因此可用载荷空间略小于 1 MiB。CLI 的载荷文件也最多 1 MiB。JSON 转义后的大小同样计入事件上限。

所有新事件在最终数据库插入前再次校验，包括领域操作在事务中生成的回执。领域回执有固定顶层字段表、值类别和必需身份字段；嵌套快照由类型化的领域操作构造。校验失败时，事务中的事件、业务数据和项目 revision 一起回滚。来源文件写回仍遵循其独立恢复协议，不能从数据库回滚推断文件也已回滚。

未知字段或错误值的诊断不会回显字段名称或内容。默认搜索和 Context 不读取事件正文；显式读取仍可取得原始正文。历史事件保持不可变，这项变更不修改既有记录。

所有允许字段也受[秘密策略](secret-boundaries.md)约束。CLI/MCP 在解析结构前拒绝可识别的敏感输入，返回固定 `RuleViolation`；普通字段和类型错误返回不回显内容的 `InvalidInput`。组件合同统一验证字段、大小及秘密数据边界；这些检查不代表真实客户端验收或发布。

项目策略要求续约时，`checkpoint.created` 与 `work.progressed`、`work.blocked`、`work.unblocked` 回执带有可选字段 `claim_renewal`（`ttl_ms`、`renewed`、`expired_claim_ids`），记录同一事务里延长或确认已过期的 claim；没有变化时不出现。字段含义见 [claim policy](claim-policy.md)。
