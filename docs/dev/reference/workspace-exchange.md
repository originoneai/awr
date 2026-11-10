# 跨机器 Workspace 交换平面

Workspace 在同一操作者的多台机器间交换跟踪文件、证据和交接单元；使用 publish 送出、sync 取回。

## Use it

`awr workspace` 将跟踪的源字节、证据产物和交接单元发布到对象存储，供同一操作者的另一台机器取回。

权威源在拥有它的机器上保持权威，交换平面传送源文件和产物字节。

```
dev 机器 (Codex)                         peer 机器 (Hermes)
  权威源 + .awr/state.db                   权威源 + .awr/state.db
        │                                        │
        └── publish ─→  对象存储  ←─ sync ───────┘
                        (R2 / S3 / OSS / MinIO)
```

### 命令

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

`--config` 默认 `<项目根>/remote_workspace.toml`，可放子命令前后。入站 handoff 默认 `<root>/infra/handoffs/<origin>/`；将该目录排除 `track` 或改用 `--handoff-outdir`，避免重复发布。停止共享先收窄 `project.track`，再 `drop --path P`。`sync` 会取回索引里新出现的路径。JSON 额外给出 `backend`、`backend_requests`、`elapsed_ms`。

## How it behaves

| 操作 | 结果 |
| --- | --- |
| publish | 内容对象与索引提交 |
| sync | 取回文件与 handoff |
| drop | 移除共享引用 |
| status / verify-index | 冲突、竞争与漂移诊断 |

### 会话开始时的自动取回

`awr client hook` 收到 `SessionStart` 时，会先做一次 `sync`（取回对端文件与入站 handoff），
再把结果写进本次会话上下文——所以"对端在我离线时改了东西"不依赖 agent 记得敲命令：

| SessionStart 条件 | 行为 |
| --- | --- |
| 存在 `remote_workspace.toml` | 自动 sync，然后刷新 AWR 来源并注入本次上下文 |
| 取回文件 | 列出最多 24 条具体路径及其余数量，磁盘字节已更新 |
| 原发布字节仍相同、索引条目被挤掉 | `repair_index` 重新登记；固定多读一次索引 |
| 修复提交持续被抢先 | 上下文提示运行 `awr workspace publish` |
| 凭据/存储故障 | 一行诊断，继续会话入口 |
| 本地已修改、对端也变化 | 逐条报告冲突，保留本地字节 |
| 对端源解析失败 | 本次 source 刷新按原守卫 exit 1 `SourceStale` |
| 对端随后修好来源 | 下一次入口取回并正常打开 |
| 存储往返 | 历史同区测量约 200 ms |
| 整个取回 | 独立工作线程、10 秒预算；存储请求本身超时 60 秒 |
| 预算到期 | 设置取消标志，提示 `left for later, run awr workspace sync` |

### 使用场景

此交换平面服务同一操作者的多台机器。

团队协作采用 AWR 内核的 claim/session/权限扩展和共享权威。若引入多人传输，需重新设计桶布局、逐 actor 凭据、审计和权限。

明确目标场景：同一操作者的笔记本 + 台式机 + 服务器，顺序使用（publish 在一台，sync 在另一台）。

### 配置：只写这台机器才有的东西

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

配置键和 backend 严格校验；拼写或值错误直接报错，便于复制配置时保持机器行为一致。

### 每台机器的凭据

`credential set --stdin` 将 `{"access_key","secret_key"[,"session_token"]}` 写入 `<项目根>/.awr/workspace-credentials.json`，采用 temp+rename。Unix 读取要求权限 600。环境变量 `AWR_WORKSPACE_ACCESS_KEY` / `AWR_WORKSPACE_SECRET_KEY` / `AWR_WORKSPACE_SESSION_TOKEN` 供作业执行器兜底，文件优先。

配置中的 `ak`、`sk`、`access_key`、`secret_key`、`session_token` 报错并提示 `credential set`。CLI 在参数解析前拒绝敏感 argv。

### 存储布局

```
projects/<key>/manifest.json                       索引（CAS 提交）
projects/<key>/files/<url-encoded path>/<sha256>   内容对象（内容寻址，只写一次）
projects/<key>/files/<url-encoded path>/current.json  逐文件指针镜像（派生）
projects/<key>/handoffs/<origin host>/<name>.json  交接单元（if-none-match，一次写入）
```

| 存储机制 | 作用 |
| --- | --- |
| 相对路径整体编码成单个 key 段 | `infra/a.json` → `infra%2Fa.json`，保持路径身份 |
| 内容寻址 | 同内容去重，续传补缺失对象 |
| Manifest 索引 | 一次 GET 代替指针扇出，每次发布一次 manifest 提交 |
| 指针镜像 | 服务单文件读者的派生数据，失败计入 `pointer_mirror_failures` |
| R2/S3/MinIO | `If-Match`，`commit_mode: "cas"` |
| OSS | HEAD 检查索引，PUT 提交，读回核验，`commit_mode: "guarded"` |
| HEAD 已发现变化 | 重读、合并、重试，最多三次 |
| 三次均被抢先 | 路径列入 `contended`，publish/drop exit 1 `WorkspaceContended`，提示 publish again |
| 条目被后写挤掉 | 失主可见 `index_missing_local`，publish 重登，内容对象仍在 |

### 状态与冲突

| 状态 | 含义 | 建议动作 |
|---|---|---|
| `in_sync` | 本机与工作区一致 | 无 |
| `local_ahead` | 只有本机变了 | `publish` |
| `remote_ahead` | 只有对端变了 | `sync` |
| `index_missing_local` | 本机发布过的条目从索引里消失了 | `publish` 重新登记（内容对象还在） |
| `missing_local` | 工作区有、本机没有 | `sync` 取回，或确认它不该在这里 |
| `conflicted` / `conflicted_first_sync` | 两端都变过、且没有共同基准 | 人工决定，工具不猜 |

`sync` / `publish` 遇冲突逐条列路径，exit 1 `WorkspaceConflict`。`publish --dry-run` 预览同样的冲突，exit 0。

`contended` 表示本轮三次索引提交均被抢先，来自 publish、drop 或入口 repair_index；CLI 返回 `WorkspaceContended`，重跑同一命令即可。

`track` 使用可移植项目相对路径，发布与取回都检查项目根包含关系。

### 实测：无 CAS 存储上被收窄的提交窗口

同一份构建、同一个真实 OSS 桶，唯一变量是左边那份二进制里有没有"写前查索引"这一步。
每轮：一台机器一次发布 40 个新文件（内容阶段约 1.4s），另一台在 200ms 后发布 1 个文件，
让那次小发布正好落进批量发布"已经读过索引、还没写回"的窗口里。

| | 小发布的索引条目被挤掉 | 批量发布请求数 | 小发布请求数 |
|---|---|---|---|
| 写前不查索引 | 4/4 轮 | 83 | 5 |
| 写前查索引 | 0/4 轮 | 86 | 6 |

每次 guarded 提交多一次 HEAD（小发布 5→6）。批量发布增加 3 请求，因为写前发现索引变化后重读、合并和重试。被挤掉条目的内容对象保留；失主 status 报 `index_missing_local`，一次 publish 补回，`content_uploaded: false`。

极端交错时，两端读同一索引并都通过检查，后写仍会挤掉先前条目。双方可观察并通过重新发布恢复。

#### 实测：另外三条边界

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

#### 实测：三台 / 四台同时提交

同一真桶跨机实测：Mac 两到三个 Rust 项目根和 mac mini 冻结 Python 对端。校正时钟偏移后对齐 publish，每端发布新文件；三写者最忙端每轮 20 文件。

| 场景 | 轮数 | 提交条目 | 被挤掉（含报成 `contended` 的） | 自愈 |
|---|---|---|---|---|
| 三写者，各 1 个文件 | 24 | 72 | 7（0 次报 `contended`） | 每条 1 次 `publish` |
| 三写者，最忙的一台每轮 20 个文件 | 6 | 132 | 0 | 不需要 |
| 四写者，各 1 个文件 | 20 | 80 | 27（5 次报 `contended`） | 每条 1 次 `publish` |

| 历史观测 | 结果 |
| --- | --- |
| 提交时刻差 | 决定竞争；同机根对齐时常碰撞，20 文件组为零 |
| 跨机 SSH 与时钟偏移 | 约 370 ms；四写者组中该端保持成功 |
| 所有被挤掉条目 | 失主 status 均报 `index_missing_local`；contended 本机为 `local_ahead` |
| 报警准确性 | 每条报告与索引缺失对应 |
| 恢复 | 每条一次 publish 补回；每轮所有端 in_sync、逐字节一致、索引对象完整 |
| 五次 contended | exit 1，所有路径字节保持原样，重跑 publish 恢复 |

### Project-relative evidence locators

`project:///<project-relative-path>` is the legacy identity: one evidence row points at the same
relative path inside whatever checkout each node holds. When `[project] external_key` is set in
`.awr/project.toml`, **new** registrations store
`project://<encoded-external_key>/<project-relative-path>` instead. Rows already stored as
`project:///` keep parsing and reading as they do today; they are not rewritten on read.

The key is authored in `.awr/project.toml`, which is a tracked project file (`awr init` ignores
only runtime state such as `.awr/state.db`, `.awr/artifacts/` and `.awr/executions/`). Two
checkouts of the same project therefore share the same key. The key separates **different
projects**; it does not separate two checkouts of the same project. It is never taken from the
SQLite name fallback used at `register_project`. Manifest validation only requires a non-empty
key. Locator parse splits on the first unencoded `/` after `project://`, so a key that contains
`/` (or any other reserved byte) is percent-encoded with uppercase hex (`awr-v1/records` becomes
`awr-v1%2Frecords`). Parse percent-decodes the key and requires `encode(decode(s)) == s`; an
unencoded reserved byte, lowercase hex, or over-encoded unreserved byte is `InvalidInput`. The
decoded key is compared to the authored key. A key with `/` is never silently stored as a legacy
`project:///` identity.

**What "project root" means.** The root stored in the project record (the directory that holds
`.awr/project.toml`), taken through `canonicalize()` at read time. If the root has moved, the
read fails with `project root moved since registration` instead of guessing a new location.
An identity-bearing locator whose key is missing or does not match the authored key fails closed
(`InvalidInput` / `SourceConflict`): the file is not opened, the locator is not rewritten, and
there is no path-only fallback.

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

Both `project://` forms fingerprint content as `sha256(bytes)`. Authored `external_key` distinguishes projects even when paths and bytes match.

### 验证入口

```sh
cargo test -p awr-workspace --locked
cargo test -p awr-cli --test workspace_exchange --locked
```

单元测试覆盖签名（AWS 官方向量）、键布局、配置、凭据、对象原语后端、manifest/冲突/CAS 语义、
真实的 `publish --dry-run`、两步 `drop`，以及"连续三次提交都被对端抢先"（`WorkspaceContended`）
的报告与重试自愈；
`workspace_exchange` 用两个项目根加一个目录存储，跑真实的 CLI、配置与提交路径，
不需要网络和凭据。真实对象存储的双机联调按 `examples/workspace/README.md` 的步骤执行。

## Limits

- 每台机器保留自己的文件系统；交换由 publish 与 sync 显式完成。
- 运行数据库、凭据和 workspace 状态属于本机 `.awr/`，保留在各自机器。
- drop 只删除索引/指针镜像引用，本地文件与内容对象保留；仍被 track 覆盖的路径须先移出 track。
- 缺少配置时入口保持零请求、零输出；自动取回仅 SessionStart 触发，PostCompact 沿用当前会话。
- 入口执行取回和原字节修复；新的本地修改须显式 publish，避免把半成品登记为基准。
- 取回故障保留 hook 退出码；冲突保留本地字节，来源解析错误仍由 AWR 守卫处理。
- 10 秒后停止后续本地写入和状态保存；已写入文件保留，下次 sync 接续。
- 此层仅为个人字节交换；身份、权限、任务调度和语义合并属于内核/宿主职责，单桶 CAS 不是多人协作基础。
- 每台机器独立的事件历史会分叉；多人共用 ledger 应使用内核协作与统一回执。
- Windows 凭据权限依赖 `.awr/` NTFS ACL；mode 600 检查仅适用于 Unix。
- 凭据通过受保护文件或环境输入；status 只报告字段存在性与权限，保留秘密值。
- Manifest 单对象提交保证整批可见性；派生指针镜像失败只记诊断，不改变发布结果。
- OSS guarded 适合个人顺序使用；HEAD 与 PUT 间仍有竞争窗口，后写可覆盖条目且读回自己的 ETag。
- 可能同时发布时采用支持 If-Match 的 CAS 存储；guarded 的写前检查和读回校验不提供多写者互斥。
- 冲突保留本地字节并交给人调解；dry-run 同时保留本机与存储状态。
- contended 是提交报告；sync 只取回，索引提交竞争与需要人工调解的文件冲突分开处理。
- track 拒绝绝对路径、`..`、反斜杠、盘符和空路径段；远端索引按外部输入校验。
- 一次发布保证整批可见性；跨机器同时改同文件仍按冲突处理，三次提交竞争按 contended 处理。
- track 覆盖目录全部文件（含点文件）；出现 `.git` 或 `.awr` 目录即拒绝，应收窄跟踪范围。
- 本机状态库的 active_claim 唯一索引和 revision CAS 要求单机单写者；跨机仅交换源字节、证据、handoff。
- Workspace 原始字节由操作者审查；秘密策略作用于 AWR 输入/来源/事件，跟踪源须排除凭据。
- local 后端仅有进程内锁，适用测试和单写者；多写者使用对象存储。
- 两端使用同版本。旧指针版可使 manifest 陈旧；verify-index 检测漂移，删除 manifest.json 可退回指针布局。
- 历史实测将竞争窗口收窄到一个请求；剩余窗口仍按可观测和重新发布恢复处理。
- 这些测量刻意对齐提交时刻，描述压力下的观测上界；日常提交分布需单独测量。
- 这组报告属于所列构建与存储现场的历史观测；文件冲突仍需人决定，提交竞争可重发恢复。
- Project locators identify projects rather than individual checkouts; two checkouts sharing one project key retain the same identity.

## Code and tests

- [Workspace crate](../../../crates/awr-workspace/src/lib.rs)
- [CLI exchange tests](../../../crates/awr-workspace/tests/pr29_boundaries.rs)
- [Cross-machine example](../../../examples/workspace/README.md)
