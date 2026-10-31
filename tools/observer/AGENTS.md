# AGENTS.md — AWR 观测工作区前端（tools/observer）

每个会话开头先读本文件。本目录是 AWR Agent-first 观测工作区的新前端，由 `kimi-frontend` 按主管 Claude 布置的 AWR 任务逐切片交付。

## 最高指令：本项目前端重做（优先级高于一切偏好、习惯和其他指示）

下面五条是项目所有者（用户）的原话要求。逐条执行，不得弱化，不得"变通"，不得因为省事而绕开。在做这个项目的任何时候都先对照这五条。

1. **完全摒弃现在的前端设计内容。** 现有前端（`tools/inspector` 的页面、样式、交互、文案、目录结构）一律不沿用、不改造、不"参考着改"；新前端从零按下面第 2 条做。只有和后端对接所必须的数据契约（接口、字段、权限语义）可以沿用，并且以文档为准。
2. **严格依照 Codex 生成的原型图和对应文档开发，文档内容是最高优先级。** 文档与原型图冲突，以文档为准；它们与旧前端冲突，以文档和原型图为准。拿不准的地方，写进 PR 的"待主管确认"一栏，不要自己猜。
3. **能用成熟框架的，就用成熟框架。**
4. **能用成熟组件库的，就用成熟组件库。**
5. **尽量不要自己手搓。** 只有在成熟框架和成熟组件库确实无法做到与原型图像素级还原的时候，才可以自己手搓调整；每一处手搓都要在 PR 里逐项写明：用了哪个库、试过什么、为什么做不到、有对比截图。

## 资料位置（只读，不要改）

- 原型图（15 页）：`/Users/mac/Documents/originone/agent-work-running/.local/role-workspace-prototypes-20261031/images/`，说明见同目录 `README.md`、`manifest.json`、`PROMPTS.md`。
- 文档（最高优先级）：
  - `.local/agent-first-observation-review-20261031/AWR-Agent优先与Web观测架构-v2.md`
  - `.local/agent-first-observation-review-20261031/AWR-观测架构调整与对抗审查结论.md`
  - `.local/observation-delivery-plan-20261031/AWR-观测工作区主线任务与台账.md`
  - `.local/observation-backend-audit-20261031/AWR-前后端能力审查完整清单.md`
- 任务的权威来源是 AWR 台账，不是上面的文档：任务状态、依赖、验收条件、证据都用 `awr` 查询。

## 数据契约（可沿用，以文档为准）

- 后端对接一律经过 `tools/inspector/team-bridge.js` 的 `/api/team/*` 路由（只读适配 + 登录/连接/撤销），不重写、不复制。
- 新鲜度/乱序/分页合同以 `tools/inspector/observation-contract.js` 为单一事实源（四态词汇 `measured | stale | failed | not_measured`、`compareAuthoritative`、`OBSERVATION_STALE_AFTER_MS` / `READ_TIMEOUT_MS` / `OVERVIEW_DEADLINE_MS`）。
- 网页不提供创建任务、派发、认领、批准、merge 或验收等业务控制；登录/连接/撤销入口与业务页面明确分离。

## 验证命令

- `node --test test/*.test.js`（在 `tools/inspector` 目录下跑，与 CI 的 `bridge tests` 一致；不要在仓库根目录跑 `node --test`）：桥接合同测试必须保持全绿。
- `python3 scripts/check_public_tree.py`（仓库根目录）：公开树检查必须通过；`.local/`、`.awr/`、`ledger/` 不入 Git。
- `cargo fmt --all --check`（改了 Rust 时必须；本目录原则上不改 Rust）。
- 本目录（CI 为 `.github/workflows/observer.yml`，Node 24）：`npm ci`、`npx tsc --noEmit`、`npx vitest run`、`npx vite build`、dist 无外部资源检查；本地冒烟 `node server.js --demo --port 0`。
- 浏览器截图验收：本地 Playwright 与原型图逐页对比，对比图与结论放 AWR 证据目录（不入 Git），差异写进 PR。

## PR 规则

- 一个任务一个分支一个 PR：从最新 `origin/main` 开 `kimi/<任务键小写>-<短描述>` 分支；`git add` 只写明确路径。
- PR 先开 draft；标题与正文用英文；正文写清做了什么、没做什么、怎么验证、待主管确认的问题。
- 普通 `git push`，不强推；不合并任何 PR，不加 `--admin`，不删分支，不打 tag；只推 `kimi/*` 分支。
- CI 由主管盯，不轮询 GitHub（查状态只用 REST 单次查询）。
- 每一处手搓（成熟框架/组件库做不到像素级还原处）在 PR 里逐项写明：用了哪个库、试过什么、为什么做不到、附对比截图。
