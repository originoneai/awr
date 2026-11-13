# 秘密数据边界

秘密策略检查来源、运行记录和产物，返回分类与修复位置，帮助维护者将公开项目事实与凭据分开。

## Use it

```sh
python3 tests/security/payloads/verify_secrets.py --report .local/secret-boundary-checks.json
```

The current `awr-core` secret policy is 11. Component contract `tests/security/payloads/contract.json` 1.12.0 covers 32 conditions: boolean/null literals, integer-zero counts, glosses, public structures/commands, authorization narratives, Bearer prose and smuggled-secret checks. [Content-bound review](content-review.md) can authorize suspected public local-file content.

## How it behaves

| 检查对象 | 结果 |
| --- | --- |
| 原文和解析结构 | 同一秘密分类合同 |
| 公开定义和讨论 | 按明确语法与边界保留 |
| 选中的上下文与搜索结果 | 输出前再次检查 |

### Policy checks

写入前检查原文和解析结构：来源及直接解析、Manifest、投影及配置、提案 patch、事件、checkpoint digest/列表、证据和产物元数据。拒绝返回 `RuleViolation`、`details.policy_version`、`details.category` 和分类 `next_action`/`repair`。真实凭据与环境转储存于注册来源之外的 env/secret 管理，以 `${VAR}` 引用；`labelled_value` 同时提示秘密安置路径和公开 schema 写法。来源问题保留分类，CLI/MCP 共用合同。

An intake scan (`awr init`, `--write-draft`, `awr takeover`) that finds credentials in several documents reports all of them in one `RuleViolation`: `details.findings` lists the category and location of each file (50 at most, never the content) and `details.finding_total` counts them, so the owner deals with every file in one round. The first finding keeps the existing fields, and one finding is reported as before.

识别范围包括：

- API key、access/refresh/id token、client secret、password/passwd/pwd、authorization，以及中文密码、令牌、密钥等标签后的值；支持常见前缀变量名。
- 显式 `private_prompt` 字段、私有提示词标签和独立的私有 prompt 标题块。
- `.env` 风格的独立大写变量赋值、`export` 赋值、明确标为 env/environment 的对象、列表或多行块。普通的 `environment: candidate` 业务标签可以使用。
- 具有足够长度的常见 OpenAI、GitHub、Slack 凭据前缀，AWS access key、JWT、Bearer/Basic 凭据、PEM 私钥头，以及含用户名和密码的 URL。

有来源位置时返回 `details.location`（文件或 Git locator、原文行列）、`rule`、`repair`。规范化字符位置映射回原文件。

Basic 候选依据 [RFC 7617 第 2 节](https://www.rfc-editor.org/rfc/rfc7617#section-2) 按标准 Base64 解码，要求用户名与密码之间有冒号，接受省略填充符。

Bearer 普通协议讨论如 `Bearer authentication` 可保留。`Authorization:` 赋值或 Header 实际值按标签规则拒绝。无标签候选至少 32 字符，或至少 8 字符且含数字、编码分隔符或非首字母大小写混合。

结构定义可保留 `{"token":{"type":"string"}}`、`{"authorization":{"type":"http","scheme":"bearer"}}` 及同类 YAML 块/流式和 Markdown 示例。检测完整结构，支持基础类型、属性、引用、约束和认证方案；未知字段保守处理。片段解析上限 16 KiB，超限拒绝。占位值使用明确脱敏形式。

检测折叠全角 ASCII，移除常见零宽/双向格式字符，检查 JSON Unicode 转义及解码后的 JSON/YAML/TOML 字段。

完整 TypeScript `interface`、对象 `type` 和 `class` 的纯类型字段声明可保留，例如 `interface Login { password: string; }`。支持行首 export/declare、基础 `string/number/boolean/unknown/never/undefined/null`、数组、这些类型的联合、可选及 readonly 属性。复杂公开定义用结构化 schema。

普通安全讨论、空值及 `[redacted]`、`<redacted>`、`[withheld]`、`***`、`${EXAMPLE_API_KEY}` 等明确占位符可保留。实际值移到秘密管理，来源保留必要引用。

| 字面量与边界 | 规则 |
| --- | --- |
| `true` / `false`、`null` / `~`、`{}` / `[]` | 接受完整公开字面量；YAML 大小写形式同样支持 |
| 文本、结构字段、Markdown 解码 | 使用同一规则 |
| 文本结束边界 | 闭合括号、完整代码分隔、空白、逗号、分号、中文句读；英文句末点后接空白或结束 |
| `falsehood`、`null.value`、闭合后仍连值 | 拒绝 |
| 普通单/双引号字符串、Shell 布尔词拼接 | 非空实际值继续拒绝 |
| 解析后的 JSON/YAML 公开说明 | 外层字符串闭合引号可作边界 |
| 私有提示词标题正文 | 按整块检查，仅空块或明确占位符可通过 |
| 数字、其他非空值和其他凭据 | 独立检查 |
| 策略变更 | 旧来源审查凭据失效 |

Quoted YAML boundaries come from scalar tokens in the YAML scanner, including
anchors and tags. Quotes in comments, plain scalars, or literal/folded blocks do
not create string envelopes. Markdown backticks must belong to a complete code
span; a Shell command substitution after a literal remains a nonempty value.
Raw source checks, event summaries and structured payloads share these boundaries.

Complete single-line literal glosses may remain, for example `FALSE=disabled; TRUE=enabled; NULL=未设置` and `FALSE=无附件；TRUE=有附件`. The entire string may contain only the exact names `TRUE`, `FALSE`, and `NULL`, equals signs, and one short public phrase per clause, separated by semicolons. ASCII values must be one of the existing state labels `enabled`/`disabled`/`unset`/`on`/`off`/`yes`/`no`/`empty`/`missing`/`unknown`, in any case. Other values must contain 1–32 Han-script letters only, such as `未设置`, `开启`, and `关闭同库位多目的地`. Mixed scripts and other alphabets do not qualify, nor do arbitrary ASCII words or letter-only tokens such as `hunter`, `admin`, or alternating-case credentials. Han phrases containing Chinese secret labels such as `秘密`, `密码`, `密碼`, `口令`, `令牌`, `密钥`, `密鑰`, or `私有` do not qualify. Arbitrary strings, paths, numbers, quotes, script tokens, newlines, tabs, extra variables, and incomplete separated clauses remain outside this exemption. Ordinary spaces around names and separators, one leading semicolon, and fullwidth forms after punctuation normalization are allowed. JSON-decoded notes use the same rule and do not depend on field names. Real `export` statements, explicit environment objects/lists/blocks, sensitive labels, and recognizable credentials remain independently checked; a public gloss cannot hide sensitive content in other fields or entries of the same input. Longer gloss summaries must still pass independently, falling back to a safe ellipsis when needed.

公开配置说明如 `PROJECT_ROOT=/public/project EXPECTED_ITEMS=42 cargo test --offline` 和叙述/注释中的 `EXPECTED_ITEMS=42` 可保留。识别简单无引号变量前缀加已知命令词。独立赋值、环境转储、export、显式 environment 对象继续拒绝。

For `awr execution run`, set required variables in the process environment **before
invoking AWR**. The supervisor and managed child inherit that environment. For
example, in a POSIX shell:

```sh
PATH="/public/toolchain/bin:$PATH" awr execution run --session <session-id> \
  --key public-build --purpose "Build public report" -- /public/toolchain/bin/build-report
```

The shell supplies this environment to AWR; the assignment is absent from the
recorded command vector. Passing `env` followed by a standalone `PATH=...` argument
after `--` remains rejected as `environment_dump`. Use the inherited process
environment for this case; do not move environment assignments into recorded shell
command strings as a workaround. Detection and execution-intent/event validation
are unchanged. Environment contents are not captured in the immutable command
intent or operation key, so changing the environment does not make the same key
dispatch again; use a new operation key for a new run.

授权叙述如 `Completed native authorization: the user confirmed this operation.` 可保留：标签前为普通文字，后为多词说明或有标点的中文句子。公开配置用普通结构字段，授权过程写入 summary 等叙述字段。独立 authorization 字段、Header、不透明单值及 Bearer/Basic 实际值继续拒绝。

产物导入先读取最多 64 MiB 的完整内存快照，执行秘密检查，再将相同字节写入受管文件并计算摘要。显式产物/证据正文读取上限 16 MiB，完成大小、摘要和秘密检查后返回；JSON 正文同时检查解码内容。

对旧数据的输出保护：

- FTS policy is 13. On first read it rebuilds old caches, redacting ASCII token-like glosses previously accepted by policy 11 and preserving integer-zero counts under credential-like labels, short Han-letter boolean glosses, exact literal glosses, boolean/null notes, ordinary auth notes, public configuration, and structure definitions that earlier policies redacted by mistake. Failed source reindexes reread the original content; source-map and adapter-config changes also invalidate the cache. Summaries are shortened only after the full field passes; sensitive summaries become `[redacted]`. When identity, linked work, or source references contain sensitive values, the whole search document stays out of the index. Query parameters are checked as well.
- L0、L1、硬规则/关联事实及 delta 检查实际选中的输出。选中的必需事实包含敏感值时返回 `ContextIncomplete`，不返回看似完整的包、哈希或渲染文本。原本不进入 Context 的正文仍然被排除；例如 delta 只使用 checkpoint 的基线和身份，不返回 digest。
- CLI 的敏感参数在参数解析报错前拒绝，包括 `--` 之后的参数。拒绝仍以退出码 1 和 stderr JSON 返回原有 `RuleViolation` 与通用消息；`details` 保留共享分类器的 `policy_version`、`category`、`next_action`，并给出 `argument_index` 与 `argument_index_basis: zero_based_process_argv`。索引从 0 开始，0 表示可执行文件参数；不回显命中的参数、键、值或片段。环境转储分类的指引说明通过启动 AWR 前的进程环境传递变量。证据、完成输入、分支关闭输入及 Manifest 的结构错误不回显字段内容。CLI 和 MCP 共用安全错误报告；MCP 同时保护结构化结果、文本副本与读取输出。

[workspace](workspace-exchange.md) 按原始字节交换被跟踪文件。其凭据由 `awr workspace credential set` 写入 `<项目根>/.awr/workspace-credentials.json`。Unix 要求 mode 600；Windows 用 temp+rename 和 `.awr/` NTFS ACL。status 只报字段存在性与权限，配置中的密钥字段和敏感 argv 拒绝。

验证入口：

```sh
cargo build -p awr-cli -p awr-mcp --locked
python3 tests/security/payloads/verify_secrets.py --report .local/secret-boundary-checks.json
```

负面用例在隔离临时项目使用合成值。识别器、存储/来源/上下文、CLI/MCP 传输分别留证。`tests/security/payloads/verify_all.py` 运行全部 32 条件并保存本地报告。

## Limits

- Recognizable credentials remain nonreviewable. Project maintainers retain responsibility for original source content.
- 秘密策略检查输入，原始来源的清理和删除由项目维护者负责。
- 诊断仅含分类与修复位置，命中值、键和片段保留在原始输入中。
- 来源读取失败保留旧投影并标明陈旧；拒绝运行态写入时，记录与事件一并保持原样。
- 位置供本地查找，正文留在本地；结构解析后才发现的问题或无效 UTF-8 使用可确认的位置，行列可缺省。
- Basic 识别覆盖短用户名/密码；普通 basic source-intake/basic authentication 讨论可保留，敏感标签仍单独检查。
- Authorization 标签检查独立于值的长度和复杂度；普通英文单词跟随 Bearer 可作讨论，任意无标签秘密仍需维护者审查。
- 单个 type 字段只声明其本身；default/example/examples/const/enum 实际值、嵌套凭据和额外 value 均独立接受秘密检查。
- 覆盖范围限于已列识别规则；任意私有文字、编码、加密、压缩及所有 Unicode 同形字符需要额外审查。
- test、dummy、example 标签与普通输入执行相同检查。
- TypeScript 例外限完整纯类型字段；初始化器、字面量和任意类型表达式继续检查，裸 YAML、对象赋值与额外凭据同样检查。
- 该识别器只处理列出的声明形态；完整 TypeScript 编译及文件级豁免属于范围之外。
- 密码保护、token 预算和 API key 管理的普通讨论按完整语境判断，单个主题词本身可保留。
- 字面量例外要求完整语义边界；首个布尔/空值词只代表该词，其他内容继续检查。
- 字面量规则按内容适用，独立于来源文件名、项目名和特定字段名。
- 命令说明是文本；项目名和变量名本身只作内容，敏感键及可识别凭据在命令前缀中仍独立检查。
- 疑似内容保守拒绝；检查始终开启，例外限明确规则，任意字符串白名单及任意私有文字自动分类属于范围之外。
- 完整快照检查覆盖跨块内容；元数据登记仅核对元数据，外部文件正文需要显式读取验证。
- FTS 重建仅处理派生索引；权威数据、不可变事件、SQLite 空闲页和备份继续保留。磁盘擦除和凭据轮换另行执行。
- Workspace 原始字节传输在秘密策略覆盖之外；跟踪来源须由操作者排除凭据、私有 prompt 和环境转储。
- Workspace 凭据文件优先保护秘密值；Windows 权限依赖 NTFS ACL，Unix 读取强制 mode 600。
- 合成秘密留在隔离测试项目；真实客户端场景和发布验收采用独立证据。

## Code and tests

- [Secret classifier](../../../crates/awr-core/src/secrets.rs)
- [Component security contract](../../../tests/security/payloads/contract.json)

## Project source admission modes

The default is `strict`: detected sensitive content stops a source read with
its location and repair guidance. A project can opt into `warn` before its
first initialization, or switch an existing project:

```sh
awr policy secrets warn --actor project-owner
awr policy secrets show
awr init --goal "Maintain the project" --accept
awr policy secrets log
awr policy secrets strict --actor project-owner
```

Warn mode reads flagged project sources in memory and automatically redacts
sensitive projected fields before storing or returning them. It leaves the
original files unchanged. It may withhold an entire field or source-content
response when a safe smaller boundary cannot be established; partial source
reads cannot bypass that protection. Redacted output is not the original
content, even though its source reference retains the original byte fingerprint.
Initialization previews expose warnings; accepted refreshes retain queryable
`secrets.warned` receipts with file position, category and policy provenance,
never the matched value. Receipts describe detector findings, not a semantic
review or proof that a document contains no other sensitive information.

Request arguments, configuration, report uploads and AWR-authored writes remain
strict. Editing a flagged source through AWR requires cleaning it first, because
mutation recovery must not archive its original sensitive bytes. Use explicit
placeholders in authoritative ledgers when those fields must remain actionable.
An unreadable or unsupported policy falls back to strict for that project; use
`awr policy secrets show` to inspect the policy error. Returning to strict makes
the next refresh reject still-flagged sources again. Mode and pending warnings
are scoped to each synchronous project operation, including shared MCP workers.
