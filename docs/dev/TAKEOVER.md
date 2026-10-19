# Project intake and work continuity

This addition covers project intake and organization, client checkpoints, execution registration, and recovery inspection. It extends the existing AWR work runtime; it does not transfer arbitrary process memory or reconstruct unrecorded conversations.

## Existing project vocabularies

Keep the existing ledger's filenames, columns and state words. Init discovers conventional Markdown ledgers including `*-ledger.md`; multiple candidates require choosing the authority. Common columns such as `工作项`, `Owner 角色`, `完成硬门槛`, and `当前证据 / 下一动作` are recognized. Custom columns and YAML work fields can be mapped explicitly:

```sh
awr init --status-map pending=planned --status-map complete=completed
awr init --status-map pending=planned --status-map complete=completed --accept
# If the title column/key is nonstandard, add --field-map title=事项.
awr intake inspect --json
```

These flags appear in the preview and accepted manifest. Status maps interpret source-declared facts, not evidence of completion. Unknown states retain a diagnostic; the Agent can resolve the mapping in `.awr/project.toml` and reindex. Mapped YAML writes retain original keys/status spellings and obey existing claim, revision and completion checks. Markdown remains read-only through AWR; edits continue in the authoritative file. A combined evidence/next-action column supplies its literal text, without inferring completed verification.

Chinese ADR list metadata such as `- 状态：Accepted`, bold labels and Chinese YAML front matter are supported. Conflicting declarations fail. Credential-free JSON/YAML schema and authentication definitions can be indexed alongside the documents; actual values inside defaults/examples or extra fields still trigger the shared boundary. See the [YAML mapping reference](../../adapters/yaml-ledger/README.md), [ADR reference](../../adapters/markdown-directory/README.md), and [data boundary](reference/secret-boundaries.md).

## Intake

Run `awr --project /absolute/project init` to inspect the inventory and proposed source mapping. Nothing is initialized until `--accept` is supplied. Conventional YAML sources are retained; Markdown task tables and checklists can be projected with the read-only `markdown-ledger-v1` adapter. Existing Markdown remains the authority and is edited in its original file, then reindexed.

Only missing goals and task ledgers are proposed under `.awr/intake/`. These are source files, not disposable runtime state. When both are missing, the placeholder goal is written into the proposed YAML ledger with stable id `intake-goal`; its key is that id, not a checkout path. A separate `GOALS.md` is proposed only when an existing ledger cannot hold that goal. Existing goals inside a primary YAML ledger are reused. Separate plan/rule files are optional; existing ones remain authoritative. The first work item has `kind: intake` and organizes the project baseline. Plan headings create planned review tasks; they do not assert that implementation is missing or completed. Existing project files are never overwritten. Authored intake sources participate in the normal writeback paths: an intake YAML ledger accepts reviewed proposals and work actions, while the rest of `.awr/` stays runtime-owned and is never rewritten through source mutations. A file under `.awr/intake/` counts as an authored source only if it is registered in `.awr/project.toml` (directly or inside a registered directory source), after symlinks are resolved. The runtime-written `inventory.json` and `project.toml` there never count, even if a manifest names them. The same rule applies to every source write: proposals, work creation, batch ledger edits, sharded writes and organization field changes. If the manifest cannot be read, AWR treats the path as runtime-owned.

For a blank project, state its purpose with `awr init --goal "Deliver a document portal" --accept`. For a project with complex requirements, save a JSON draft with `awr init --write-draft /outside/project/intake.json`. An agent or owner can use the inventory and original documents to replace the generated work with concrete actions, dependencies and acceptance criteria. Apply the reviewed draft with `awr init --from-draft /outside/project/intake.json --accept`. A changed input inventory requires a new review. Generated paths are limited to the four owned intake source files.

Source-content checks apply to preview, `--write-draft` and accepted initialization. A heading such as `Environment:` alone is not a dump; public measurement prose, versions, commit IDs, sample sizes and units remain usable. Variable-shaped entries can still require review. A blocked `--write-draft` writes a diagnostic-only JSON file without source bodies, while retaining the failing exit code; that file cannot be accepted as an intake draft. Neither the blocked scan nor its diagnostic initializes a database.

For a suspected public value, use `awr intake review --source docs/notes.md --write-draft /outside/project/review.json`. Inspect the exact original file, fill in reviewer identity, review time and one reasoned decision per finding, then run `awr intake review --from-review /outside/project/review.json`. AWR rescans the source before archiving the receipt; retry Init afterwards. Recognizable credentials and private keys cannot be approved. Source edits or detector-policy changes require a new review. See [content review](reference/content-review.md) for the receipt contract, limits and recovery behavior.

The scanner records filenames, sizes, document hashes and observed Git state. It skips hidden/vendor/build directories and symlinks and has explicit resource limits. It does not infer business completion from code filenames or Git commits. Arbitrary spreadsheet/Word/PDF ledgers still require conversion or an explicit adapter; unrecognized states remain unknown.

## Organize and recheck

```sh
awr --project /absolute/project init
awr --project /absolute/project init --accept
awr --project /absolute/project intake inspect --json
# The Coding Agent edits the authoritative sources using the returned actions.
awr --project /absolute/project intake inspect --json
```

Init preview, accepted initialization, `status`, `ready`, and MCP `awr_project_status`/`awr_work_ready` expose the same `organization` report. Its ordered `actions` describe the material to inspect, required fields and completion conditions. `sources` and `goals[].source_ref` identify authority; `goals[].key` is the exact key to use in task links. YAML ledger goals use their record id. Markdown headings include the locator prefix. `gaps` retains source references and task keys. Missing input is not replaced with invented intent. Diagnostics are sampled at 100 entries with exact totals and `truncated`; fix the sample and recheck to reveal remaining findings.

| State | Meaning |
| --- | --- |
| `not_initialized` | No usable project runtime has been established. Preview Init first. |
| `source_unreadable` | Manifest/source/cache verification failed. Inspect the accompanying error or `source_issues`; retained projections cannot authorize readiness. |
| `needs_organization` | Goals, links, work fields or concrete business work are missing/unconfirmed. An empty ledger is not completion. |
| `ready` | At least one task appears in `executable_work` with a source-declared goal, acceptance, next action and resolved prerequisites. Other findings remain open. |
| `blocked` | Work structure is present but dependencies, status or blockers prevent execution. |
| `awaiting_verification` | Source tasks say completed; actual current acceptance reports have not all been verified. |
| `completed` | All non-cancelled ledger work has validated reports for an explicit source SHA; required cancelled scope prevents this state. This is scoped to the ledger and evidence, not release or real-client business acceptance. |
| `closed_without_completion` | Work is cancelled, or required scope was cancelled. It cannot be credited as project completion. |

`ready`/`ready_count` in the existing scheduling API still describe dependency selection. `organization.business_execution_ready` and `executable_work` add the goal/structure check; inspect context and acquire the appropriate session claim before execution. Organization tasks remain claimable even when business execution is not ready. An unrelated candidate goal does not prevent well-defined work from proceeding. When work already links to goals, its context is judged from those links; an unrelated unconfirmed goal stays a project notice and does not by itself mark that work incomplete.

The Coding Agent compares user intent, README, implementation and development history, then records the goal and its provenance. Use `draft`, `candidate` or `needs_confirmation` for uncertain goals; use `active`/`confirmed` when supported by the source material or user instruction. AWR checks the declaration; it does not certify semantic agreement. Ask for business intent only when the available material cannot establish it. Init without `--goal` creates a draft goal; accepting the files does not confirm that goal. A generic intake task never makes business execution ready.

New intake drafts explicitly select `[project] context_profile = "minimal"` in their reviewable manifest. This permits absent rule/milestone sources in L0/L1 for small projects; every configured rule, source failure, acceptance criterion and dependency is still checked. The profile is bound to source configuration revisions and L1 context content. Existing manifests default to `standard` and are preserved; changing their profile is an explicit source-configuration edit. Unresolved goal scope or a ledger containing only completed intake tasks still requires organization.

A small project can keep everything in one `work-ledger.yaml`:

```yaml
goals:
  - id: search
    title: Let readers find documents
    status: active
    summary: The user requested search in the existing portal; see README.md.
    success_criteria: [Readers find a requested document]
work_items:
  - id: search-api
    title: Add document search
    status: ready
    goal: search
    acceptance: [A matching query returns the requested document]
    next_action: Implement the search handler using the current document index
```

Quote ledger values that contain `#`. YAML truncates a plain scalar at ` #` and treats the rest as a comment, so `next_action: Follow PR #322` indexes as `Follow PR` and the tail is unreachable by field edits — which deliberately preserve same-line comments, so the truncated tail survives later rewrites. Write `next_action: "Follow PR #322 review"` (or a block scalar) instead.

For existing Markdown ledgers, add `goal`/`目标`/`关联目标`, `acceptance`/`验收标准`, and `next_action`/`下一步` columns in the original table. A checklist without this information remains a useful source observation and produces organization actions. AWR does not rewrite the Markdown or replace it with a second ledger.

CLI inspection refreshes only the rebuildable projection cache; it does not change goals, task files, claims or execution state. MCP remains read-only: after source edits run `awr source reindex`, then call `awr_project_status` again. A stale MCP snapshot returns a structured error plus repair guidance, never a cached success. Keep both error/exit status and `organization` when integrating an Agent.

The MCP server can bind an uninitialized project directory so `awr_project_status` can return `not_initialized` and intake guidance. Starting it or querying status creates no runtime files; mutations still require a valid initialized project and their normal revision/session checks. Initialization remains an explicit CLI operation.

To verify source-declared completion, use `awr intake inspect --source-sha FULL_SHA --json` or supply `source_sha` to MCP `awr_project_status`. AWR validates registered local completion reports against their hashes, work identity, SHA, command, scope, time and every current acceptance criterion at the required evidence level. It does not run the recorded command. Missing, changed, failing, historical or mismatched reports leave completion unverified. Validation is bounded to 1 MiB per report and 16 MiB per inspection; exhausted budgets remain explicit. `source_completed` and `verified_completed` remain separate counts.

## Client checkpoints

Host integration is layered: L0 is CLI/MCP plus `awr client bind --client generic`;
L1 records a named host; L2 optionally installs native hooks. See
[host integration layers](integrations/README.md).

The L0 binder attaches a host conversation to an active AWR session, or resumes
a predecessor. It does not install hooks:

```sh
awr client bind --client generic --external-session HOST_CONVERSATION_ID \
  --work INTAKE-001 --session AWR_SESSION_ID
```

An optional L2 Codex adapter is installed with:

```sh
awr client install --client codex --work INTAKE-001
awr client install --client codex --work INTAKE-001 --accept
```

It merges `SessionStart`, `PreCompact`, `PostCompact`, `Stop`, `SessionEnd` and `Interrupt` handlers into the project's `.codex/hooks.json`, preserving existing handlers and config. Review and trust the exact definitions in a fresh client's `/hooks` view. Installation always reports `activation_verified: false`; a generated file or a synthetic receiver call is not proof that a native client activated the hooks. The adapter follows the [official lifecycle contract](https://learn.chatgpt.com/docs/hooks). Other hosts stay on L0 until an L2 adapter exists.

The receiver binds a native conversation ID to a work-bound AWR session. SessionStart/PostCompact return recovery context. Stop/PreCompact/SessionEnd/Interrupt save the persisted next action, open loops and actual AWR event/source delta. Shutdown hooks are advisory and do not terminate sessions, release claims or kill processes. Explicit session end/handoff retains those responsibilities.

Record a changed next action before it is lost from the client:

```sh
awr client progress --client codex --external-session CLIENT_ID \
  --next-action "Apply the reviewer corrections" --open-loop "Independent review remains"
awr client show --client codex --external-session CLIENT_ID
```

The adapter does not read transcript bodies or infer facts from assistant prose. A binding command returns the compiled context; its hash identifies that snapshot, not proof that a model consumed it. Duplicate events with unchanged work reuse their checkpoint. A continued turn with changed persisted progress creates a new checkpoint. Process locks release automatically on crashes. A checkpoint completed before a lost client receipt is recovered by its delivery key; incomplete saves are never credited.

For explicit cross-client handoff, bind a new native conversation to a predecessor:

```sh
awr client bind --client generic --external-session NEW_CLIENT_ID \
  --work INTAKE-001 --from-session AWR_PREDECESSOR_ID
```

`--client generic` is the L0 identity. `kimi` is a documented native dialect for the same receiver; automatic L2 installation is currently provided for Codex only. The receiver accepts the documented `session_id`, `cwd`, `hook_event_name`, optional `turn_id` and `model` fields. Native mode emits only documented hook output fields; `--json` adds diagnostic AWR receipts. This does not start a native client or migrate its process memory.

## Executions

```sh
awr execution run --session AWR_SESSION_ID --key build-reviewed-report \
  --purpose "Build the reviewed report" -- report-builder --output report.pdf
awr execution list --work INTAKE-001
awr execution show EXECUTION_ID
awr execution register --session AWR_SESSION_ID --key external-report \
  --purpose "Track the CI report build" --reference 'ci://build/42'
```

Intent is committed to the immutable event journal before dispatch. Each project-wide operation key identifies one exact intent and work/branch binding, even after a session handoff. Repeating it returns that execution; changing its intent is a conflict. A registered operation whose dispatch was interrupted is not automatically retried. This is at-most-once dispatch, not a promise of exactly-once effects across crashes.

Managed commands run through a separate local AWR supervisor with null stdin and their argument vector unchanged. The invoking CLI can exit, and the AWR session can end, while the command continues. Start/end records, exit code or signal, stdout/stderr and an atomic result receipt are retained under `.awr/executions/`, excluded from Git. The supervisor records only its direct child's outcome; a command that backgrounds unrelated descendants does not transfer their lifecycle to AWR. Commands inherit the launch environment; AWR does not persist environment secrets or grant new permissions. On Windows, the launcher clears inheritance on its own standard pipe handles before dispatch so a piped caller can return while the managed command is running; the caller's handles remain open. The implementation uses the documented [handle inheritance flags](https://learn.microsoft.com/en-us/windows/win32/api/handleapi/nf-handleapi-sethandleinformation).

External references remain unverified. Recording a PID, a URL or an asserted success does not produce a managed completion. There is no automatic takeover of arbitrary existing processes or client memory.

## Migration drill and mainline takeover (WS-050)

Before migrating this project's mainline, prove **backup → preview → migrate →
restore** on the independent fixture under
`tests/fixtures/workstreams/migration-takeover/`. Classification rules live in
`awr_core::migration_takeover` and are summarized in
[migration-takeover](reference/migration-takeover.md). Unprovable person→agent
history stays `pending_confirmation`; Agents are never auto-promoted to owner or
independent approver. Goal lines Team / EVO / DEC / AUTO, incomplete work,
active sessions, and historical release evidence must remain.

## Recovery inspection

```sh
awr execution inspect EXECUTION_ID
awr recovery inspect --session AWR_SESSION_ID
awr session resume --from-session AWR_SESSION_ID --agent successor \
  --provider generic --model selected-model --no-claim --expected-revision REVISION
```

`execution inspect` and `recovery inspect` open the runtime database read-only. They never restart commands, kill processes, update sources or create successor sessions. Recovery inspection includes the last successful checkpoint, while `session resume` performs the existing source refresh, revision checks and successor transition.

Each execution observation is `running`, `succeeded`, `failed` or `unknown`, with an observation time and evidence basis. Running requires a response from the registered loopback supervisor matching its execution ID, nonce, supervisor PID and child PID. Success/failure comes from the supervisor's immutable completion event, or its identity-bound atomic result receipt if the final database write was interrupted. A missing supervisor, invalid receipt, absent dispatch or unsupported external executor remains unknown. A numeric PID or a produced file alone is insufficient. Observations are local snapshots; running status can change immediately afterward.

L0 and L1 retain every execution's recorded facts for the selected work and branch, including prior sessions' results. These records participate in deterministic context hashing and mandatory token budgeting. When they do not fit, compilation fails explicitly rather than omitting them. Live observations are separately labeled in resume output and client SessionStart/PostCompact context, because network/time observations are not part of the source-context hash. Automatic inspection has a two-second work budget; remaining records are returned as unknown with a reason and an explicit inspection command. The complete native additional-context output has a 10,000-token ceiling.

Recovery checks use actual owned subprocesses to cover caller/session exit, live continuation, success, failure, a killed fixture supervisor, duplicate keys and a fault-injected final database write. These are local integration checks. Native hook trust/activation, full E4 acceptance, other operating systems and publication of this feature batch remain separate delivery steps.
