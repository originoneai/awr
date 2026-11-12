# Unreleased

Personal CLI/MCP changes shipped in 0.7.0 are listed in [0.7.0](0.7.0.md).
The Team/Observer changes and standalone library work below remain outside that
packaged release. Earlier changes are listed in [0.6.0](0.6.0.md).

## Changed

- **An intake scan names every file with a credential finding.** The scan of `awr init`,
  `--write-draft` and `awr takeover` used to end at the first file the secret boundary refused, so a
  project with findings in several documents was fixed one run at a time (a public project with
  example credentials in its docs needed three rounds). The stop now lists every such file in
  `details.findings` (category and location, 50 at most, never the content) with the exact count in
  `details.finding_total`; the first finding keeps the existing fields, and one finding is reported
  as before. See [secret boundaries](../reference/secret-boundaries.md) (#627).

- **Local project registration is explicit.** Ordinary `awr` and `awr-mcp` calls no longer add the
  project they touch to the per-user local catalog; they only refresh the last-use time of a
  project that is already registered. Register a project with `awr overview register <dir>` or
  the hub's **Take over project**. `AWR_LOCAL_AUTO_REGISTER=1` restores the 0.7.0 behavior of
  registering on use. A machine that runs fixtures and scratch projects (including AWR's own
  test suite, which did not isolate its state directory) no longer fills the catalog with them.
  Entries registered by earlier versions stay in the catalog until they are removed. See
  [Local Project Hub](../reference/local-project-hub.md#register-projects).

## Fixed

- **`awr init --manifest` works with `--write-draft`.** A project with a credential in one file could
  not use the draft and review flow: the flag pair was refused, and the draft read every document of
  the project, so the credential stopped it even when the owner had listed only readable sources. The
  draft now maps exactly the manifest's sources, reads only those documents, and lists every other
  file by path and size, so a change to any file still asks for a new review. The draft records the
  readable paths in `inventory_scope`; drafts without a manifest are unchanged. A manifest with
  `--write-draft` never replaces an existing mapping. See
  [project intake](../TAKEOVER.md) (#622).

## Added

- **`awr takeover`**, an unattended and resumable takeover of an existing project. It runs
  preflight, `init` preview and draft, an optional reviewer command, `init --from-draft --accept`,
  `intake inspect` and a receipt in saved steps, so a run that stops continues where it stopped.
  The project's own files stay byte-identical; only `.awr/` and the `.gitignore` entries `init`
  lists are written, and nothing is committed. A reviewer cannot raise a goal above `candidate`
  or leave the ledger without work items, a secret source stops the run naming its category, path
  and line, and a stop exits 1 with the step and reason. The orchestration moved here from the
  DeepSeek Harness plugin, where it ran against real projects with a real model. See
  [Take over a project](../reference/takeover-command.md) (#620).

- **`awr-mcp --catalog`**, a local catalog mode for the stdio server. One process serves the
  projects registered in the per-user local catalog; each call may name one (catalog key,
  unique name or root path) and otherwise uses the default project. The catalog is read on every
  call, so a project registered while the server runs needs no restart. `awr_projects_list`
  accepts `query` and `limit`. `--allow-root` limits which projects are served, only registered
  projects are served, and each project's recorded identity is checked on every call. Per project
  the behavior is that of plain stdio. See the
  [awr-mcp README](../../../crates/awr-mcp/README.md#local-catalog-mode).

- Optional identifiers and check bindings for work-definition examples, advertised as
  `definition.example_checks`. Definition reports show each example's check binding, validate
  argument and path bounds, and identify missing or duplicate identifiers. Check metadata is
  included in admission fingerprints. All readers must support the new keys before a shared
  ledger uses them; this feature does not run commands or read the listed files. See
  [work definitions](../reference/work-definition.md).
- The generic Team read adapter (`POST /api/team/read`) now serves the real `work.next`
  response. Upstream's identity-scoped navigation read carries only
  `{ data, project_revision, protocol_version }` — no scope/snapshot attribution
  envelope — so the OBS-002 envelope check rejected every real `work.next` as
  `BadGateway`. `work.next` is now treated as what it is: the response must still be
  well formed (protocol version, object `data`, valid `next_cursor`) and is still
  refused if it claims `execution_authorized`, but no envelope is required or
  fabricated for it. Its freshness planes say the truth: `authoritative` is
  `not_measured` (a bare `project_revision` is not a comparable measurement and never
  borrows another clock), `page_read` is `measured`, and `report_received` follows the
  usual contract. Envelope-carrying reads are unchanged.
- CLI recovery guidance now includes executable next commands for creation revision
  conflicts, draft claims and context budget errors. Work creation accepts source
  fields directly and can attempt activation in its acceptance call; `work activate`
  provides a reviewed shortcut for existing drafts. Request-key aliases and JSON
  shape diagnostics cover creation and batch recovery. Omitted context budgets
  grow to required facts up to 10000 tokens; explicit budgets remain strict.
  Creation with unchanged sources and an advanced project revision now reports
  `RevisionConflict` with `preview_still_valid=true` instead of `SourceStale`.
  Long recovery commands retain the exact input in a private runtime file, and
  revision conflicts expose a machine-readable retry revision. Draft activation
  refusals offer reviewed field repair with caller-supplied values; acceptance,
  goals and concrete next steps are still required before activation.

- Native CLI regression budgets cover work creation, checked completion, hook
  continuation and automatic recovery. Failures list each step's output bytes;
  the workflow benchmark adds checked completion to both strategies, bringing
  each native platform run to twelve workflows. See the
  [workflow measurements](../benchmarks/workflow.md#golden-path-budgets).
- A compact Team MCP tool catalog for clients whose model provider caps the size of a tool schema
  (AWR-TF-014AN). Moonshot's API refuses `awr_team_command` because its full input schema is about
  130 KB (`schema exceeds maximum allowed size`), which stopped every Kimi Code turn against a Team
  MCP project. An MCP URL with `?catalog=compact` lists the same tools in the same order without the
  per-operation `allOf` branches of their input schemas (about 43 KB for the command tool) and says
  so in the tool description. `properties.op.enum` still names every operation,
  `awr_team_input_contract` still reads the exact input of one operation from the full catalog, and
  every call is still checked against the full schema. The default and `?catalog=full` are
  unchanged; any other value, or the parameter given twice, is refused when listing.
- A [documentation style guide](../STYLE.md) and CI checker for negative-term density,
  opening paragraphs, limitation sections, and relative links. The baseline preserves
  existing pages' four recorded metrics; updates accept equal or lower values and new
  pages use absolute limits. Each page can use one limitation section; larger recorded
  counts remain the ceiling. Run `python3 scripts/check_docs_style.py --self-test`
  for synthetic rule checks and mutation tests.
- Project-owned mutation report commands for `work check --phase mutation`, with bounded
  JSON reports, five status counts, surviving source locations, and commit-bound completion
  checks. Alternative discrimination methods require a stated reason and approval from a
  session other than the proposer. See [example checks](../reference/checks.md#mutation-and-alternatives).
- Compact CLI work-creation previews show the draft facts and an executable acceptance
  command with the reviewed fingerprint and revision. Default JSON replaces source
  snapshots and full texts with count and byte summaries; `--json --full` preserves
  the complete preview payload. Long commands retain their exact input in a private
  runtime file. Runtime fingerprints, acceptance checks and MCP payloads are unchanged.
  See [creation previews](../reference/source-changes.md#cli-creation-previews).
- One observation freshness and coverage contract for the Observer Team reads (AWR-OBS-002): every
  read (`POST /api/team/read`, `/api/team/inbox-detail`, `GET /api/team/inbox`, `GET /api/team/work`)
  carries a `freshness` envelope that presents the report-received clock, the authoritative
  measurement (project revision, source snapshot, coordinator epoch, query time) and the page-read
  clock side by side. Each plane is exactly `measured`, `stale`, `failed` or `not_measured`, and one
  state never substitutes for another: an unmeasured plane says so instead of borrowing another
  clock's value, a stale report-received plane is decided by the service's own declaration
  (`progress.stale`, or the age against the service-declared `reporting.stale_after_ms`), a stale
  authoritative plane is always decided against the bridge's own declared bound — never the
  report's — with `stale_after_ms` and `age_ms` reported next to every measurement, and a failed
  read keeps its clocks with the planes marked `failed`. The bridge's default bounds
  (`OBSERVATION_STALE_AFTER_MS`, `READ_TIMEOUT_MS`, `OVERVIEW_DEADLINE_MS`) are exported constants
  of the contract module. The pure ordering rule lives in `tools/observer/server/observation-contract.js` (importable by
  the bridge and the browser): an incoming authoritative measurement replaces the held one only for
  the same source snapshot and coordinator epoch with a numerically higher or equal revision; a
  lower revision never replaces; anything else is incomparable and says so. The generic read route
  and the coordination detail route share one attribution semantics: the response must belong to
  the requested work, workstream and scope, a read response that claims `execution_authorized` is
  refused, and a single-object read whose query names an identity must echo it (`READ_ECHOES`) — a
  response that omits the echo is refused rather than unattributable. `GET /api/team/overview`
  composes with a declared `coverage` (authorized scope enumeration, per-stream succeeded/failed,
  enumerated versus server-reported totals, truncation with a resume cursor, per-part as-of and the
  bridge's own `read_at_unix_ms`): a capped enumeration is declared truncated instead of counted as
  all, a non-auth part failure retains the confirmed pages with their as-of and resume cursor, an
  auth/permission revocation clears the whole response, a mixed-revision composition claims no
  common version, and the whole composition answers within one deadline with whatever is left
  unread declared failed as `ReadTimeout` rather than as empty data.
- Grouped CLI help: `awr --help` and `awr work --help` show daily commands and
  point to purpose-specific groups. Use `awr help all` or `awr work help all`
  for the original complete lists. The `environments` and `collaboration` groups
  keep command help topics distinct; detailed help, including `awr help work`,
  retains its original format. Built-in help topics also preserve the invoked
  executable name. The work planning group includes `activate` alongside
  `create` and `edit`.
- `awr verify` checks completed work for a Git commit using local observations or
  database-free CI reruns. Baseline comparison checks fail-before/pass-after
  behavior in a temporary worktree; `verify install-hook` previews and installs a
  pre-push gate while preserving existing hooks. Includes `verify.commit`, an
  optional required-work policy, and Git/CI examples. See
  [commit verification](../reference/verify.md).
- Team evidence keeps the caller's claim and the recorded outcome of an execution apart. `evidence.submit` compares a
  claim bound to an execution with the Team's record of it (same work, scope, contract and input digest; a claim of
  success against a recorded failure or cancellation, or the reverse) and refuses a provable contradiction as
  `EvidenceInvalid` before anything is stored. In its response `claimed_success` is what the caller said,
  `execution_success` is now the recorded outcome and null until the execution is terminal (it used to repeat the
  claim), and `execution_consistency` says `consistent`, `unverifiable` or `contradicted` and why, always with
  `independent_verification: false`. `evidence.inspect` and the review provenance recompute the same answer when
  they are read, and only after the work binding, the replay of the evidence digest, the current review contract and
  the current work binding of the execution; otherwise they say `unverifiable` with a reason and show no execution
  fact. Completion and integration keep their strict result binding. No schema change.
- `awr work check` runs declared example checks directly, records oracle and log digests,
  and generates registered reports and completion input after passing acceptance checks.
  Baselines can run before admission; governed acceptance and mutation runs require admission
  and the session's claim. Timeouts are recorded as unknown and their child processes are
  terminated. Unchecked acceptance criteria remain unmapped. Evidence registration reports
  the shared trust vocabulary in CLI and MCP receipts and reads: generated evidence starts as
  `recorded` and becomes `verified` after completion checks its report bytes.
  See [running checks](../reference/checks.md).
- Governed work in a Team: a snapshot may declare governance (`governance.json`, written by
  `PublishPrepOptions::governance`), the Team reuses the Core admission rules on its own facts, and
  `admission.decide` records decisions attributed to the verified caller (`planning.approve`, a
  different person for independent review). Claim, dispatch and execution prepare and start answer
  `AdmissionRequired` until the decision is current; a package without governance cannot replace a
  governed snapshot. A review round records the governed scope it opened under, so a change of the
  definition, the examples or the policy of that work needs a new review and touches no other work.
  Review decisions take findings whose severity is derived from the record: for governed work a
  return needs a blocking finding, an approval resolves the open ones, a re-review covers the open
  blocking findings and the delta, and a new non-blocking finding in a later round is deferred
  instead of reopening it. New schema version 58 (`snapshot_governance`, `work_governance`,
  append-only `work_admissions`, `review_rounds.governance_binding`, append-only `review_findings`).
  See [Team governance](../reference/team-governance.md).
- Local hook dialects for Claude Code, Codex, Kimi and generic hosts. Claude
  session starts receive AWR context, including after compaction; unbound
  conversations receive the action view and a binding command. Optional project
  write gates ask or deny when a conversation lacks a valid claim, or ask about
  writes outside its declared work paths. The write check reads existing state
  without refreshing sources; gate evaluation failures continue with a diagnostic,
  while existing lifecycle errors retain their behavior.
  Claude protocol coverage uses simulated events, without a named-version or
  native-client verification. See [hook decisions](../reference/host-contract.md#hook-decisions).
- MCP `awr_work_check` runs bound example checks through the shared CLI runtime,
  records outcomes, and returns generated completion input. It is a second-level
  write tool under `awr_work`; the first-level catalog remains eight domains.
  Work inspection and preparation expose policy-scoped check obligations, completion
  accepts a small-work check waiver, and `mcp.work_check` supports capability negotiation.
- Executable shell blocks in the [quickstart](../../series/quickstart.md) and
  [daily workflow](../../series/daily-workflow.md) run against the built CLI in
  disposable projects during Linux x64 verification. Named `doc-test` markers
  support output substring assertions, file-and-line failures and a shared
  60-second deadline.
- The contract of a context packing kernel with no store (`awr-context-kernel`, a library crate, not in the binaries): a request of fixed
  text chunks with stable names, entities and revisions, required and ranked optional chunks, atomic groups and a token or byte budget;
  a result that is a consistent partition of it; `validate_request` and `check_result` that say why a request or a result is refused, with
  a corpus of 69 offline cases. Mandatory content is never dropped to fit, unsupported input fails by name, and nothing a result carries
  says anything of authority, readiness or acceptance. The packing itself moves out of `awr-context` in a later step. See
  [context packing kernel](../reference/context-component.md).
- Compact text rows for the default `status` action view and `ready`: each shows
  the work key, a next action capped at 90 characters, and its source marker with
  a relative age. A newer checkpoint supplies the displayed action; the footer
  explains each used marker once through the shared trust vocabulary. Full and
  summary status views and JSON retain their existing output. See the
  [daily workflow](../../series/daily-workflow.md).
- Definition guidance in governed-work activation refusals and `work show`, pointing
  to a short YAML starter in `awr work help governance`. Unclassified rule diagnostics
  explain the heading annotation format. Admission and completion checks retain
  their existing requirements; daily help and workflow output budgets stay unchanged.
- The capabilities of the Team workstream protocol say, for each of the five result layers (work readiness,
  execution admission, context completeness, delivery observation, completion validity), which Team operations carry
  it and which entries of the personal CLI and MCP server have no Team operation, with the reason; the report is checked
  against the protocol's own operation lists. Asking for such an entry is still refused as `Unsupported`.
- Source commit identity in `awr --version` and `awr-mcp --version`, with a short
  hexadecimal hash and a `-dirty` marker for Git working tree changes. Source
  archives can inject `AWR_BUILD_SHA`; builds without commit information keep
  the package version alone. See [version output](../reference/cli-mcp-contract.md).
- Trust metadata in progress, session-label, evidence and admission JSON, with
  the negotiable `output.trust_field` capability. Existing fields and entry text
  remain available. Caller-supplied evidence types stay `reported` until a
  completion byte check establishes `verified`; a type name never grants
  `recorded`. See [Trust fields](../reference/trust.md).
- A bounded, read-only observation entry in the Observer Team bridge (`POST /api/team/read`): every implemented Team
  query is reachable except the deferred full ops audit (`audit.history`, `audit.export`, `audit.count`); commands are
  never exposed, an arbitrary command or a full audit export cannot be requested. Each operation's selectors, paging and
  byte budgets are checked against a hand mirror of the service's `WorkstreamQuery` rules — guarded by a drift test that
  reads the service's query list — selector values are never echoed in errors, per-operation response limits apply
  (`artifact.segment` pages stay at 64 KiB), and unsupported operations, denials, timeouts, partial pages and stale
  cursors come back as explainable errors. The bridge adds no identity and grants no authority: the upstream service
  reauthorizes the caller's own cookie session on every forwarded read. The `awr_team_command` MCP description now
  matches the fixed delivery capability it declares.

## Changed

- Project-scoped Claude Code and Codex lifecycle installation, removal and status.
  Optional preview fingerprints reject changed settings; managed entries are replaced
  while preserving parsed user configuration. Shared Claude settings use portable
  commands. Status and doctor share adapter inspection and report receiver observations.
  Claude coverage is **simulated events verified**; native verification is
  **unexecuted, awaiting approval**. See [Claude Code hooks](../integrations/claude-code.md).
- Follow-ups of the early digest check: `delivery.integration.prepare` now refuses evidence whose declared output digest or
  stored artifact digest differs from the settled execution's reported output with the same 409 `EvidenceInvalid` reasons,
  digest prefixes and repair as `review.open` (stage `prepare`), instead of the generic refusal; the refusal at an approving
  `review.decide` is written for the reviewer (return the round); a present but malformed declared digest is shown as
  `invalid` instead of `none`; the input contracts of `evidence.submit`, `delivery.evidence.submit`, `review.open` and
  `delivery.submit_and_request_review` state the rule; and such a refusal is audited as `failed` instead of `unknown`.
  An already prepared integration whose evidence no longer binds is refused at its lease, dispatch or recheck with guidance
  of its own (reject the prepared intent while it is prepared or leased; inspect it once it is dispatched), and the refusal at
  `review.decide` names `/args/round_id`. No schema change.
- Admission freezes required examples' check commands and file hashes in `oracle_fingerprint`.
  Changed or unreadable check files make a frozen approval stale. The effective
  `fail_before_pass_after` method requires a failed baseline for each required check-bound
  example before governed approval; observation mode reports gaps. Per-example baselines remain
  current when another example is added, and legacy bindings retain their original comparison rules.
- Quickstarts now preview and install project-local lifecycle hooks, then accept the
  reviewed fingerprint. MCP instructions and the short manual-host fallback share
  the core working agreement. These commands target current source builds; see
  [connecting an agent](../../series/agents.md#hosts-without-lifecycle-hooks).
- Baseline checks distinguish behavior failures, checks that cannot run, and unclassified
  failures. Governed admission and completion require a recognized behavior failure;
  receipts show the classification and recovery guidance. Older baselines retain their
  original semantics and display `legacy`. See [baseline classifications](../reference/checks.md#classify-baseline-failures).
- Evidence for a workspace-settled execution is checked against the execution's reported result when a review
  round is bound to it, not only at completion and integration. `review.open` and an approving `review.decide`
  refuse it with HTTP 409 `EvidenceInvalid` when the `payload.output_digest` it declares or the sha256 of its stored
  artifact differs from the output the execution reported; the response lists every failing reason, shows 12-character
  digest prefixes and names the repair (new evidence with matching digests, or a new execution that reports the
  stored artifact's hash). `evidence.submit` still accepts such evidence (one execution can have auxiliary evidence)
  and reports the relation in `execution_consistency.result_binding`. Works whose completion does not verify an exact
  result, and executions that are not workspace-settled, are unchanged. No schema change.
- Purpose-first guides and eight Personal reference pages now collect assurance boundaries in
  a single Limits section. MCP descriptions state the action and result; empty domain discovery
  returns a `limits` array with the detailed constraints. Catalog tests cap descriptions at
  200 bytes and the default stdio catalog at 6144 bytes. Incomplete-context guidance points to
  gaps and an incomplete checkpoint; execution and completion rules stay the same. Documentation
  baseline updates lower improved metrics and retain recorded ceilings for drifted metrics;
  new-page limits and relative links are checked before writing.
- Local commit verification can reuse acceptance observations across commits that change
  only configured ledger source files. This applies to explicit `--work` selection or an
  empty selection that requires associated work; ordinary default selection keeps its
  existing behavior. Per-work receipts identify the bound commit and changed paths while
  preserving the shared completion gates. See [commit verification](../reference/verify.md#ledger-only-local-observations).
- A work that delivers a selected candidate into a repository (a connector is configured for it, a
  candidate is selected and its completion policy can be integrated) can no longer be completed before
  that candidate's integration is confirmed. `delivery.finalize` and `work.complete` (and the library
  `ReviewStore::complete`) are refused with HTTP 409 `IntegrationNotConfirmed` unless an integration of
  the selected candidate is `confirmed` for the current binding, and the `reason` names the missing
  step: `integration_not_prepared`, `integration_prepared`, `integration_dispatched`,
  `integration_unknown`, `integration_rejected`, `integration_target_changed` (rebuild on the new
  target) or `integration_confirmation_not_current`. `work.prepare` and `work.inbox` show an
  `integration` step instead of `finalization` until then. All other works behave as before,
  completion receipts that already exist are not changed, and an integration confirmation
  still never creates a completion receipt by itself. See
  [delivery integration](../integrations/delivery-integration.md#finalize-only-after-the-integration-is-confirmed).

## Fixed

- A command that needs a live claim now says what to do when the claim is no longer live.
  `claim.renew`, `execution.prepare`, `execution.start` and `delivery.candidate.select` still
  refuse it with HTTP 409 `LeaseExpired` and the message `lease expired`, and the body now adds
  `reason: "claim_lease_expired"`, a `next_step` and `action_guidance`: an expired claim cannot be
  renewed; read `session.inspect` and `work.prepare`, settle or cancel an unfinished execution,
  call `claim.acquire` with the current `expected_session_version` and `expected_work_version`,
  and continue only with the new claim. The text is fixed, repeats no value of the request and is
  identical over HTTP, the Web entry and MCP. Before, the body held only the code and the message,
  and a member whose claim had expired needed two failed attempts to find `claim.acquire`.
  `LeaseExpired` from any other command (the source publication commands, the delivery workers, a
  settlement that races with expiry in `execution.report`) and every other refusal are unchanged
  (AWR-TFQ-024).
- `awr_team_planning_draft` in `create` mode with a `candidate_id` that already exists no longer
  answers `resumed: true` for a different request. Only a retry of the creation that candidate
  recorded (same changes, suggestion IDs, spec roots, goal keys, self-approve policy and author)
  resumes; any other request is refused before any command is reserved as `InvalidInput` with the
  reason `candidate_exists_with_different_request`, `differs_in`, the existing candidate's
  `candidate_id`, `draft_revision`, `candidate_digest` and `state`, and a next step (use `edit` with
  that `candidate_id`, or `create` with a new one or none). Before, a supervisor that reused a
  candidate ID with new changes was told the draft was created while it still held the old changes
  (AWR-TFQ-023).
- A count of zero under a credential-like label is public content. A diagnostic record such as
  `app-password: 0` or `{"manager-token":0}` says that a label never appeared, so the source
  screen accepts the integer zero as it accepts `false` and `null`. Every other number, a string
  that spells zero, `00`, `0123`, `0.5` and a zero followed by other text remain values. Before,
  one such record in a shared source made every write to that source fail with `labelled_value`,
  including writes about unrelated work. The secret policy is now 11 and the search index policy 13,
  so existing caches rebuild on first read.

- A `succeeded`, `failed` or `cancelled` `execution.report` for a caller-managed run admitted
  under a stored workspace settlement policy must now state `executor_stopped` and
  `no_external_effects` in `workspace_settlement` as `true` or `false`. A report that omits
  `workspace_settlement`, omits either declaration or leaves it `null` (the placeholder of
  `terminal_reporting`) is refused with HTTP 400 `InvalidInput`, reason
  `settlement_declaration_incomplete`, `invalid_field` naming the declaration and
  `request_reserved: false`. Nothing is recorded, so the same `request_id` works with both
  declarations. Before, such a report was committed as unsettled: the execution became `unknown`,
  the task's recovery was blocked and its reservations were marked unknown, and only a
  `project_admin` reconcile could clear it, so the member's run stopped. Outcome `unknown`, explicit
  `false` declarations, other settlement mismatches and runs without a stored policy are unchanged
  (AWR-TFQ-022).
- A work that was integrated, completed and written back to the source is no longer shown as stale
  after a later plan activation that left its contract alone. Before, every activation made the
  project snapshot differ from the one recorded with the selection and the source publication, so
  `work.observe` reported the delivered work's candidate, review round and integration binding as
  not current, `delivery.source.status` reported `source_synchronized` false,
  `delivery.neutral.inspect` reported its selection and facts as not current, and the Observer
  said the earlier integration record does not apply to the current result, although the delivered
  bytes and the contract had not changed and nothing could repair it. A completed work now stays
  current while its contract hash is the same in the current snapshot, and turns stale when an
  activation changes it. The reads of a work that is not completed, integration preparation,
  dispatch, the reservation of a new inspection, `delivery.schedule` and
  `delivery.submission.describe` are unchanged (AWR-TFQ-021).
- `awr_team_planning_draft` in `edit` mode without a `candidate_id`, or whose mode is neither
  `create` nor `edit`, is now refused before any command is reserved or candidate created:
  `InvalidInput` at `/candidate_id` (constraint `required_when_editing_a_draft`) or at `/mode`
  (`create_or_edit`), with the next step. `edit` changes an existing planning candidate by the
  `candidate_id` of your earlier `create`; to start a draft, including a change to existing work,
  use `create` with the changes (each one carries the exact prior definition in `before`).
  The tool description and schema say what `mode` means. Before, such a request was answered with
  the generic "query fields or bounds are invalid" text, and a supervisor that picked `edit` for
  existing work could not recover (AWR-TFQ-020).
- `review.inspect` of an open round now ends its reading guidance with the reviewer's next step:
  read what the delivery service observed in `delivery.neutral.inspect`, then record the decision
  with `review.decide` (approve or reject, with a reason that states what was verified and what
  could not be). A source revision that the reviewer cannot verify (no source locator disclosed)
  is a limit to state in the decision, not a reason to withhold it; approval still integrates and
  delivers nothing. For a candidate without a source revision the note says that
  `delivery.integration.prepare` refuses it. Before, the reviewer was told only that the revision
  needs an authorized-channel check "else version unverified", and a careful reviewer agent read
  the review as blocked and made no decision. The neutral inspection and decided rounds are
  unchanged (AWR-TFQ-019).
- Project completion checks can observe or enforce source-bound acceptance runs and failed baselines through `.awr/gates.toml`. Preparation lists each check command, small work can record a waiver, and Stop reminders use the same completion gate. See [completion check gates](../reference/gates.md).
- Governance help maps every implementation-definition gap code to its field path,
  a one-line YAML value shape and a short repair hint, including registered decision
  front matter, hashing and adopted intent roles. Every acceptance criterion needs
  a positive and a negative example. The reference fits within 3,584 bytes. See
  [work definitions](../reference/work-definition.md).
- The next-action guidance after a repository target moved no longer tells the developer to
  update against the current target, or to rebuild, "in a new admitted execution"
  (AWR-TF-014AN). It appeared in the integration inspection of a rejected integration, in the work
  inbox of the developer who owns the rejected work and in the `IntegrationNotConfirmed` refusal
  for `integration_target_changed`. An admitted execution scope checks every observed workspace
  effect, imported baseline paths included, so that update failed with a scope violation whenever
  the new target touched files outside the work's scope. The guidance now says to update the
  workspace as a separate baseline step before the next coding execution begins, then retest in a
  new admitted execution and publish a new candidate; all three places build the text from one
  module. Wording only: no rule, schema or protocol changes.
