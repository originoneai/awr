# 秘密数据边界

当前使用 `awr-core` 的秘密策略 7。组件合同 `tests/security/payloads/contract.json` 1.8.0 保持 32 个条件，覆盖布尔和空值字面量、无凭据结构定义、公开命令说明、授权叙述、Bearer 普通文字和伪装夹带检查。[Content-bound Agent review](content-review.md) can authorize suspected public local-file content; recognizable credentials remain nonreviewable. 原始来源仍由项目维护者负责；AWR 不改写或删除包含敏感值的源文件。

写入前检查原始文本和解析后的结构。覆盖来源文件及直接解析、Manifest、直接投影与来源配置、提案 patch、所有事件、checkpoint 的 digest/列表、证据元数据和产物元数据。拒绝保留 `RuleViolation`，并提供 `details.policy_version`、`details.category` 和按类别区分的 `details.next_action`/`repair`：真实凭据和环境转储明确指示“不属于 AWR 管理，保持在注册来源之外，真实值放 env/secret 管理并用 `${VAR}` 引用”；`labelled_value` 同时给出真实秘密的安置路径与公开 schema 的结构化写法。诊断不附带命中的值、键或片段。来源索引问题保留这些分类，CLI/MCP 使用同一合同。来源读取失败会保留旧投影并报告非新鲜状态；直接运行态写入被拒绝时不提交记录和事件。

识别范围包括：

- API key、access/refresh/id token、client secret、password/passwd/pwd、authorization，以及中文密码、令牌、密钥等标签后的值；支持常见前缀变量名。
- 显式 `private_prompt` 字段、私有提示词标签和独立的私有 prompt 标题块。
- `.env` 风格的独立大写变量赋值、`export` 赋值、明确标为 env/environment 的对象、列表或多行块。普通的 `environment: candidate` 业务标签可以使用。
- 具有足够长度的常见 OpenAI、GitHub、Slack 凭据前缀，AWS access key、JWT、Bearer/Basic 凭据、PEM 私钥头，以及含用户名和密码的 URL。

有来源位置时，拒绝还返回 `details.location`（文件或 Git locator、原始文本行列）、`rule` 和 `repair`。位置只用于本地查找，不附带命中正文；规范化后的字符位置会映射回原文件。只在解析结构后发现、或字节不是有效 UTF-8 时，不猜行列。

Basic 候选值需能按标准 Base64 解码，并包含用户名和密码之间的冒号，依据 [RFC 7617 第 2 节](https://www.rfc-editor.org/rfc/rfc7617#section-2)。检测也接受省略填充符的形式，不设置会漏掉短用户名/密码的长度下限。普通的 “basic source-intake” 或 “basic authentication” 不因此被当作凭据；带有 `authorization` 等敏感标签的值仍按标签规则检查。

Bearer 的普通协议讨论（例如 `Bearer authentication`）可以保留。`Authorization:` 等赋值或 Header 中的实际值仍按标签规则拒绝，不依赖长度或复杂度；无标签的 Bearer 候选按不透明值形态识别：至少 32 个字符，或至少 8 个字符且包含数字、编码分隔符或非首字母大小写混合。单独一个普通英文单词无法可靠证明是凭据，因此不再仅因跟在 Bearer 后面就拒绝。任意无标签秘密仍不在完整识别承诺内。

结构定义可保留 `{"token":{"type":"string"}}`、`{"authorization":{"type":"http","scheme":"bearer"}}`，以及同类 YAML 块/流式定义和 Markdown 中的代码示例。检测器检查定义的完整结构；单个 `type` 字段不会使其他字段获得豁免。支持的定义包含基础类型、属性、引用、约束与认证方案字段；未知字段仍保守处理。`default`、`example`、`examples`、`const`、`enum` 中的非空实际值，嵌套凭据或额外 `value` 字段不会因处于定义中而放行。定义片段解析上限为 16 KiB，超出时保留拒绝行为。占位值仍须使用明确脱敏形式。

检测时折叠全角 ASCII，移除常见零宽/双向格式字符，并检查 JSON Unicode 转义；结构化 JSON/YAML/TOML 另检查解码后的字段。该策略没有宣称识别任意未标记私有文字、任意编码/加密或压缩内容，也没有宣称覆盖所有 Unicode 同形字符。禁止值的判断不以 `test`、`dummy` 或 `example` 字样豁免。

完整且仅含类型字段的 TypeScript 声明也可保留，例如 `interface Login { password: string; }`，以及同样形式的 `type Login = { ... }`、`class Login { ... }`。当前识别基础类型 `string/number/boolean/unknown/never/undefined/null`、数组、这些基础类型的联合、可选和 readonly 属性；声明必须完整，支持行首的 export/declare。含初始化器、字面量值或任意类型表达式的声明不在此例外内。裸 YAML `password: string`、对象赋值、额外凭据仍会被检查；复杂公开定义可使用上面的结构化 schema。该有限识别不是 TypeScript 编译器，也不提供文件级豁免。

可以保留安全主题的普通讨论、空值和明确占位符，例如 `[redacted]`、`<redacted>`、`[withheld]`、`***`、`${EXAMPLE_API_KEY}`。实际值应从来源中移除，只留下必要的引用。讨论密码保护、token 预算或 API key 管理不会因这些词本身被删除。

布尔字面量 `true` / `false`、JSON/YAML 空值 `null` / `~` 和空容器 `{}` / `[]` 不作为秘密值。文本、结构化字段和 Markdown 解码使用相同规则，也接受 YAML 的首字母大写和全大写布尔/空值形式。文本中的字面量必须完整：支持闭合括号、代码分隔符、空白、逗号、分号及中文句读边界，也支持其后为空白或结束的英文句末句号；`falsehood`、`null.value` 或闭合符之后仍连着值的写法继续拒绝。Markdown 代码分隔符可包裹字面量，普通单/双引号得到的非空字符串不会因此放行，Shell 的布尔词与引号字符串拼接也会拒绝。只有已解析的 JSON/YAML 容器中确实包裹公开说明的外层字符串闭合引号可作为包裹边界。私有提示词标题后的正文按整块检查，只有整块为空或明确占位符时可通过，不按首个布尔词或空值豁免。数字、其他非空值和同一文本中的其他凭据仍独立检查；此规则不依赖来源文件名、项目名或特定字段名。策略变更仍使旧来源审查凭据失效。

公开配置的命令说明也可以保留，例如 `PROJECT_ROOT=/public/project EXPECTED_ITEMS=42 cargo test --offline`，以及叙述或注释中的 `EXPECTED_ITEMS=42`。识别范围限于简单的无引号变量前缀加可识别命令词；不执行命令，不按项目名或某个变量名豁免。独立赋值、只有若干赋值的环境转储、`export` 和明确的 environment 对象继续拒绝。所有位置的敏感键赋值和可识别凭据仍独立检查；把实际密钥放在命令前缀中不能使其通过。

`Completed native authorization: the user confirmed this operation.` 这类叙述也可保留：标签前有普通文字，后面是多词文字或带明确标点的中文句子。独立的 authorization 字段、HTTP Header、不透明单值，以及跟随 Bearer/Basic 的值继续拒绝。不确定内容保留拒绝，不提供全局关闭检查或任意字符串白名单。需要明确表达公开配置时可以使用普通结构字段，授权过程可以记在 summary 等叙述字段；这不是对任意未标记私有文字的自动分类承诺。

产物在创建受管文件前，读取完整且最多 64 MiB 的快照，完成检查，再写入同一份字节并计算摘要。扫描不使用可能漏掉跨块内容的滑动窗口；为此使用有明确上限的内存缓冲。显式产物/证据正文读取仍受 16 MiB 限制，并在大小、摘要和秘密检查全部通过后返回。正文为有效 JSON 时也检查解码内容。元数据登记不构成对外部文件正文的验证。

对旧数据的输出保护：

- FTS 策略为 9，首次读取时重建旧缓存，包括旧策略下误删的布尔/空值说明、普通认证说明、公开配置和结构定义。失败来源重索引时重新读取原始内容；来源映射和适配器配置变更同样使缓存失效。摘要检查完整字段后才取短文本；敏感摘要标为 `[redacted]`。身份、关联工作或来源引用含敏感值时，整条搜索文档不进入索引。查询参数也经过检查。
- L0、L1、硬规则/关联事实及 delta 检查实际选中的输出。选中的必需事实包含敏感值时返回 `ContextIncomplete`，不返回看似完整的包、哈希或渲染文本。原本不进入 Context 的正文仍然被排除；例如 delta 只使用 checkpoint 的基线和身份，不返回 digest。
- CLI 的敏感参数在参数解析报错前拒绝；证据、完成输入、分支关闭输入及 Manifest 的结构错误不回显字段内容。CLI 和 MCP 共用安全错误报告；MCP 同时保护结构化结果、文本副本与读取输出。

FTS 重建仅更新派生索引。原有权威数据、不可变事件、SQLite 空闲页和备份不因此被擦除；这不是磁盘擦除或凭据轮换功能。

[workspace 交换平面](workspace-exchange.md)传输的是被跟踪文件的原始字节，不经过本策略：它照原样送出、照原样取回。因此凭据、私有 prompt 或环境转储不能放进被跟踪的源里。工作区凭据自身走独立的边界——`awr workspace credential set` 写到 `<项目根>/.awr/workspace-credentials.json`（Unix：600，非 600 拒绝读取；Windows：temp+rename 写入，依赖 `.awr/` 目录 NTFS ACL，当前不检查 mode），`status` 只报字段存在与文件权限、从不回显值，配置文件里出现密钥字段直接报错，密钥也不能作为命令行参数传入。

验证入口：

```sh
cargo build -p awr-cli -p awr-mcp --locked
python3 tests/security/payloads/verify_secrets.py --report .local/secret-boundary-checks.json
```

所有负面用例只在独立临时项目中使用合成值；不把秘密样本注入本项目的运行数据库。共用识别器单元检查、存储/来源/上下文检查和真实 CLI/MCP 传输检查分别留证。完整入口 `tests/security/payloads/verify_all.py` 运行全部 32 个条件并保存本地报告；这些检查不计真实客户端场景或发布验收。
