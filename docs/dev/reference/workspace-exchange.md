# 跨机器 Workspace 交换平面

两台机器上的 agent 服务同一个项目，但**不共享文件系统**。`awr workspace`
把项目里被跟踪的源字节、证据产物和交接单元送到一台对象存储上，另一端拉回去。

它的定位是**交换平面（exchange plane）**，不是"共享一个 AWR 状态库"：权威源仍然只在拥有它的那台机器上权威，
`.awr/`（运行时数据库、凭据、workspace 状态）永不外传。

```
dev 机器 (Codex)                         peer 机器 (Hermes)
  权威源 + .awr/state.db                   权威源 + .awr/state.db
        │                                        │
        └── publish ─→  对象存储  ←─ sync ───────┘
                        (R2 / S3 / OSS / MinIO)
```

## 命令

```sh
awr workspace status   [--pointers]        # 逐文件：本机 / 基准 / 工作区，各是什么状态
awr workspace publish  [--dry-run]         # 送出本机跟踪的文件；dry-run 只报告，不写存储
awr workspace sync     [--handoff-outdir D]# 取回对端的文件与入站 handoff
awr workspace drop     --path P [--path P] # 从索引里拿掉路径；本机文件不动
awr workspace verify-index                 # manifest 与逐文件指针镜像的漂移
awr workspace handoff push --file F --name N   # 发布交接单元（一次写入）
awr workspace handoff pull --outdir D          # 收取其他主机写的交接单元
awr workspace credential set --input F | --stdin
awr workspace credential status | clear
```

`--config` 默认 `<项目根>/remote_workspace.toml`，可写在子命令前后。`sync` 的入站 handoff 默认落在 `<root>/infra/handoffs/<origin>/`；如果这个目录本身在 `track` 里，它会被当成普通被跟踪文件再发布出去，所以要么把它排除在 `track` 之外，要么用 `--handoff-outdir` 指到别处。
停止共享某个路径是两步：先从 `project.track` 里拿掉（或收窄）它，再 `awr workspace drop --path P`。
`drop` 从索引和指针镜像里去掉这条路径，不删本机文件，也不删内容对象；路径还被 `track` 覆盖时它会拒绝——否则会话开始的自愈会马上把它登记回去。
`sync` 仍会取回索引里有、本机 `track` 没有的路径：那是新对端文件出现的方式。
`--json` 输出额外带 `backend`、`backend_requests`、`elapsed_ms`，把"离存储有多远"变成可观测量。

## 会话开始时的自动取回

`awr client hook` 收到 `SessionStart` 时，会先做一次 `sync`（取回对端文件与入站 handoff），
再把结果写进本次会话上下文——所以"对端在我离线时改了东西"不依赖 agent 记得敲命令：

- 只有项目根存在 `remote_workspace.toml` 才生效；没有这个文件时是完全无操作，零请求、零输出。
- 取到东西时列出具体路径（最多 24 条，其余计数），并说明本机磁盘上的字节已经是新的。
- **自愈也在这里发生**：本机发布过、却被对端"陈旧读提交"挤掉的条目，会在会话开始时被重新登记
  （`repair_index`）。它只补这些条目——本地字节必须与当初发布的一模一样——所以不会把半成品发布出去。
  没有东西要修时，代价是固定多读一次索引；万一连这次补登记都被对端连续抢先，上下文会明说
  "这一轮没补上，跑一次 `awr workspace publish`"，而不是悄悄跳过——那意味着自愈这次没有发生。
- 取不到时（没凭据、存储不可达）只报告一行，**不改变退出码**：hook 是会话入口，
  不能因为对端离线就打不开会话。冲突同样只报不猜，逐条列进上下文，本地文件保持原样。
- 只拉不推：会话开始时本地工作树常常是半成品，自动 `publish` 会把它登记成对端的新基准。
- `PostCompact` 不触发：同一会话中途压缩没有"我不在时发生了什么"这层含义。
- **取回在 AWR 自己的 source 刷新之前**，所以两条后果都发生在本会话而不是下一次：
  对端推来一个解析不了的源，本会话按 AWR 自己的守卫退出 1 `SourceStale`（与手工改坏源文件同一条路径，
  交换平面不绕过它）；对端随后补一份可解析的版本，本机就会在会话开始时被换回来、会话正常打开。
  注意这**不等于**"对端能覆盖你的本地修改"：本机自己改过（本地 ≠ 基准）的文件仍然按冲突处理，
  一个字都不动。
- 代价是会话开始多一次存储往返（实测同区对象存储 200ms 上下）。**整个取回有一个 10 秒预算**：
  它在自己的工作线程上跑，超时就不再等，并给该线程设取消标志：之后的本地写入与 state 保存会停止，
  只在上下文里留一句 "left for later, run `awr workspace sync`"。之所以需要它，是因为存储自己的
  请求超时是 60s——对盯着看的操作者合理，对会话入口不合理。超时前已写入的文件留在磁盘上，
  下一次 `sync` 接着取；超时后不会继续覆盖 agent 已开始改的源文件。

## 功能边界：这层做什么，不做什么

这是 **个人跨机器** 的字节交换平面，不是团队协作地基：

1. **只做字节交换，不做治理。** 没有身份、没有权限、没有任务调度、没有语义合并。这些不是"待补全"，而是明确不属于这一层。
2. **不是多人协作功能的地基。** 未来的多人/团队协作会构建在 AWR 内核的 claim/session/权限扩展上（中心或共享权威），而不是在这层 P2P 字节同步上叠身份。若未来需要多人共享传输，桶布局、per-actor 凭据、审计、权限模型都需要重新设计；当前单桶 CAS 不支撑。
3. **不建议多人用这层同步同一份 work ledger。** 每人各自的 `.awr` 事件历史会分叉，没有统一回执与审计；多人场景应等待内核的协作扩展。

明确目标场景：同一操作者的笔记本 + 台式机 + 服务器，顺序使用（publish 在一台，sync 在另一台）。

## 配置：只写这台机器才有的东西

```toml
[project]
key   = "my-project"                    # 全队一致，也是桶内前缀
root  = "/absolute/path/to/project"     # 这台机器的项目根
host  = "alice-laptop"                  # 每台机器唯一
track = ["work-ledger.yaml", "infra/evidence", "infra/scripts"]

[store]
endpoint = "https://<account-id>.r2.cloudflarestorage.com"
bucket   = "my-workspace"
```

必填只有 6 个键，其余全有默认值，因此**换机器只改 `root` 和 `host`**：

| 字段 | 默认 | 何时要写 |
|---|---|---|
| `state` | `<root>/.awr/workspace.json` | 想把状态放别处时；相对路径按配置文件所在目录解析 |
| `backend` | 有 `endpoint` 即 `s3`，有 `path` 即 `local` | 极少需要 |
| `region` | `auto` | OSS 填真实 region（如 `cn-beijing`） |
| `addressing` | `auto`（host 以 `<bucket>.` 开头即虚拟主机） | **OSS 公网填 `virtual`**；R2/S3/MinIO 保持默认 |
| `prefix` | `""` | 多个项目共用一个桶时 |
| `concurrency` | `8`（上限 32） | 链路能承受更高并发时 |
| `path` | 无 | 用目录代替对象存储（测试与单写者演示） |
| `allow_insecure` | `false` | 仅当需要非 loopback 的 `http://` 实验端点时设为 `true`；默认只允许 `https://`，loopback `http://` 无需此标志 |

未知键、拼错的键、未知的 `backend` 都是错误而不是静默默认：配置文件是要被复制、被提交的，
一行拼错就意味着两台机器的行为不同，这是跨机交换平面最贵的故障类型。

## 凭据：每台机器一份，永远不进入配置文件

`awr workspace credential set --stdin` 把 `{"access_key","secret_key"[,"session_token"]}`
写到 `<项目根>/.awr/workspace-credentials.json`。Unix 上权限 600（不是 600 会在读取时被拒绝），
并以 temp+rename 写入以免中断留下截断文件。Windows 上同一路径同样 temp+rename，但**没有**等价的
mode 600 检查——请用 NTFS ACL 保证 `.awr/` 仅所有者可访问；这是当前平台限制，不是静默跳过安全模型。
环境变量 `AWR_WORKSPACE_ACCESS_KEY` / `AWR_WORKSPACE_SECRET_KEY` / `AWR_WORKSPACE_SESSION_TOKEN`
作为兜底，供无法写文件的作业执行器使用；文件优先。

因此配置文件里出现 `ak` / `sk` / `access_key` / `secret_key` / `session_token` 会**直接报错**，
并提示改用 `awr workspace credential set`；`status` 只报告哪些字段存在和文件权限，从不回显值。
密钥也不能作为命令行参数传入：CLI 在参数解析前就拒绝含敏感值的 argv。

## 存储布局

```
projects/<key>/manifest.json                       索引（CAS 提交）
projects/<key>/files/<url-encoded path>/<sha256>   内容对象（内容寻址，只写一次）
projects/<key>/files/<url-encoded path>/current.json  逐文件指针镜像（派生）
projects/<key>/handoffs/<origin host>/<name>.json  交接单元（if-none-match，一次写入）
```

- **项目内相对路径整体作为单个 key 段做百分号编码**（`infra/a.json` → `infra%2Fa.json`），
  这样存储侧无论怎么处理 `/` 都不能把两条不同路径折成一条。
- **内容寻址**：同内容天然去重，断点续传只需补缺失的内容对象。
- **manifest 是索引**：一次 GET 取代逐文件指针扇出；每一次发布只对 manifest 做一次 CAS，
  所以被中断的发布不会半可见。指针镜像只为单文件读者保留，是派生数据，
  写失败不影响发布结果（会记在 `pointer_mirror_failures`）。
- **提交保证**：R2/S3/MinIO 用 `If-Match` 真 CAS（`commit_mode: "cas"`）；
  OSS `PutObject` **没有条件写**，改用"写前查索引 + 写后读回校验"（`commit_mode: "guarded"`）——
  仍是一次原子提交（manifest 单对象），缺的是互斥。写前 HEAD 能发现"提交前索引已变"并重读重试；
  **不能**防止对端插在 HEAD 与 PUT 之间：本端后写会覆盖对端，readback 看到自己的 ETag 仍报成功。
  因此 guarded 模式只适合**个人顺序使用**（一台 publish、另一台再 sync）。两台机器可能同时 publish 时，
  请使用支持 `If-Match` 的存储（R2/S3/MinIO，`commit_mode: "cas"`），不要依赖 OSS guarded 做多写者互斥。
  被挤掉的条目在失主那边可能报成 `index_missing_local`，重新 `publish` 可补回（内容对象仍在）。
  提交前发现索引已变就重读、合并、重试，最多三次；三次都被对端抢先时，这批路径逐条列进
  `contended`，`publish` / `drop` 以 exit 1 `WorkspaceContended` 退出并在正文里写"publish again"。

## 状态与冲突

| 状态 | 含义 | 建议动作 |
|---|---|---|
| `in_sync` | 本机与工作区一致 | 无 |
| `local_ahead` | 只有本机变了 | `publish` |
| `remote_ahead` | 只有对端变了 | `sync` |
| `index_missing_local` | 本机发布过的条目从索引里消失了 | `publish` 重新登记（内容对象还在） |
| `missing_local` | 工作区有、本机没有 | `sync` 取回，或确认它不该在这里 |
| `conflicted` / `conflicted_first_sync` | 两端都变过、且没有共同基准 | 人工决定，工具不猜 |

冲突**从不自动合并**：`sync` 与 `publish` 都会以 `WorkspaceConflict` 退出（exit 1），
本地文件保持原样，正文里逐条列出冲突路径。`publish --dry-run` 是预览：同样列出冲突，
但 exit 0，存储与本机都不动。

`contended` 不是状态，是"这一轮对索引的提交一次都没成功"的报告，来自 `publish`、`drop`
（以及会话开始时的自愈补登记）——取回不写索引，所以不会产生它。CLI 把它报成
`WorkspaceContended`，和 `WorkspaceConflict` 分开：冲突要人去调解，`contended` 只要再执行一次同一命令。

三个跟踪文件必须满足 `track` 里的路径是**可移植名字**：项目内相对路径，
不含绝对路径、`..`、反斜杠、盘符或空路径段。这条规则同时守住两个方向——
发布不会读到项目根之外，取回也不会写到项目根之外（索引是远端数据，不是事实）。

## 边界

- **跨机没有强互斥**：交换平面保证"一次发布要么全可见要么不可见"，不保证两台机器不会同时改同一个文件。
  后者会被报成冲突，这正是不自动合并的原因；两台机器同时发布**不同**文件时也可能有一方连续三次
  提交都晚一步，那是 `contended`（再发布一次，字节没丢也没被改写），不是冲突。
- **`track` 就是边界**：被跟踪目录里的文件全部进工作区（含点文件，与参考实现一致），但树里出现 `.git` 或 `.awr` 目录会直接拒绝——Git 历史与本机 AWR 运行时永不外传，要发布就请把 `track` 收窄。
- **`.awr/` 不参与**：`active_claim` 唯一索引与 revision CAS 的前提是单机单写者，
  两台机器各改一份状态库只会互相判 stale。交换平面只传源字节、证据与交接单元。
- **产物的秘密策略不覆盖这里传的字节**：`awr-core` 的秘密策略作用于 AWR 自己的输入/源/事件，
  workspace 传的是被跟踪文件的原始字节。不要把凭据放进被跟踪的源里。
- **`local` 后端只有进程内锁**：它服务测试套件和单写者演示；多于一个写者就需要真正的对象存储。
- **同版本成对**：只跑旧版（只有指针、没有 manifest）的对端会让 manifest 变陈旧。
  `verify-index` 用来发现漂移；删除 `manifest.json` 即回退到指针布局。

## 实测：无 CAS 存储上被收窄的提交窗口

同一份构建、同一个真实 OSS 桶，唯一变量是左边那份二进制里有没有"写前查索引"这一步。
每轮：一台机器一次发布 40 个新文件（内容阶段约 1.4s），另一台在 200ms 后发布 1 个文件，
让那次小发布正好落进批量发布"已经读过索引、还没写回"的窗口里。

| | 小发布的索引条目被挤掉 | 批量发布请求数 | 小发布请求数 |
|---|---|---|---|
| 写前不查索引 | 4/4 轮 | 83 | 5 |
| 写前查索引 | 0/4 轮 | 86 | 6 |

两条代价都看得见：每次 guarded 提交多一次 HEAD（小发布 5→6）；批量发布那 +3 是它**真的撞上了**
——第一次提交在写之前发现索引已变，于是重读、合并、再提交。被挤掉的条目也不丢字节：
失主 `status` 报 `index_missing_local`，再 `publish` 一次就补回（`content_uploaded: false`，内容对象还在）。
所以这里说的是"窗口收窄到一个请求"，不是"窗口为零"；剩下的部分按"接受 + 自愈"处理。

窗口内的极端交错仍然可能发生：两端同时读到同一份索引、又都在对方写回之前通过检查，
后写的那次提交仍会挤掉对方刚登记的条目。它对两端都是可观测、可自愈的，工具不猜。

### 实测：另外三条边界

都在同一个真桶、跨真机（Mac 跑 Rust，mac mini 跑冻结的 Python 参考实现）跑过：

- **三台主机**：三台各自先发布一个文件、再各自 `sync`，三端都拿到 3/3 且全部 `in_sync`；
  同一文件三方同时改，先发布者胜出，另外两台各自 exit 1 `WorkspaceConflict` 且**本地字节一字未动**；
  按文档的做法（把本地改回基准再 `sync`）三端收敛到胜出版本。
- **handoff 一次写入**：两台主机不会撞车——key 里带 origin host，各写各的命名空间；会撞的是同一台机器上
  两个写者。同一瞬间用同名写两份不同内容，4/4 轮都是"一份创建、一份 exit 1 `WorkspaceConflict`"，
  对象内容始终是其中一份的完整字节——说明 OSS 上的 `x-oss-forbid-overwrite` 真的生效，不是被忽略。
  同名同内容重放是 `idempotent_replay: true`。
- **会话开始的预算**：把 endpoint 指向一个"接受连接、永不回答"的本地端口，会话 10.4s 打开、
  exit 0，上下文里是 "was still answering after 10s and has been left for later"（存储自身的请求超时是 60s）。

### 实测：三台 / 四台同时提交

同一真桶、跨真机：Mac 上跑两到三个 Rust 项目根，mac mini 上跑冻结的 Python 参考实现。
每轮把各端的 `publish` 对齐到同一个瞬时（先测对端时钟偏移，再统一起跑），每端发布自己的新文件
——三写者那组里最忙的一台每轮 20 个文件。**这是最坏情况**：真实协作里几台 agent 不会约在同一毫秒提交，
所以下面的数字是上界，不是日常值。

| 场景 | 轮数 | 提交条目 | 被挤掉（含报成 `contended` 的） | 自愈 |
|---|---|---|---|---|
| 三写者，各 1 个文件 | 24 | 72 | 7（0 次报 `contended`） | 每条 1 次 `publish` |
| 三写者，最忙的一台每轮 20 个文件 | 6 | 132 | 0 | 不需要 |
| 四写者，各 1 个文件 | 20 | 80 | 27（5 次报 `contended`） | 每条 1 次 `publish` |

- 撞不撞取决于两端**提交时刻差多近**，不取决于数据量：同机三个根（时钟对齐）的四写者几乎每轮都撞，
  20 文件那组反而一次没撞；跨机那台（SSH + 实测时钟偏移 ~370ms）在四写者里一次没输过。
- **没有一次静默丢失**：每个被挤掉的条目，失主自己的 `status` 都报 `index_missing_local`；
  走 `contended` 那几次本机报 `local_ahead`（"还欠一次发布"）。也没有反向的误报
  （没有谁声称"我丢了"而索引里其实还在）。
- **每条都靠一次 `publish` 补回**，没有出现要补第二次的；每轮结束后所有主机都收敛到 `in_sync`、
  逐字节一致，索引里没有悬空条目（每个条目都能找到对应的内容对象）。
- 那 5 次 `contended` 都如实 exit 1，并且**没有任何路径被改写**——这正是它和冲突的区别：
  冲突要人决定，`contended` 只要再发布一次。

## Project-relative evidence locators

`project:///<project-relative-path>` makes one evidence row point at the same relative path
inside whatever checkout each node holds.

**What "project root" means.** The root stored in the project record (the directory that holds
`.awr/project.toml`), taken through `canonicalize()` at read time. If the root has moved, the
read fails with `project root moved since registration` instead of guessing a new location.

**One rule.** A `project://` locator resolves inside the project root and nowhere else: `..`,
absolute paths and every other escaping component are refused by `Locator::parse`
(`project locator must stay inside the project root`), and the read repeats the containment
check against the `canonicalize()` result. **`authorized_roots` does not take part in
`project://`**: it is node-local configuration, so letting it satisfy a project locator would
resolve one stored string to different bytes on different nodes. `file://` and plain paths keep
using `authorized_roots` as before.

**Different bytes must fail.** Registration resolves the locator once and stores the canonical
identity. The content is bound at the same moment: a declared `sha256` has to match the bytes the
locator resolves to, and when the caller declares none, the resolved digest is stored in its
place, so the row always names the content a later node is expected to serve. Reading on another
node then fails with
`registered content bytes no longer match the SHA256 bound at registration` when the same
relative path holds different bytes, and **never falls back to another copy on that node**. The
cross-node contract is therefore "same relative path + same `sha256`", not "same machine".
Evidence body reads (`evidence show --content`) verify against the stored `sha256`.

Known limit: the locator carries no root fingerprint, so two checkouts with the same relative
path and the same bytes cannot be told apart. Stronger root identity is still open (#348).

## 验证入口

```sh
cargo test -p awr-workspace --locked
cargo test -p awr-cli --test workspace_exchange --locked
```

单元测试覆盖签名（AWS 官方向量）、键布局、配置、凭据、对象原语后端、manifest/冲突/CAS 语义、
真实的 `publish --dry-run`、两步 `drop`，以及"连续三次提交都被对端抢先"（`WorkspaceContended`）
的报告与重试自愈；
`workspace_exchange` 用两个项目根加一个目录存储，跑真实的 CLI、配置与提交路径，
不需要网络和凭据。真实对象存储的双机联调按 `examples/workspace/README.md` 的步骤执行。
