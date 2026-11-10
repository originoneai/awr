# CLI / MCP 输出与错误契约

对照 CLI 与 MCP 的动作、回执、来源版本和错误字段。自动化按结构字段判断结果。

## Use it

基础版本 0.3.3：原有八个工作/上下文工具，加十二个会话/接续工具；stdio 共 20 个，共享 HTTP 加项目目录后为 21 个。当前工具清单见[共享 MCP 服务](mcp-service.md)。CLI 提供完整管理入口。

### 动作映射

CLI 示例统一以 `awr --project /absolute/project --json` 为前缀。表中的 `R` 是最近一次观察到的 `project_revision`。会话可由 `awr session start` 或 `awr_session_start` 创建；共享 HTTP 调用必须使用当前认证客户端绑定的会话。

共享 HTTP 每次显式传 `project`，写入带稳定 `request_id`。请求日志和领域回执都推进项目版本，继续操作使用返回的版本。上下文另带等待/回复的 `continuity` 信封，其正文、预算和 hash 与 L1 分开。

| MCP 工具 | CLI 对应动作 | 参数对应与返回值 |
| --- | --- | --- |
| `awr_project_status` | `status [--branch NAME_OR_ID] [--view action/full/summary]` | 默认 action：当前续接、可领取、等待、实际阻塞及历史汇总；旧完整结构显式传 full；[详细语义与兼容说明](daily-work.md) |
| `awr_work_ready` | `ready [--limit N] [--branch NAME_OR_ID]` | `limit` 默认 10，范围 1..100；就绪项、诊断、认领与截断提示 |
| `awr_work_get` | `work show WORK [--branch NAME_OR_ID] [--source-sha SHA]` | `work`、`branch`、`source_sha`；任务、验收、依赖、决策、证据及来源 |
| `awr_context_compile`，无 `branch` | `context compile --work WORK [--session SESSION] [--detached]` | L1、完整性、缺口、来源、预算和 Context hash |
| `awr_context_compile`，显式 `branch` | `branch context NAME_OR_ID --work WORK [--session SESSION] [--detached]` | 同一命名分支自 fork 起的增量；保持默认分支 |
| `awr_work_transition` | `work progress/block/unblock/cancel/reopen WORK --session SESSION --reason TEXT --expected-revision R` | `action` 选择动作，`next_action` / `summary` / `blocker` 对应同名连字符参数；返回提案、源写入结果和事件回执 |
| `awr_work_transition`，`action=complete` | `work complete WORK --session SESSION --reason TEXT --input /absolute/completion.json --expected-revision R [--end-session]` | MCP `completion` 是文件里的 JSON 对象；逐条绑定验收与证据，成功后释放本会话的认领。`--end-session` 对应 MCP `end_session`：同一请求内在完成落盘后结束会话（`session_end`），会话仍有其他活动认领时拒绝且保持原状态；`work complete` 与 `work cancel` 的响应在会话已无其他活动认领时带一条 `close_out` 指引（条件、依据、下一步、重评触发，至多 60 tokens） |
| `awr_event_append` | `event append --type TYPE --summary TEXT --expected-revision R` | `event_type` 对应 `--type`；可带 `--work`、`--session`、`--branch`、`--importance`、`--payload FILE`；返回完整 Event |
| `awr_evidence_record` | `evidence add --input FILE --expected-revision R` | 输入字段映射见下节；返回完整 Evidence 与 `event_id` |
| `awr_search` | `search [TEXT] [--type KIND] [--status STATUS] [--work WORK] [--limit N]` | `text` 对应位置参数，`kind` 对应 `--type`，其余同名；`work` 是 `work_item` 的别名 |

两种 Context 入口均支持 `work`、`session`、`detached`、`agent`、`goals`、`paths`、`tags`、`source_sha`、`intent`、`budget`；CLI 对应 `--work`、`--session`、`--detached`、`--agent`、可重复的 `--goal` / `--path` / `--tag`、`--source-sha`、`--intent`、`--budget`。省略的值采用同一领域默认值。

| 上下文入口 | 增量基线 |
| --- | --- |
| MCP 省略 `branch` | `checkpoint` / `after_revision` 对应 CLI 同名连字符参数，二选一 |
| MCP 显式 `branch` / CLI `branch context NAME_OR_ID` | 命名分支 fork |
| CLI `context compile --branch ID` | 普通上下文基线 |

查询 `branch` 接受名字、内部 ID 或 `main`；省略时选默认分支。MCP `null` 等同省略，主分支显式传 `"main"`。

## How it behaves

| 层 | 负责的结果 |
| --- | --- |
| CLI | 项目管理及可刷新投影的读取 |
| MCP | 来源验证后的只读查询、显式写入 |
| 回执 | 真实版本、来源与恢复入口 |

### 记录输入与回执

`awr_evidence_record` 与 CLI EvidenceDraft 的字段如下：

| MCP 参数 | CLI 输入 JSON 字段 |
| --- | --- |
| `expected_revision` | 命令行 `--expected-revision`，不写进输入 JSON |
| `work` | `work_item_key` |
| `branch` | `branch_id`，传内部 ID；显式 `null` 表示主分支，省略表示当前分支 |
| `external_key`, `evidence_type`, `level`, `summary`, `locator`, `sha256`, `source_sha`, `command`, `scope`, `verified_at` | 同名 |

两端写入返回 `ok`、`project_revision`、完整 `evidence`、`event_id`、`validation_basis`、`freshness_basis`、`source_refresh_performed`。`evidence_type`、`summary`、`command`、`scope` 保存调用者完整记录。

早期 `evidence add` 的摘要字段 `type` 已由完整写入回执替代。`evidence show` 保留含 `type`、`scope_total` 的摘要视图。

`event append` 的 `--payload FILE` 是 JSON 值文件，省略时为 `{}`。MCP 直接传 `payload` JSON 值。`importance` 默认 `normal`。成功回执含 `ok`、`project_revision`、完整 `event` 和刷新元数据。事件 ID、时间由每次实际执行产生；各次独立写入产生各自 ID。

Evidence 输入和 Event payload 的相对路径以 `--project` 根目录为基准。Completion 输入的相对路径以进程当前目录为基准，自动化建议始终使用绝对路径。

### 版本、来源与完整性

同一项目、分支、已索引来源与参数产生相同领域状态、标识、版本、验收、依赖、诊断及缺口。`active_claims` 含 `id`、`agent_id`、`session_id`、`expires_at`。

认领的生命周期在两个入口一致：请求省略 `--ttl-ms` / `ttl_ms` 时采用项目策略的默认值（`.awr/policy.toml` 或 `AWR_DEFAULT_CLAIM_TTL`，见 [claim policy](claim-policy.md)），`--no-expiry` / `no_expiry` 明确要求无到期，二者同时给出是用法错误。检查点保存与 `work progress|block|unblock`（MCP `awr_work_transition`）按同一策略续约仍有效的 claim。

读取策略有意保留以下差异：

| 字段 / 行为 | CLI status / ready / work show / search / L1 | MCP 的 5 个读工具 |
| --- | --- | --- |
| `freshness_basis` | `source_refresh` | `source_verified_readonly` |
| `source_refresh_performed` | `true` | `false` |
| `read_only` | `false`，可能更新可重建的 DB 投影 | `true`，所有持久表保持原样 |
| 来源已改变 | 先刷新，返回新版本或显式缺口 | 返回 `SourceStale`；显式运行 `awr source reindex` 或 `awr_source_reindex` 后再读 |

MCP 在内存快照验证来源，返回前再次核对来源和真实 DB revision。通用查询给出 `source_issues`、`source_warnings`；L1 将来源与必需缺口保留在正文和 `completeness`。

对照读取先同步来源，再比较同一 `project_revision`；传输策略差异限于上表三个字段。CLI 刷新失败可同时给出带 `source_issues` 的诊断正文与错误。

Event / Evidence 写入在来源刷新前后都核对 `expected_revision`。来源变化推进版本后，调用者读取当前状态再写。状态变更复用领域提案与源指纹校验。

L1 JSON 含 `ok`、`project_revision`。必需缺口返回 `ok=false`、`ContextIncomplete`，保留 `completeness`、诊断与可用正文。硬事实超预算返回 `BudgetExceeded`、`details.required`、`details.budget`。

#### Creation recovery and CLI context budgets

**Error-code change:** accepting a work-creation preview after only the project
revision advances now returns `RevisionConflict`, rather than `SourceStale`.
`details.current_revision` and `details.retry.expected_revision` supply the retry revision and
`details.preview_still_valid=true` confirms that the reviewed source facts still
match. The CLI adds `details.next_action` with the same acceptance request and the
new revision. Commands that would exceed 300 bytes or contain control characters
use a private literal JSON input file with the same reviewed request instead of
discarding its fields. Changed source content or mappings still require a fresh preview:
`SourceStale` carries `preview_still_valid=false`. An incorrect preview fingerprint
without a source change remains `SourceConflict`.

CLI `context compile` starts with 5000 tokens when `--budget` is omitted. If required
facts need more, it raises the budget to that requirement up to 10000 tokens.
The JSON response adds `budget: {applied: N, basis: "raised_to_required"}`;
other bases are `default` and `explicit`. Explicit budgets remain strict and
`BudgetExceeded` includes a command retaining the selected work, session and scope.
Long requests use CLI `context compile --input FILE --budget N`; the file contains
the full context request, and its budget is explicit. Input and selection flags
are mutually exclusive.
The rendered context and its hash exclude this CLI metadata. MCP defaults and
request shapes remain unchanged; parity comparisons use explicit matching budgets
and check the CLI metadata separately.

Daily CLI errors carry recovery guidance in `details.next_action`; text errors end
with `Next: ...`. Claim conflicts expose `holding_session`, `expires_at` (null for
no expiry), and the original claim records. Draft claim refusals suggest
`work activate`; unsupported operations suggest capability negotiation. JSON shape
errors on `batch change` and `host preview` identify `details.missing` and include a
JSON skeleton in `details.expected` (at most 600 bytes).

Draft activation refusals retain their error codes and list missing planning fields.
Their `next_action` previews a `work edit` with caller-supplied `AWR_*` values;
the caller accepts that repair before activating. Missing acceptance, next steps
and valid goal links are not replaced with invented defaults. Text refusals include
the same repair command in their final `Next:` line.

### 输出、错误与退出码

普通输出使用有界摘要；状态页最多一个当前建议，ready 默认最多 10 项。事件追加显示 ID、类型、短摘要、版本。`event show --full`、`object show --full` 和报告正文是显式展开。L1 文本保留预算内完整验收与硬规则。

`--json` 成功正文是 stdout 上的一个 JSON 对象；失败可保留部分诊断正文。stderr 独立输出 typed error JSON，分别解析两个流。

```json
{
  "code": "RevisionConflict",
  "message": "revision conflict: expected 10, actual 11",
  "details": {"expected": 10, "actual": 11}
}
```

上例展示错误形状。机器判断读取 `code`、`details`，人类解释读取 `message`；附加结构为空时省略 `details`。

| 情况 | CLI | MCP |
| --- | --- | --- |
| 成功 | exit 0；JSON 正文在 stdout | `isError=false` / 省略；`structuredContent` 与首个 text content 的 JSON 完全相同 |
| 领域错误 | exit 1；typed error 在 stderr | `isError=true`；相同 typed error 在 `structuredContent` 和 text content |
| 有诊断 / 持久提案的部分失败 | exit 1；正文在 stdout，typed error 在 stderr | `isError=true`；正文保留 `error`、缺口或提案回执 |
| CLI 缺参数、参数值错误、未知参数、未知嵌套子命令 | exit 2；`--json` 时 `InvalidInput` | 工具参数不符合 schema 时 `InvalidInput`、`isError=true` |
| 未实现的 CLI 顶层命令 | exit 1、`Unsupported` | 未知工具名是 JSON-RPC `-32601`（method not found），无领域结果 |
| `--help`、`--version` | exit 0；即使带 `--json` 也输出普通帮助 / 版本文本 | 使用 initialize / tools list 协议能力 |

`--json` 可放已知子命令前后；顶层命令诊断需要将它放在命令前。

#### Remote Team reads

The native [Team CLI reads](team-cli.md) use the authenticated Team HTTP query
contract shared with `awr_team_query`. They preserve its original JSON shapes
and remote error fields, including unknown codes and errors with no `message`.
They do not add Personal `ok` or completeness wrappers to Team results.

A valid HTTP 200 prepare/snapshot with `data.context_complete=false` exits 0:
the read succeeded, while context is incomplete. JSON stderr stays empty and
the flag, reasons and guidance remain in stdout. Consumers requiring complete
context must check that flag. This is distinct from the Personal prepare
partial-failure convention above. A Team HTTP error such as `ContextIncomplete`
exits 1 with the error on stderr and no result on stdout. Successful reads do not
authorize execution. Usage errors still exit 2, and Personal error reports and
exit behavior remain unchanged.

输入 JSON 语法或字段错误返回 `InvalidInput`。多个错误同时存在时，修正首个错误后继续处理。

核心错误保留 `NotFound`、`SourceUnavailable`、`SourceStale`、`SourceConflict`、`RevisionConflict`、`DependencyBlocked`、`ClaimConflict`、`RuleViolation`、`EvidenceMissing`、`MutationUnsupported`、`MutationConflict`、`WorkspaceConflict`、`WorkspaceContended`、`ContextIncomplete`、`BudgetExceeded`、`InvalidTransition`。`InvalidInput`、`Unsupported`、`Storage`、`Io`、`Json` 表示输入、实现或底层错误。提案 / 恢复错误还可能包括 `proposal_required`、`MutationIncomplete`、`WorkActionIncomplete`、`CheckpointIncomplete`，其中附带的提案或尝试 ID 是恢复入口。

部分写入或通信中断后，先检查提案、事件、会话与当前版本，再决定下一步。

MCP 参数、CLI Evidence 输入和 Event payload 文件至多 1 MiB；Completion 映射至多 64 KiB。领域字段、预算和内容读取按各自上限逐层校验。

### 定向验证

```bash
cargo build -p awr-cli -p awr-mcp --locked
python3 crates/awr-mcp/tests/cli_parity.py \
  --awr target/debug/awr --mcp target/debug/awr-mcp
cargo test -p awr-cli --test output_cli --locked
cargo test -p awr-mcp --test stdio --locked
cargo test -p awr-mcp --test shared --locked
```

对照程序在临时 fixture 启动真实 CLI 与 MCP stdio。读取比较完整领域 JSON，逐表核对持久状态及权威来源字节；写入由相同快照独立执行，核对状态、源文件、revision、缺口、证据与认领回执。

HTTP 集成覆盖多客户端、多项目、会话、等待、重复请求、重启和待恢复结果；官方 Rust MCP SDK 验证发现与调用。两项 AWR-EVO-012 对相同事实核对结论、诊断和五层独立性。

### 五层结果语义与入口不混用（AWR-EVO-010）

本段固定五层结果语义和协商边界，沿用既有工作状态机。核对矩阵：

- `.local/awr-evolution-20260919/semantic-contract-matrix.json`
- `tests/fixtures/evolution/AWR-EVO-010/`
- `scripts/evolution/verify_evo_010_semantic_contract.py`

#### 层与入口主键

| 层 id | CLI / MCP 主要入口 | 可读成功不意味 |
| --- | --- | --- |
| `work_readiness` | `status` / `ready` / `awr_project_status` / `awr_work_ready` | 执行准入、上下文完整、投递、完成 |
| `execution_admission` | `session start`、claim、`work transition`、`awr_work_transition` | 依赖已完成、投递成功、验收通过 |
| `context_completeness` | `context compile` / `awr_context_compile` / prepare | 依赖完成、可写执行权、模型已消费 |
| `delivery_observation` | client progress/hook、execution report、ack | 完成有效性、业务验收 |
| `completion_validity` | `work complete`、evidence、ordinary confirm | 仅因就绪队列或投递回执而通过 |

可表达但禁止混用的组合：

- 上下文完整且依赖未完成（`CX-SEP-01`）
- 可读但无执行权（`CX-SEP-02`；对照 `read_only` / 无 claim / MCP 只读）

AWR-EVO-012：CLI 与 MCP 的 `ready` / `work` 简报由同一组函数生成（`awr_core::ready_brief`、`work_brief`）；
`cli_parity.py` 的 `test_the_layers_stay_apart_and_both_entries_agree_on_every_set_of_facts` 对多组事实（依赖未完成、
阻塞、未知状态、缺失依赖、无验收、已认领）逐项核对两端结论与诊断一致且各层答案互不推出；
`test_a_query_authorizes_nothing_and_a_claim_is_judged_on_the_facts_of_that_moment` 核对查询不授权、事实变化或租约
结束后提交重新判定。诊断含义见 [diagnostics](diagnostics.md#readiness-diagnostics-which-layer-answers)。

#### 宿主模式

- `runtime_delegated`：唯一写入所有者为 `awr_runtime`。
- `component_only`：唯一写入所有者为 `embedding_host`。

#### 兼容

旧输出保留。新字段/视图通过能力或协议协商；缺必要能力返回 `CapabilityUnavailable` / `ProtocolUnsupported`（`CX-COMPAT-01`）。

```sh
python3 scripts/evolution/verify_evo_010_semantic_contract.py
```

## Limits

- CLI/stdio 对照采用关闭请求日志的基线；开启日志后比较领域事实与实际返回版本。
- 命名分支固定使用 fork 基线；普通上下文的两个自定义基线仅适用于对应普通入口。
- 查询的分支选择只影响本次读取，保留默认分支、会话、认领和 Git checkout。
- 手工证据 `trust=reported`，`validation_basis=caller_supplied_bindings`；登记保存绑定，命令执行与业务验收另行取证。
- 读取摘要与写入回执各有用途；登记结果以写入回执为准。
- 保留领域事件由对应状态变更服务产生。通用 Event payload 和 Evidence command 只作记录数据。
- `revision`、`source_revision`、`project_revision` 分别标记对象、来源与项目，按各自作用域使用。
- MCP 返回真实持久状态的版本；CLI 刷新只更新可重建投影，保留权威源字节。
- 对照保留 revision、Context hash、硬事实与缺口。MCP 遇陈旧快照仅返回错误，旧结果退出可执行路径。
- 已过期请求在刷新前拒绝；刷新后版本变化也要求重新确认，拒绝沿用旧版本登记。
- 预算不足时完整硬事实进入错误诊断；完整性声明以实际保留事实为准。
- 事件默认摘要省略 payload，完整正文通过显式展开读取。
- stdout 正文与 stderr 错误保持分流；合并输出会破坏单对象 JSON 的解析边界。
- 参数解析器的解释文字可变；自动化依赖结构字段而非逐字消息。
- 两端参数解析和文件读取顺序各自生效；多重故障的报错先后顺序属于实现细节。
- `WorkspaceConflict` 表示同一跟踪文件两端冲突：本地字节保持原样，交给操作者调解。
- `WorkspaceContended` 表示 publish/drop 连续三次提交被抢先：路径保持原样，重跑同一命令即可。
- 进程失败先检查实际写入结果再重试；项目绑定失败或协议损坏是启动/协议错误，领域 CallToolResult 此时缺席。
- 对照保留原始 Context hash 与必需事实；独立写入生成的 ID 和时间分别验证，不作为相等条件。
- 这些是本地接口与功能检查；真实 Agent 客户端业务验收、性能与发布验证分别留证。
- 各层结果分别判断；一层成功只回答该层问题。
- `CX-MIX-01` 检查同一入口保持五层结果独立，防止从一个成功推导其他成功。
- 能力不足时显式拒绝；unknown 保留未知含义，成功必须有相应事实。
- 命名分支读取保留默认选择；完成并结束会话要求其他活动认领已释放，拒绝时原状态保持。
- 独立写入保留各自生成的 ID 与时间；重放身份按对应请求日志判断。
- MCP 查询将快照变化留在内存；全部持久表保持原样。
- `--` 后的 --json 及 --field=--json 是字面值，保持当前输出模式。
- 外层与领域层上限同时生效，取适用于本次输入的全部约束。
- CLI 对照范围为既有十一项；全部新增工具逐项对照需另行扩充，查询授权仍由独立反例验证。
- component_only 由 embedding_host 持有唯一工作状态；AWR 创建宿主 Work/Run/owner 或第二工作状态属于模式范围之外。

## Code and tests

- [CLI/MCP parity](../../../crates/awr-mcp/tests/cli_parity.py)
- [CLI output](../../../crates/awr-cli/tests/output_cli.rs)
- [Stdio and HTTP](../../../crates/awr-mcp/tests/stdio.rs)
