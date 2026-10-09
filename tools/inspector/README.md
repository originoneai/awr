# AWR Inspector

AWR 的本地观察工具。看工作队列、挑下一件活、看清上下文包里装了什么、检查源新鲜度。

**它是什么：** 一个把 AWR 状态显示给人看的界面。
**它不是什么：** 不是 agent 客户端，不是项目管理系统，不编辑权威源文件，不推进工作状态，不替代 CLI 或 MCP。

---

## 三十秒跑起来

只要机器上有 Node 18 以上：

```bash
node server.js
```

浏览器会自动打开 <http://127.0.0.1:7381>。

**没装 `awr` 也能跑。** 这时它进入演示模式，用一份编好的样本项目数据，界面功能完全一样。
先在演示模式里把四个页面点一遍，熟悉了再连自己的项目。

连自己的项目：

```bash
node server.js --project /你的/项目路径
```

| 参数 | 作用 |
| --- | --- |
| `--project <路径>` | 要看哪个 AWR 项目，默认当前目录 |
| `--port <端口>` | 换端口，默认 7381 |
| `--demo` | 强制演示模式，不执行任何真实命令 |
| `--allow-reindex` | 允许从界面触发 `source reindex`，**默认关闭** |
| `--map-config <文件>` | 项目地图的展示配置（JSON）：主线、依赖面板、停滞阈值、里程碑泳道，见下文「项目地图」 |
| `--no-open` | 不自动打开浏览器 |
| `--team-url <URL> --team-only` | 仅提供真实 Team 入口，隐藏本地项目页面并拒绝本地文件系统 API |

Team-only 部署需在反向代理配置 HTTPS 和受控 Origin。当前网页支持登录、项目列表、任务总览与有权限的合同/运行详情；分工、交接和评审通过 MCP 完成。项目管理员还可使用成员管理与活动记录页面；普通成员仅能查看自己的活动记录。未返回的依赖/运行信息保持未知，不能据此认定任务可执行或已完成。

Team 工作区复用 AWR 官网的蓝白协作关系图：项目 → 工作线 → 任务，选择卡片查看右侧详情，也可切换为列表。图中只画当前权限内已读取的依赖；跨工作线依赖用虚线区分，缺失的依赖导出会提示不完整。任务详情每批最多读取 60 项、并发最多 4 个请求；可继续读取剩余项。窄屏关系图支持内部滚动，详情移到下方。

连接 Team 服务时，页面通过 `work.observe` 展示真实会话、检查点、认领有效期、执行记录和已登记交付。负责人、认领人和客户端分开显示；未记录的模型和 Token 用量明确标为缺失。执行记录不证明 Agent 进程此刻在线，认领过期和结果待核对会提示需要处理。回执正文仍遵循服务端权限。

可见的团队任务页每 15 秒自动刷新，保留选中的任务和布局；切到成员管理、编辑表单或隐藏页面时暂停刷新。失败时保留并标注上次读取的数据，权限失效时清除受保护内容。已登记的 GitHub PR，以及当前检查点明确提及的 PR，可以显示公开 GitHub API 查到的当前提交和检查结果（至少缓存 5 分钟，随 PR 数量延长以适应匿名额度，限流时遵守重试时间）；页面标明实际查询时间。检查点提及不会变成正式交付登记，CI 通过也不等于 AWR 验收；私有仓库、限流或网络失败不会显示为通过。

---

## 界面语言

右上角的语言选择器支持 **English / 简体中文**，切换后刷新页面并记住选择。
语言优先级为 URL 的 `?lang=en` 或 `?lang=zh-CN`、上次保存的选择、浏览器语言，最后回退到英文。
静态页面、动态提示、术语、新手引导和演示内容使用同一套本地翻译资源，不加载外部服务。
真实项目的任务标题、来源正文和原始 AWR 错误保持原文；服务端和 CLI 的开发者诊断使用英文。

## 第一次用？

界面第一次打开会自动弹出**新手引导**（5 步，1 分钟）。跳过了也没关系，右上角「新手引导」随时能再看。

三个帮你看懂界面的东西：

- **右上角「术语」** —— work item、queue、checkpoint、budget、drift、revision 这些词的大白话解释。
- **面板标题旁的 `?`** —— 点开是这一块在说什么、该怎么读。
- **每个 `$` 开头的命令框** —— 就是桥接后台真正跑的那条命令，复制到终端跑结果一样。界面不是黑盒。

---

## 四个页面

| 页面 | 回答什么 | 背后的命令 |
| --- | --- | --- |
| 概览 | 现在该干什么 | `awr status`（+ `awr ready` / `awr session list --active` / `awr event history`）|
| 工作项 | 这件活要做成什么样、卡在哪 | `awr work show` |
| 上下文 | 给 agent 的那包东西里装了什么 | `awr context compile` |
| 索引源 | 看到的东西还算数吗 | `awr intake inspect`（读 `organization.sources`）|

**先看哪里：** 概览页的「被阻塞」和「等待中」两个队列，进度停下来的地方都在那儿。

### 每页看得到什么

| 位置 | 观察字段 |
| --- | --- |
| 概览 · 状态条 | 四个队列计数、结构缺口数、组织状态、project revision |
| 上下文 · 这个包有多大 | 必需内容 / 这次装进去的 / 预算上限，都是这次编译实际返回的数。就画在编译按钮下面 |
| 概览 · 结构缺口 | `organization.gaps` 的 code / target / 说明 |
| 概览 · 待查的运行时操作 | `pending_operations`——被中断、结果未知的操作（字段缺失时整块隐藏）|
| 概览 · 活跃 session | `awr session list --active`：agent / status / work / last checkpoint（`Unsupported` 时整块隐藏）|
| 概览 · 最近事件 | `awr event history`：type / summary / importance / 时间（`Unsupported` 时整块隐藏）|
| 工作项 · 表格 | 队列、源状态、负责人、认领状态、诊断码、source revision |
| 工作项 · 详情 | 目标、验收标准、阻塞/等待、依赖与未决依赖、依赖成环、**谁占着这件活**（agent / session / 到期）、**证据与决策**、诊断码 |
| 上下文 · 完整性 | `completeness.status`、**六个维度**（规则 / 目标上下文 / 工作状态 / 验收标准 / 依赖 / 源新鲜度）、**证据缺口**、未决依赖、issues、被省略的块及原因 |
| 索引源 | 每个源的 domain / role / 新鲜度 / revision，与 project revision 并列 |

### Session 与事件

当前源码里 `awr session list --active` 和 `awr event history` 已经实现。概览页会调用它们，显示活跃 session 和最近事件摘要。发布版 0.4.0 若仍返回 `Unsupported`，这两块面板会隐藏，不会用空列表冒充「没有会话」。这里不展开 checkpoint 正文或 open loop；那些仍走 `session show`。

---

## 项目地图（节点图与依赖图）

左侧导航的「项目地图」把当前项目画成三张图：

| 图 | 画什么 |
| --- | --- |
| 项目节点图 | 工作项按主线分组，每条主线内按状态（开发中、停滞、阻塞、就绪、草稿、等待前置）排列；指向已取消依赖的缺陷、活动认领的执行者、`doctor` 的健康条一并列出 |
| 主线依赖图 | 每条主线面板里的依赖关系（传递约简后）、关键路径，已完成的工作折叠成缩略图 |
| 泳道依赖图 | 按里程碑分泳道的未完成工作及其依赖，位于已取消依赖下游的节点用红色虚线框标出 |

页面下方有全部工作项的数据表，每个卡片也有可读的文字替代；最后是「官方接口未提供的字段」清单。

### 交互：卡片、泳道和图本身都是活的

图不是一张死图。页面和下载的 HTML 用同一套交互层，所有操作也都能用键盘完成：

| 想做的事 | 怎么做 |
| --- | --- |
| 看一个工作项的详情 | 点卡片（或数据表里的一行，键盘上 Enter / 空格）。右侧滑出详情面板：状态与就绪判定、主线与里程碑、负责/认领、最近活动、依赖链规模；有阻塞、下一步、等待项、就绪诊断、有效认领、会话、健康发现时逐项列出；位于已取消依赖下游的项会说明原因；末尾列出官方接口没有提供的字段。Esc 或 × 关闭，焦点回到原卡片 |
| 顺着依赖走 | 面板里的「前置」和「后继」是按钮，点一下就切到那一项并在图里定位、闪一下。被折叠藏起的项会自动展开 |
| 看依赖链 | 鼠标悬停或键盘聚焦一张卡片，它的全部上游（紫色环）和下游（粉色环）、连线加粗，其余淡出；点击后固定高亮，Esc 或「清除选择」取消。没有任何依赖的项只加环，不淡出别的 |
| 折叠与展开 | 点主线标题折叠整条主线（头部和计数留着）；点“N 项已完成”块展开成一行一项，点“另有 N 项”展开整组；依赖面板和里程碑泳道的标题同样可折叠；工具栏有「全部折叠 / 全部展开 / 显示已完成工作」。折叠后的图就是渲染器对该折叠状态画出的图，展开回来逐字节还原 |
| 搜索 | 工具栏搜索框按键、标题、负责人、里程碑、状态匹配，不匹配的卡片淡出；Enter 依次跳到下一个匹配（藏在折叠里的会自动展开）；`/` 聚焦搜索框 |
| 按状态过滤 | 点状态胶囊（可多选），其余状态的卡片淡出 |
| 只看某个执行者 | 点项目节点图下方“执行者”条里的执行者胶囊，等于按他的名字搜索：只剩他认领的卡片，再点一次放开 |
| 数据表跟着走 | 搜索和状态过滤同样淡出数据表里不相关的行；点任意一行打开同一个详情面板 |
| 缩放与平移 | 「− / 适应宽度 / 100% / +」，键盘 `+` `-` `0`，Ctrl/⌘ + 滚轮围绕指针缩放；按住图的空白处拖动平移。每张图各有自己的缩放 |
| 刷新后保持原样 | 刷新地图后，折叠、缩放、搜索和过滤保持不变 |

详情面板里的内容全部来自快照；快照没有的字段不猜、不补，列在“官方接口未提供”里。面板还有「在图中定位」「复制键」「复制 awr 命令」（`awr work show <键>`）。

**数据只来自 awr 的官方 JSON：** `work graph`、`nav --cached`（加 `--milestone`）、`search --type goal --cached`、`event history`（分页，固定在同一个 revision）、`session list --active`、`doctor`。所有应答必须属于同一个 project revision，读取期间项目变了就整体重读，仍不稳定则报错，不会画出半新半旧的图。**不读账本和 `.awr/state.db`，不调用模型，不改任何东西。** 官方接口没有的字段（每项的目标、范围、优先级，批量的里程碑）明确列为“不可得”，不会用别处的数据悄悄补上。

页面上的操作：

| 操作 | 说明 |
| --- | --- |
| 刷新地图 | 重新读取并重画。默认的 `work graph` 和其他页面一样会刷新来源投影 |
| 仅用已记录状态 | 改用 `work graph --cached`，不刷新来源（看到的是上次记录的事实）。需要支持该参数的 awr，否则会明确提示 |
| 下载 HTML | 把当前页面存成一个自包含的 HTML 文件：内联 SVG、样式和同一套交互脚本，数据（快照与配置）一并内嵌，无外部请求；不运行脚本的查看器仍能看到全部图和表。亮暗主题、窄屏横向滚动。读不到页面自己的模块文件时退而求其次，存成不含脚本的静态文件并在状态栏说明 |
| 下载快照 JSON | 保存画图所用的快照（`awr-project-snapshot` 版本 1，带指纹），可以离线重画 |

演示模式下显示一份内置的合成项目，不执行任何命令。

### 展示配置

主线（泳道）、依赖面板、“停滞”阈值、里程碑泳道是展示决定，不是项目数据。没有配置时：主线按工作项键的前缀自动分组（最多 6 条，其余归入“其他”），依赖面板取有内部依赖的最大几条主线，里程碑泳道为空并提示需要配置（官方 CLI 列不出里程碑）。要自己指定，写一个 JSON 文件并用 `--map-config` 启动：

```bash
node server.js --project /你的/项目 --map-config display.json
```

字段全部可选，完整示例见 [`examples/project-map.config.json`](examples/project-map.config.json)：

| 字段 | 含义 |
| --- | --- |
| `stale_days` | 状态为 `in_progress` 且超过这么多天没有任何事件，画成“停滞”（默认 7） |
| `titles.page` / `overview` / `mainline` / `explore` | 页面与各图的标题 |
| `strip_prefixes` | 卡片上显示键时去掉的前缀，例如 `["EX-"]` |
| `overview.lanes` | 主线列表：`id`、`name`、`tag`、`color`（十六进制）、可选的 `key_regex`（按键匹配） |
| `overview.match_order` / `default_lane` | 按此顺序用 `key_regex` 匹配，都不匹配的进 `default_lane` |
| `mainline_panels` | 画依赖面板的主线：`lane`，可选 `crit_end`（关键路径终点）、`keep_done`（不折叠已完成） |
| `explore_lanes` | 里程碑泳道：`name`、`color`、`milestones`（只对这里列出的里程碑逐个解析成员），可选 `goal`（用其标题做泳道说明） |

配置不合法时（颜色、正则、重复的泳道……）服务启动即报错，页面和导出也会给出明确提示，不会画出错误的图。

### 命令行导出

不开浏览器，直接在命令行得到同一个自包含 HTML（默认带交互；`--static` 得到不含任何脚本的版本，适合邮件附件和打印）：

```bash
node export-map.js --project /你的/项目 --out map.html
node export-map.js --project /你的/项目 --out map.zh.html --lang zh-CN --config display.json
node export-map.js --project /你的/项目 --out map.html --cached --save-snapshot snapshot.json
node export-map.js --project /你的/项目 --out map.static.html --static
node export-map.js --snapshot snapshot.json --out again.html
```

| 参数 | 说明 |
| --- | --- |
| `--project <目录>` | 通过 awr 读取的项目（默认当前目录） |
| `--snapshot <文件>` | 不读项目，直接画一份保存过的快照；快照会校验结构与指纹 |
| `--out <文件>` | 输出文件，默认 `awr-project-map.html`；`-` 写到标准输出 |
| `--lang <en\|zh-CN>` | 页面语言，默认 `en` |
| `--config <文件>` | 展示配置（同 `--map-config`） |
| `--cached` | 只读已记录状态（需要 `work graph --cached`） |
| `--static` | 文件里不含脚本：只有图、表和说明，没有详情面板等交互 |
| `--awr <路径>` | awr 可执行文件，默认取 PATH 上的 `awr` |
| `--save-snapshot <文件>` | 同时保存快照 JSON |
| `--now <时间>` | 记入快照的生成时间 `YYYY-MM-DDTHH:MM:SSZ`，默认当前时间；固定它可得到逐字节相同的文件 |

导出和页面用同一套抽取、绘制和交互代码：同一份快照、配置和语言，页面里的图与导出文件里的图相同，页面“下载 HTML”与命令行导出逐字节一致。数据以 JSON 内嵌（`<` 已转义，数据里的任何文字都不会提前结束脚本元素）。失败时只输出一行原因并以退出码 2 结束。

### 快照契约

快照（`awr-project-snapshot`，版本 1）是三张图唯一的输入，渲染是（快照，展示配置，语言）的纯函数：节点按键排序，边按（依赖方，前置）排序，JSON 键排序，`fingerprint` 是去掉 `fingerprint` 与 `generated_at` 后规范化 JSON 的 SHA-256，所以同一状态下重复抽取得到同一个指纹。`generated_at` 记录抽取时间，也是渲染器的“当前时间”，渲染不依赖本机时钟。版本 1 内只允许新增可选字段；改名、改义或删除要升版本。`unavailable` 列出官方接口没有提供的字段。若 awr 的 `work graph` 节点已带 `last_event_at`，抽取器直接使用它并省去事件历史翻页，结果与由事件历史归并得到的相同。

---

## 它到底改不改东西

说清楚这条边界，因为 AWR 是源优先的：

- **不编辑权威源。** 你的 Markdown / YAML 不会被这个工具改一个字。
- **不推进业务状态。** 不做状态流转、不写 evidence、不完成工作项。
- **但读操作会刷新源投影。** `awr status` 会报 `source_refresh_performed: true`，
  `context compile` 会报 `read_only: false`。也就是说它**不是运行时完全只读**，
  AWR 的 SQLite 投影和运行时状态可能被刷新。
- **重新索引是一次显式的维护操作**，默认关闭，要 `--allow-reindex` 才开，界面上还要再确认一次。

需要严格的运行时只读时，用 AWR 自己的 `--cached` 模式（代价是看到的是上次记录的事实，不是最新的）。

---

## 本地边界

绑定 `127.0.0.1` 并不够——浏览器里任何页面都能向回环地址发请求。所以 `/api/*` 还有一层检查：

| 检查 | 挡什么 |
| --- | --- |
| `Host` 必须是预期的回环形式 | DNS rebinding |
| 带了 `Origin` 就必须是本机本端口（`null` 也拒） | 跨站脚本调用 |
| `Sec-Fetch-Site` 为 `cross-site` / `same-site` 时拒绝 | 跨站请求 |
| 非 GET 必须带 `X-AWR-Inspector: 1` | 跨站表单 POST（CSRF）|
| 严格 CSP，无外部资源、无内联脚本 | 页面被注入后外连 |

另外：只能执行白名单里的 `awr` 子命令，参数按字段分别校验，`spawn` 不走 shell，
stdout / stderr / 请求体都有字节上限，并发子进程数有上限。

**不加载任何外部资源。** 用系统字体栈，没有外部字体、没有 CDN。上下文编译本身也是全本地、不调模型的。

---

## 超时的语义

只读命令和写命令处理方式不同：

- **只读命令**超时 60 秒：先 `SIGTERM`，5 秒后 `SIGKILL`，返回 `BridgeTimeout`。重试是安全的。
- **`source reindex`** 超时 120 秒：**不终止子进程**，返回 `OutcomeUnknown`。
  它可能已经生效了。界面不会自动重试，会让你先去查 AWR 的真实状态。
- 写命令的输出超过上限时也**不杀进程**，只丢弃多出来的部分，并同样报
  `OutcomeUnknown`——输出收不全不是终止一个正在改状态的操作的理由。
- 并发上限数的是**活着的子进程**，不是未完成的 HTTP 请求。超时先回响应、
  子进程还在跑时，那个名额仍然被占着，直到它真的退出。

---

## 界面上的数字对不上？

工作项页的「当前队列」表示进行中、可开工、等待中和阻塞队列，不包含已完成或已取消的历史任务。真实项目通过 `/api/work-page?queue=ready&offset=0&limit=10` 分页查询；桥接调用 `awr --json status --queue ready --offset 0 --page-size 10`。默认每页10项，可选20/50/100项，计数与页数据来自同一快照。四个队列均支持分页，超过100项可继续翻页。默认 status/MCP 摘要行为保持不变。查询失败显示错误；切换队列、每页数量时回到第一页，过期请求不会覆盖新页。需要同时更新本地 CLI 和 Inspector；旧 CLI 不支持此新增参数。

四条命令的映射在真实的 `awr 0.4.0` 和 `0.5.0` 上都核对过。

### 版本差异

0.4.0 和 0.5.0 的 `status` 输出**不是同一个形状**，本工具两种都认：

| | 0.4.0 | 0.5.0 及以后 |
| --- | --- | --- |
| `status` 的队列 | 只有 `current` 数组 | `current`/`ready`/`waiting`/`blocked` 四个数组 |
| ready 列表从哪来 | 另跑 `awr ready` | `status` 自带 |
| waiting 队列 | **没有** | 有 |
| 截断条数 | `ready_total` 减列表长度 | `omissions.<队列>` |
| `pending_operations` | 没有 | 有 |

版本里没有的队列，界面显示「—」并说明原因，不拿 0 冒充「没有」；
`pending_operations` 缺失时整块面板隐藏。0.5.0 发布后本工具未改一行代码即适配。

遇到某一格显示「—」：

1. 展开那个页面底部的 **「原始 JSON」**，看真实字段叫什么。
2. 打开 `public/app.js`，最上面有一张 `FIELD_MAP` 表。
3. 把真实字段名加进对应的候选数组里，刷新页面。

所有字段映射都集中在那一张表里，别处不猜字段。

### 一个测不出来的数

界面上没有「不用 AWR 要读多少 token」这种对比条。AWR 不报语料体积，浏览器里也没有
o200k 分词器，这个数造不出来。仓库公开 benchmark 的那组对比（18,955 → 4,998）写在
新手引导第一页，并标明那是 39 个活跃任务上的测量值、不是你项目的数。
你自己项目的实测，在「上下文」页编译一次就有。

---

## 测试

```bash
node --test test/*.test.js
```

48 个用例，零依赖，分三档：

- `test/bridge.test.js` —— 起真实的 `server.js` 子进程、打真实 HTTP 请求，PATH 上放一个
  假 `awr`（`test/fixtures/stub-awr.js`）。覆盖请求来源边界、命令构造、子进程输出、
  超时与生命周期语义、演示模式。
- `test/detail.test.js` —— 在一个最小 DOM 替身（`test/fixtures/dom-stub.js`）上跑
  `app.js` 里**真正的** `renderWorkDetail()`，不是抄一份副本来测。覆盖缓存命中时
  详情与原始响应是否配套、迟到响应（成功与失败）的丢弃、刷新后旧响应的作废。
- `test/packet-size.test.js` —— 同样在替身上跑真正的 `renderPacketSize()` 和
  `doCompile()`。覆盖编译后体积面板会不会填上、三条数字是否原样取自 AWR、
  空态有没有承诺做不到的事、换一次编译旧数字会不会残留。另外三条查页面自身的一致性：
  每个 `?` 都有对应的说明段落（点了没反应的按钮界面上看不出来）、没有打不开的说明、
  主区不再有固定宽度上限。再三条守 `BudgetExceeded` 的处理：重试按钮不超过 AWR 的上限、
  必需量本身超上限时不给必然失败的按钮、先成功再失败时上一次的数字不残留。

CI 见 `.github/workflows/inspector.yml`。

超时相关的用例靠三个只给测试用的环境变量把等待时间压下来：
`AWR_INSPECTOR_READ_TIMEOUT_MS`、`AWR_INSPECTOR_WRITE_TIMEOUT_MS`、
`AWR_INSPECTOR_CONCURRENT`。不设就用默认值。

---

## 它在仓库里的位置

`tools/inspector/`，不在 Cargo workspace 里（`Cargo.toml` 的 members 是显式列举的），
所以 `cargo build` 不会碰它，它也不需要 Rust 工具链。

```
server.js            本地桥接：一个 HTTP 请求 = 一条 awr 命令
start.sh             一键启动
public/
  index.html         页面结构
  styles.css         设计令牌与组件（亮/暗双主题，系统字体）
  app.js             字段映射、渲染、新手引导
  demo-data.js       演示数据（结构与真实 JSON 一致）
test/
  bridge.test.js        桥接测试
  detail.test.js        详情面板的前端回归
  packet-size.test.js   上下文体积面板的回归
  fixtures/stub-awr.js  假的 awr，用来制造边界情况
  fixtures/dom-stub.js  最小 DOM 替身，让 app.js 能在 Node 里跑
```

---

## 设计上的几条硬规则

写在这儿，改代码时别破坏：

- **源文件是唯一的记录来源。** 不编辑 Markdown / YAML，不自己读源文件补数据。
- **数字不自己推算。** 队列的「还有几条」来自 AWR 给的计数，不是列表长度。
- **不混用两套 blocked。** `awr ready` 的 `blocked_total`（不可选，含已被 claim 的）和 action view 的
  `blocked_count`（真实阻塞）定义不同。状态条只用后者。
- **错误原样显示。** AWR 的 `code` 和 message 不重写，只在下面另起一行给建议动作。
- **被规则挡下的敏感内容不回显。** AWR 故意不返回匹配到的原值，本工具也不去读源文件补出来。


### Agent-first Team onboarding

Task details preserve the repository-neutral collaboration facts from the same
atomic `work.snapshot`: selected candidate digest/version, current binding,
reported verification runs, AWR review, repository integration and source
publication phase. Missing or truncated facts remain explicit. These stages do
not imply each other, and a recorded integration may refer to an earlier request;
inspect its bound receipt before relying on it. No GitHub connection is required.
Optional public PR observations retain their own provenance and timestamp.

Current guidance shows its applicable condition, recorded basis, one next step
and reevaluation trigger. An expandable read selector preserves the server's
authorized query for a connected Agent; the page does not execute it or acquire
tasks. Snapshot project/source versions and task requirements are available with
the reporting sources. Older services without neutral facts display their
absence rather than inferring completion from a PR or progress report.

Members connect their Agent directly to the project's remote MCP endpoint using
an administrator-provisioned personal credential. The Agent refreshes tasks,
recovers its own sessions, claims eligible work, obtains execution admission,
saves checkpoints and submits evidence for review. No browser sign-in or task
selection is required. See the [Team Agent workflow](../../docs/dev/integrations/team-agent-workflow.md).

Inspector displays authorized project state. **Connect Agent** provides
client-neutral MCP connection details and a project instruction; task details offer an
optional copyable brief. These controls make no work-session or claim writes.
Signing in only opens the workspace; it grants no additional permissions.
Signing out does not end Agent work sessions or release claims. Public Git
repository access is separate from Team permissions.

When a reverse proxy exposes Team MCP at a different address from `--team-url`,
set `--team-public-url https://your-team.example` so copied connection details
use the member-facing service. It defaults to the configured Team service URL,
not the Inspector page URL.


### Member management and activity

With a compatible Team server, Inspector shows a Members tab only when the
current credential has project administration authority and explicit workstream
manage grants. Administrators can add members, change project roles/scopes,
issue or rotate project credentials, and remove project access. Every change
passes server preview/apply gates; browser visibility is not authorization.

The browser creates a random credential with Web Crypto and registers only its
hash. After a confirmed commit, copy the complete personal Agent instruction
once and send it privately. Clearing the panel, switching projects or logging
out removes the plaintext. Unknown outcomes keep the original request ID and
require inspection before an exact retry. There is no plaintext retrieval.

Activity separates authenticated access metadata from recorded development
actions. Project auditors can filter by member or task; ordinary members have a
personal view. Queries and grant checks run on the server, including pagination.
The connected Agent uses `work.next` to continue or discover work; the browser
does not claim tasks or choose an Agent product on the member's behalf.
