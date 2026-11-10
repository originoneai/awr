# Unreleased

Personal CLI/MCP changes shipped in 0.7.0 are listed in [0.7.0](0.7.0.md).
The Team/Observer changes and standalone library work below remain outside that
packaged release. Earlier changes are listed in [0.6.0](0.6.0.md).

## Added

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
- The contract of a context packing kernel with no store (`awr-context-kernel`, a library crate, not in the binaries): a request of fixed
  text chunks with stable names, entities and revisions, required and ranked optional chunks, atomic groups and a token or byte budget;
  a result that is a consistent partition of it; `validate_request` and `check_result` that say why a request or a result is refused, with
  a corpus of 69 offline cases. Mandatory content is never dropped to fit, unsupported input fails by name, and nothing a result carries
  says anything of authority, readiness or acceptance. The packing itself moves out of `awr-context` in a later step. See
  [context packing kernel](../reference/context-component.md).
- The capabilities of the Team workstream protocol say, for each of the five result layers (work readiness,
  execution admission, context completeness, delivery observation, completion validity), which Team operations carry
  it and which entries of the personal CLI and MCP server have no Team operation, with the reason; the report is checked
  against the protocol's own operation lists. Asking for such an entry is still refused as `Unsupported`.
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

- Follow-ups of the early digest check: `delivery.integration.prepare` now refuses evidence whose declared output digest or
  stored artifact digest differs from the settled execution's reported output with the same 409 `EvidenceInvalid` reasons,
  digest prefixes and repair as `review.open` (stage `prepare`), instead of the generic refusal; the refusal at an approving
  `review.decide` is written for the reviewer (return the round); a present but malformed declared digest is shown as
  `invalid` instead of `none`; the input contracts of `evidence.submit`, `delivery.evidence.submit`, `review.open` and
  `delivery.submit_and_request_review` state the rule; and such a refusal is audited as `failed` instead of `unknown`.
  An already prepared integration whose evidence no longer binds is refused at its lease, dispatch or recheck with guidance
  of its own (reject the prepared intent while it is prepared or leased; inspect it once it is dispatched), and the refusal at
  `review.decide` names `/args/round_id`. No schema change.
- Evidence for a workspace-settled execution is checked against the execution's reported result when a review
  round is bound to it, not only at completion and integration. `review.open` and an approving `review.decide`
  refuse it with HTTP 409 `EvidenceInvalid` when the `payload.output_digest` it declares or the sha256 of its stored
  artifact differs from the output the execution reported; the response lists every failing reason, shows 12-character
  digest prefixes and names the repair (new evidence with matching digests, or a new execution that reports the
  stored artifact's hash). `evidence.submit` still accepts such evidence (one execution can have auxiliary evidence)
  and reports the relation in `execution_consistency.result_binding`. Works whose completion does not verify an exact
  result, and executions that are not workspace-settled, are unchanged. No schema change.
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
