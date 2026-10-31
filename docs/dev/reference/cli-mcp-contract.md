# CLI / MCP 输出与错误契约

本页描述八个原有工作与上下文工具的 CLI/MCP 领域对照。0.3.3 新增十二个会话与接续工具；共享 HTTP 服务另有项目目录工具，总计 21 个，stdio 为 20 个。完整契约见[共享 MCP 服务](mcp-service.md)。CLI 继续提供完整项目管理入口。

## 动作映射

CLI 示例统一以 `awr --project /absolute/project --json` 为前缀。表中的 `R` 是最近一次观察到的 `project_revision`。会话可由 `awr session start` 或 `awr_session_start` 创建；共享 HTTP 调用必须使用当前认证客户端绑定的会话。

共享 HTTP 每次调用显式传 `project`，写入还需稳定 `request_id`。请求回执会额外推进项目版本，原有 CLI/stdio 对照以不启用请求日志的调用为基线。使用日志时按返回版本继续，不要求它与独立 CLI 执行后的数字相同。上下文额外返回等待/回复的 `continuity`，该信封独立于 L1 正文、预算和 hash。

| MCP 工具 | CLI 对应动作 | 参数对应与返回值 |
| --- | --- | --- |
| `awr_project_status` | `status [--branch NAME_OR_ID] [--view action/full/summary]` | 默认 action：当前续接、可领取、等待、实际阻塞及历史汇总；旧完整结构显式传 full；[详细语义与兼容说明](daily-work.md) |
| `awr_work_ready` | `ready [--limit N] [--branch NAME_OR_ID]` | `limit` 默认 10，范围 1..100；就绪项、诊断、认领与截断提示 |
| `awr_work_get` | `work show WORK [--branch NAME_OR_ID] [--source-sha SHA]` | `work`、`branch`、`source_sha`；任务、验收、依赖、决策、证据及来源 |
| `awr_context_compile`，无 `branch` | `context compile --work WORK [--session SESSION] [--detached]` | L1、完整性、缺口、来源、预算和 Context hash |
| `awr_context_compile`，显式 `branch` | `branch context NAME_OR_ID --work WORK [--session SESSION] [--detached]` | 同一命名分支自 fork 起的增量；不切换默认分支 |
| `awr_work_transition` | `work progress/block/unblock/cancel/reopen WORK --session SESSION --reason TEXT --expected-revision R` | `action` 选择动作，`next_action` / `summary` / `blocker` 对应同名连字符参数；返回提案、源写入结果和事件回执 |
| `awr_work_transition`，`action=complete` | `work complete WORK --session SESSION --reason TEXT --input /absolute/completion.json --expected-revision R [--end-session]` | MCP `completion` 是文件里的 JSON 对象；逐条绑定验收与证据，成功后释放本会话的认领。`--end-session` 对应 MCP `end_session`：同一请求内在完成落盘后结束会话（`session_end`），会话仍有其他活动认领时拒绝且不写入；`work complete` 与 `work cancel` 的响应在会话已无其他活动认领时带一条 `close_out` 指引（条件、依据、下一步、重评触发，不超过 60 token） |
| `awr_event_append` | `event append --type TYPE --summary TEXT --expected-revision R` | `event_type` 对应 `--type`；可带 `--work`、`--session`、`--branch`、`--importance`、`--payload FILE`；返回完整 Event |
| `awr_evidence_record` | `evidence add --input FILE --expected-revision R` | 输入字段映射见下节；返回完整 Evidence 与 `event_id` |
| `awr_search` | `search [TEXT] [--type KIND] [--status STATUS] [--work WORK] [--limit N]` | `text` 对应位置参数，`kind` 对应 `--type`，其余同名；`work` 是 `work_item` 的别名 |

两种 Context 入口均支持 `work`、`session`、`detached`、`agent`、`goals`、`paths`、`tags`、`source_sha`、`intent`、`budget`；CLI 对应 `--work`、`--session`、`--detached`、`--agent`、可重复的 `--goal` / `--path` / `--tag`、`--source-sha`、`--intent`、`--budget`。省略的值采用同一领域默认值。

无显式 MCP `branch` 时，`checkpoint` / `after_revision` 对应 CLI `--checkpoint` / `--after-revision`，两者互斥。显式 MCP `branch` 使用 fork 基线，不接受这两个自定义基线。CLI 单独提供的 `context compile --branch ID` 使用普通上下文基线；它与 `branch context NAME_OR_ID` 的含义不同，不用来替代上表中的命名分支映射。

查询的 `branch` 可以是名字、内部 ID 或 `main`；省略时读取项目当前默认分支。显式选择不会修改默认分支、既有会话、认领或 Git checkout。MCP 的 `null` 等同省略，选择主分支须传 `"main"`。

## 记录输入与回执

`awr_evidence_record` 与 CLI EvidenceDraft 的字段如下：

| MCP 参数 | CLI 输入 JSON 字段 |
| --- | --- |
| `expected_revision` | 命令行 `--expected-revision`，不写进输入 JSON |
| `work` | `work_item_key` |
| `branch` | `branch_id`，传内部 ID；显式 `null` 表示主分支，省略表示当前分支 |
| `external_key`, `evidence_type`, `level`, `summary`, `locator`, `sha256`, `source_sha`, `command`, `scope`, `verified_at` | 同名 |

两端成功写入都返回 `ok`、`project_revision`、`evidence`、`event_id`、`validation_basis`、`freshness_basis`、`source_refresh_performed`。`evidence.evidence_type`、`summary`、`command`、`scope` 是实际保存的完整值，不截短调用者的记录。`validation_basis=caller_supplied_bindings` 表示按调用者提交的绑定保存；记录成功不代表命令被执行或业务验收已完成。

这修正了早期 CLI `evidence add` 用摘要字段 `type` 返回写入结果的行为。读取命令 `evidence show` 继续提供原有摘要视图（含 `type`、`scope_total`）；它不是写入回执。

`event append` 的 `--payload FILE` 是 JSON 值文件，省略时为 `{}`。MCP 直接传 `payload` JSON 值。`importance` 默认 `normal`。成功回执含 `ok`、`project_revision`、完整 `event` 和刷新元数据。事件 ID、时间由每次实际执行产生；独立的两次写入不会得到同一个 ID。

通用事件不能伪造 `work.completed` 等保留的领域事件；必须走对应状态变更服务。Event payload、Evidence command 都只是记录内容，不作为命令执行。

Evidence 输入和 Event payload 的相对路径以 `--project` 根目录为基准。Completion 输入的相对路径以进程当前目录为基准，自动化建议始终使用绝对路径。

## 版本、来源与完整性

在同一项目、同一分支选择、同一已索引来源和相同参数下，两个入口返回相同的领域状态、标识、版本、验收、依赖、诊断及缺口。查询中的 `active_claims` 都包含 `id`、`agent_id`、`session_id`、`expires_at`。`revision` 是对象版本，`source_revision` 是来源版本，`project_revision` 是项目状态版本，三者不可互换。

读取策略有意保留以下差异：

| 字段 / 行为 | CLI status / ready / work show / search / L1 | MCP 的 5 个读工具 |
| --- | --- | --- |
| `freshness_basis` | `source_refresh` | `source_verified_readonly` |
| `source_refresh_performed` | `true` | `false` |
| `read_only` | `false`，可能更新可重建的 DB 投影 | `true`，不持久化任何表的变化 |
| 来源已改变 | 先刷新，返回新版本或显式缺口 | 返回 `SourceStale`；显式运行 `awr source reindex` 或 `awr_source_reindex` 后再读 |

MCP 在内存快照中验证来源，并在返回前再次检查来源和真实 DB revision。它不会把临时快照版本当作持久状态返回。CLI 刷新不会改写权威源文件。通用查询提供 `source_issues` 和 `source_warnings`；L1 将来源与所需缺口放在自己的上下文和 `completeness` 中。

因此，对照读取应先同步来源，再比较同一个 `project_revision`，仅区别表中的三个传输策略字段；不能忽略 revision、Context hash、硬事实或缺口来制造一致结果。CLI 刷新失败时可能同时返回带 `source_issues` 的诊断正文和错误；MCP 拒绝陈旧快照时只返回错误，不返回一个可执行的旧结果。

Event / Evidence 写入在刷新前后都核对 `expected_revision`。刷新前已过期的请求不执行刷新或领域写入；刷新发现来源变化而推进版本后，拒绝沿用旧版本追加记录。状态变更使用同一领域提案与源指纹校验流程。

L1 JSON 顶层包含 `ok` 和 `project_revision`；有必需缺口时为 `ok=false`，同时保留 `completeness`、诊断与可用正文，并带 `error.code=ContextIncomplete`。硬事实超预算时返回 `BudgetExceeded` 和 `details.required` / `details.budget`，不截断硬事实后宣称完整。

## 输出、错误与退出码

CLI 普通输出面向人阅读：默认列出有界摘要，状态页最多显示一个当前建议，ready 默认最多 10 项。事件追加默认显示 ID、类型、短摘要和版本；不回显 payload。`event show --full`、`object show --full` 和报告内容读取属于显式展开。L1 的默认文本输出是预算约束内的执行上下文，完整验收与硬规则保留。

`--json` 模式下，成功正文是 stdout 上的一个 JSON 对象。失败可能有可检查的部分正文；stderr 是独立的 typed error JSON，调用者必须分别解析两个流。不要用 `2>&1` 合并后再整体解析。

```json
{
  "code": "RevisionConflict",
  "message": "revision conflict: expected 10, actual 11",
  "details": {"expected": 10, "actual": 11}
}
```

上例仅展示形状。`code` 与 `details` 是机器判断依据，`message` 用于解释，不承诺不同参数解析器逐字一致。没有附加结构时省略 `details`。

| 情况 | CLI | MCP |
| --- | --- | --- |
| 成功 | exit 0；JSON 正文在 stdout | `isError=false` / 省略；`structuredContent` 与首个 text content 的 JSON 完全相同 |
| 领域错误 | exit 1；typed error 在 stderr | `isError=true`；相同 typed error 在 `structuredContent` 和 text content |
| 有诊断 / 持久提案的部分失败 | exit 1；正文在 stdout，typed error 在 stderr | `isError=true`；正文保留 `error`、缺口或提案回执 |
| CLI 缺参数、参数值错误、未知参数、未知嵌套子命令 | exit 2；`--json` 时 `InvalidInput` | 工具参数不符合 schema 时 `InvalidInput`、`isError=true` |
| 未实现的 CLI 顶层命令 | exit 1、`Unsupported` | 未知工具名是 JSON-RPC `-32601`（method not found），无领域结果 |
| `--help`、`--version` | exit 0；即使带 `--json` 也输出普通帮助 / 版本文本 | 使用 initialize / tools list 协议能力 |

`--json` 支持放在已知子命令前后。为了诊断未知顶层命令，放在命令前。`--` 之后的字面量 `--json` 和 `--field=--json` 不切换输出模式。

输入 JSON 语法或字段错误返回 `InvalidInput`，不冒充内部序列化错误。若同时存在多个错误，两端的参数解析与文件读取顺序可能不同；修正首个错误后再继续，不依赖多重故障的报错先后顺序。

`WorkspaceConflict` 只来自 `awr workspace`：两端都改过同一个被跟踪文件，工具不自动合并，本地文件保持原样。`WorkspaceContended` 是另一种情形——一次 `publish` 或 `drop` 连续三次提交都被对端抢先，此时没有任何路径被改写，补救是再执行一次同一命令，不需要调解，也不需要读消息来区分。核心错误保留 `NotFound`、`SourceUnavailable`、`SourceStale`、`SourceConflict`、`RevisionConflict`、`DependencyBlocked`、`ClaimConflict`、`RuleViolation`、`EvidenceMissing`、`MutationUnsupported`、`MutationConflict`、`WorkspaceConflict`、`WorkspaceContended`、`ContextIncomplete`、`BudgetExceeded`、`InvalidTransition`。`InvalidInput`、`Unsupported`、`Storage`、`Io`、`Json` 表示输入、实现或底层错误。提案 / 恢复错误还可能包括 `proposal_required`、`MutationIncomplete`、`WorkActionIncomplete`、`CheckpointIncomplete`，其中附带的提案或尝试 ID 是恢复入口。

发现部分写入或通信中断时先检查提案、事件、会话与当前版本，再决定下一步；不能看到进程失败就盲目重试写入。MCP 进程无法绑定项目或协议本身损坏时属于启动 / 协议错误，没有可以读取的领域 `CallToolResult`。

MCP 参数总量最多 1 MiB；CLI Evidence 输入 / Event payload 文件最多 1 MiB；Completion 映射最多 64 KiB。领域字段、预算和内容读取另有自己的上限，外层限制不放宽内层校验。

## 定向验证

```bash
cargo build -p awr-cli -p awr-mcp --locked
python3 crates/awr-mcp/tests/cli_parity.py \
  --awr target/debug/awr --mcp target/debug/awr-mcp
cargo test -p awr-cli --test output_cli --locked
cargo test -p awr-mcp --test stdio --locked
cargo test -p awr-mcp --test shared --locked
```

对照程序使用临时 fixture，实际启动 CLI 与 MCP stdio。读取比较完整领域 JSON，逐表检查 MCP 未写入 DB，检查权威来源字节不变；写入从同一快照分别执行，并核对状态、源文件结果、revision、缺口、证据和认领回执。Context hash 和必需事实不做归一化。独立写入的生成 ID / 时间分别验证，不能作为两次执行内容相等的条件。

HTTP 集成另外覆盖多客户端、多项目、会话、等待、重复请求、重启和未知结果恢复；官方 Rust MCP SDK 客户端还验证协议发现与调用。十一项 CLI 对照没有被扩充成全部新工具的逐项对照；其中两项（AWR-EVO-012）对同一组事实经两端核对结论、诊断与五层互不混用，并核对查询不授权。

这些属于本地接口与功能验证，不代表真实 Agent 客户端业务验收、性能测量或发布结果。


## 五层结果语义与入口不混用（AWR-EVO-010）

本段冻结 AWR-EVO-010 的结果层与协商边界；不新增第二套工作状态机。核对矩阵：

- `.local/awr-evolution-20260919/semantic-contract-matrix.json`
- `tests/fixtures/evolution/AWR-EVO-010/`
- `scripts/evolution/verify_evo_010_semantic_contract.py`

### 层与入口主键

| 层 id | CLI / MCP 主要入口 | 可读成功不意味 |
| --- | --- | --- |
| `work_readiness` | `status` / `ready` / `awr_project_status` / `awr_work_ready` | 执行准入、上下文完整、投递、完成 |
| `execution_admission` | `session start`、claim、`work transition`、`awr_work_transition` | 依赖已完成、投递成功、验收通过 |
| `context_completeness` | `context compile` / `awr_context_compile` / prepare | 依赖完成、可写执行权、模型已消费 |
| `delivery_observation` | client progress/hook、execution report、ack | 完成有效性、业务验收 |
| `completion_validity` | `work complete`、evidence、ordinary confirm | 仅因就绪队列或投递回执而通过 |

同一入口不得把一层成功重解释为另一层成功（反例 `CX-MIX-01`）。

可表达但禁止混用的组合：

- 上下文完整且依赖未完成（`CX-SEP-01`）
- 可读但无执行权（`CX-SEP-02`；对照 `read_only` / 无 claim / MCP 只读）

AWR-EVO-012：CLI 与 MCP 的 `ready` / `work` 简报由同一组函数生成（`awr_core::ready_brief`、`work_brief`）；
`cli_parity.py` 的 `test_the_layers_stay_apart_and_both_entries_agree_on_every_set_of_facts` 对多组事实（依赖未完成、
阻塞、未知状态、缺失依赖、无验收、已认领）逐项核对两端结论与诊断一致且各层答案互不推出；
`test_a_query_authorizes_nothing_and_a_claim_is_judged_on_the_facts_of_that_moment` 核对查询不授权、事实变化或租约
结束后提交重新判定。诊断含义见 [diagnostics](diagnostics.md#readiness-diagnostics-which-layer-answers)。

### 宿主模式

- `runtime_delegated`：唯一写入所有者为 `awr_runtime`
- `component_only`：唯一写入所有者为 `embedding_host`；**不得**创建宿主 Work/Run/owner，不得持有第二工作状态

### 兼容

旧输出保留。新字段/视图需能力或协议协商；缺必要能力返回
`CapabilityUnavailable` / `ProtocolUnsupported`，禁止静默降级（`CX-COMPAT-01`）。
unknown 不得默认为成功。

```sh
python3 scripts/evolution/verify_evo_010_semantic_contract.py
```
