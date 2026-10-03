# Team operator access management

## Member directory and project credentials

Call `awr_team_access_inspect` with only `protocol_version: 1` to list members.
The optional `limit` (1–100) and `cursor` provide keyset pagination. Supplying
both subject selectors retains single-client inspection. The directory includes
client grants and credential expiry/revocation metadata, never bearer values or
registration hashes. A delegated administrator sees only members whose full
active project grant scope they can manage.

An access plan may set `credential_project_scoped: true` when registering a new
credential hash. Schema 32 binds that credential to the URL's project as well as
its existing actor/client grants. Existing tenant credentials retain their scope.
`revoke_project_credentials` atomically revokes specified credentials belonging
to that exact actor/client/project; it refuses legacy tenant credentials and
credentials of another project. Rotation can register the new hash and revoke
the previous project credential in the same preview/apply transaction.

Generate the random bearer in the caller's explicit one-time delivery channel
and send only its hash to preview/apply. Keep the same plan and request ID until
the result is known. After a disconnect, query the existing request's outcome;
never silently issue a replacement. If the raw bearer was lost, an administrator
must deliberately rotate it. The server cannot recover its plaintext.

## Project-admin MCP/HTTP management (AWR-TMCP-012)

After local owner bootstrap (database migrate + first project admin via
`awr-server access`), **daily** member add/remove, role/scope adjust, and
credential register/rotate are performed by authorized project admins through
the app-role business entry:

| Surface | Ops |
| --- | --- |
| MCP | `awr_team_access_inspect`, `awr_team_access_preview`, `awr_team_access_apply`, `awr_team_access_outcome` |
| HTTP | `POST /v1/projects/{key}/access/{inspect,preview,apply,outcome}` |

These paths require live `access.manage_project` (project admin / `admin` /
`project_admin` membership) **and** an authenticated client grant ceiling:
requested role/grant changes cannot exceed the caller's explicit workstream
`manage` grants and grant bits. Admin membership alone (for example an unscoped
credential with zero grants) cannot bootstrap rights. They never expose database
owner privileges, arbitrary SQL, arbitrary server paths, or the owner-only
recovery CLI.

Plans are **project-bounded**. Grant ceilings also exclude special authorities
(`attest_execution`, `reconcile_execution`). Tenant-wide credential revoke is
**refused** on this path — clear or replace **this project's** grants instead
(owner `awr-server access` remains available for tenant credential revoke and
recovery). Preview/apply responses label impact on other clients that share the
subject actor's membership and state that other projects' grants are preserved.

Concurrent admin applies serialize on subject-actor + digests; exact
`request_id` replay returns the historical receipt. Removing or demoting the
last admin without a prior handoff is refused. Raw bearers are generated only
via the protected install channel (`awr-server access token` → local `0600`
file) and registered by `secret_hash` only; query/audit/MCP responses return
redacted identity and auth metadata — never raw secrets.

Owner-only surfaces (recovery-inspect, history migration, quarantine,
execution attribution, backup/fencing/rebuild) stay on `awr-server access` and
remain unreachable from client HTTP/MCP.

---

The development branch provides `awr-server access` for an operator to provision
clients of an already enabled workstream project. It does not create a project,
activate sources, launch an executor or announce a release. Complete the
[Team service setup](team-workstream-service.md) first.

Database operations require the schema owner's PostgreSQL privileges through
`AWR_TEAM_DATABASE_URL`. Ordinary service application credentials and HTTP/MCP
bearers cannot use this operator interface. The running service continues to use
its separate application connection. Upgrade explicitly with
`awr-server migrate --app-role <service-role>` as owner; schema 18 adds operator access,
history-migration, backup-operation, claim/execution quarantine, and explicit execution-attribution receipts; schema 19 adds `project_access_changes` for project-admin MCP receipts (app role may INSERT/SELECT only). Bootstrap denies the application role all access to owner-only operator tables.

## Initial person-Agent delegation

An `actor.kind="agent"` credential alone cannot start work. The Agent also needs
an active person binding and an explicit delegation. For initial provisioning,
the schema-owner CLI supplies four commands, with no new HTTP or MCP owner route:

```sh
awr-server access agent-preview --input /secure/agent-plan.json
awr-server access agent-apply --input /secure/agent-plan.json \
  --request-id initial-agent-1 --expected-state <state_digest> --expected-plan <plan_digest>
awr-server access agent-outcome --tenant-id tenant-a --project-id project-a \
  --request-id initial-agent-1
awr-server access agent-inspect --input /secure/agent-plan.json
```

First use ordinary access preview/apply to register a **new Agent identity**, its
exact native client, membership, credential hash and current workstream grants.
Do not relabel an existing human actor. Then construct a closed JSON plan with
`protocol_version: 1`, `tenant_id`, `project_id`, and `authorization` (the WS-016
`AgentAuthorization` document). No bearer or credential hash belongs in this
second plan. The initial authorization must have:

- A fresh authorization ID and fresh binding ID; no existing active delegation
  for that actor/client and no existing binding for that Agent.
- The same `authorizer_person_id` and `responsible_person_id`, explicitly chosen
  from an active human project-member actor. This owner adapter uses that actor
  ID as the person ID and its display name; it never renames, reactivates or
  replaces an existing person. The audit records the actual database owner as
  operator, **not** an observed action by that human.
- `subject_kind: "agent"`, the existing Agent actor and exact client ID, and an
  active, parentless authorization without session binding, maintainer, verified
  capabilities or skill hints. `model_id` remains optional metadata.
- Only `Task` or `Workstream` scope in the current source catalog. Task scope
  requires an enabled current contract and matching current/snapshot ownership.
- Only `inspect`, `claim_coordination`, `start_work`, `propose_planning` and `review`, covered by the
  live membership and read/write grant. Other WS-016 actions lack a native
  product adapter and are refused here, as are special manage/attest/reconcile
  grants.
- A nonfuture `created_at_ms` at most 24 hours old when previewed/applied, and an
  optional `expires_at_ms` strictly later than the database's current time.

Binding, person creation if needed, authorization, receipts, project revision,
event and audit commit in one transaction. Preview digests bind live access,
source, person, binding and authorization facts. Stale plans fail; timeouts use
`agent-outcome` followed by an exact retry. Replays return historical receipts
and never recreate revoked access. `agent-inspect` reports `configuration_matches_plan` and a `mismatch_reason`, comparing the live
configuration with the retained initial plan. It does not calculate the runtime
permission decision. Each native request still evaluates its live delegation,
role and scope. Human actor/member/name drift or additional bindings are plan
mismatches, **not** supported revocation operations. Use authorization,
credential or grant revocation to stop access. Inspection is not native-client
execution or acceptance evidence.

Native read isolation follows [Delegated Agent read scope](team-access.md#delegated-agent-read-scope).
Task-scoped Agents can use selector-free `work.next` to discover their exact
assigned tasks and resume their own current-client sessions on those tasks.
Other reads require a covered `work_id`; task grants do not expose the owning
workstream's catalog or sibling tasks. Use Workstream scope for an Agent that
needs to discover and choose any task in a workstream. Provisioning must be
shipped with this read-scope enforcement; grants alone do not narrow delegation.

A developer normally uses `inspect`, `claim_coordination` and `start_work` with
a developer membership. Add `propose_planning` explicitly when the Agent should
submit suggestions about its assigned work. Each suggestion must name 1–256
enabled published tasks covered by **one** live delegation and writable client
workstream grants. Empty, unknown or uncovered affected work is refused; separate
task grants are never combined. Reservation, replay and domain submission all
recheck live authority, including revocation, expiry and child narrowing.
Suggestions remain unclaimable and cannot edit drafts, approve, publish, change
live dependencies or authorize execution. Existing delegations gain no new
permission. Deploy a compatible server before issuing the new action; older
servers do not understand it. Agent review additionally requires membership
`agent_review=true` and delegated `review`. Review currently needs a durable
session, so a reviewer that creates its own session also needs `start_work`;
that action includes execution/delivery permissions and is **not** review-only
least privilege. Use a narrow task/workstream scope. Agent review never implies
human approval or team-independent human acceptance.

Use the existing owner `authorization-revoke` command to stop delegation, or
ordinary access management to revoke the credential/grant. Initial provisioning
does not implement binding replacement, retirement or extension; those are
separate lifecycle operations. Retain the reviewed plan for inspection, and use
fresh identity/authorization IDs for an independently scoped initial setup.

### Renew an expired Agent authorization

An expired finite authorization can receive one immutable successor without
rerunning initial provisioning or replacing the person↔Agent binding. First use
the ordinary access token/preview/apply flow to register a fresh credential when
the prior credential also expired. Then create a renewal plan with
`protocol_version`, `tenant_id`, `project_id`, `previous_authorization_id`, and a
new `authorization` document.

The successor requires a fresh authorization ID and current issue/expiry times.
Every authority-bearing field must exactly match the predecessor: authorizer and
responsible person, subject kind/ID, client, binding, session/model, scope,
actions, capabilities, hints, parent and maintainer. Its finite lifetime duration
must equal the predecessor's duration. The predecessor must already be expired
and must not be revoked, and it must be the latest authorization in that
subject/client history. A later successor, including one later revoked, prevents
renewal from an older grant. The active project, person, Agent, binding,
membership, grant, source scope and credential must still be valid. Renewal
inserts the successor and leaves the predecessor and binding unchanged.

```sh
awr-server access agent-renew-preview --input /secure/agent-renewal.json
awr-server access agent-renew-apply --input /secure/agent-renewal.json \
  --request-id renew-agent-1 --expected-state <state_digest> --expected-plan <plan_digest>
awr-server access agent-renew-outcome --tenant-id tenant-a --project-id project-a \
  --request-id renew-agent-1
```

After an uncertain result, query `agent-renew-outcome` before retrying the exact
plan, request ID and digests. A changed client, binding, scope, action set,
lifetime, revoked predecessor, stale preview, or another effective authorization
is rejected. This owner recovery path does not rotate credentials, revive a
revoked authorization, grant new authority, dispatch an Agent, or prove native
client execution.

### Authorize an additional Agent scope

An already bound Agent can receive an independently reviewed authorization for
a new workstream or one enabled task without changing its existing
authorizations. Use a closed plan containing `protocol_version`, `tenant_id`,
`project_id`, and a complete new `authorization` document. The authorization
must have a fresh ID, current issue time, finite future expiry, the same
authorizer and responsible person, and the existing active binding. Parent,
session, maintainer, capability, skill-hint, and revocation fields are not
accepted. Exactly one active binding must match the proposed person, Agent, and
binding ID. Disabled historical binding rows remain part of the preview
fingerprint and are preserved, but do not defeat that active-binding check.

The target workstream must be active in the current source snapshot. A task
scope must name an enabled task whose live ownership matches that snapshot. The
Agent, person, project membership, credential, and target grant must all remain
active. Requested `inspect`, `claim_coordination`, `start_work`, `propose_planning`, and `review`
actions must fit the current membership and exact target grant; management,
attestation, or reconciliation grants are refused. An effective authorization
for the same Agent/client cannot already cover the same workstream or task, a
task in the proposed workstream, or the proposed task's workstream.
If an effective task authorization's current and snapshot ownership is missing,
disabled, or diverged, a proposed workstream authorization is refused because
scope separation cannot be proved. Expired and revoked authorization history
does not create an overlap.

```sh
awr-server access agent-authorize-preview --input /secure/agent-authorization.json
awr-server access agent-authorize-apply --input /secure/agent-authorization.json \
  --request-id authorize-agent-stream-2 \
  --expected-state <state_digest> --expected-plan <plan_digest>
awr-server access agent-authorize-outcome --tenant-id tenant-a --project-id project-a \
  --request-id authorize-agent-stream-2
```

Preview and apply fingerprints include current source, ownership, access,
binding, identity, and authorization history. Apply inserts only the new
authorization and its receipts; prior authorizations and all identity, binding,
credential, membership, and grant rows remain unchanged. After an uncertain
response, query `agent-authorize-outcome` before retrying the exact request.

Explicit reads and commands select the authorization that covers their
`work_id`. `workstreams.list` and `work.next` discover all independently
authorized readable workstreams. Use a returned `workstream_id` for scoped
listing or search; when several streams are visible, an unscoped list/search
requires a scope selection. Discovery does not combine action permissions
across streams or authorize execution. This operation records owner
authorization; it does not dispatch the Agent or constitute human approval or
team-independent acceptance.

## Register a client

Generate a bearer before registration, into a new file in an operator-controlled
directory:

```sh
awr-server access token --credential-id worker-one --output /secure/worker-one.token
```

The command uses operating-system randomness and prints only the credential ID,
registration hash and output path. It does not connect to PostgreSQL or register
anything. The file contains the bearer, is created exclusively, and is never
overwritten (including a symlink). Unix files use mode `0600`; on other systems,
the containing directory must provide the intended user ACL. Keep the bearer out
of command arguments, source files, logs, reports and version control. A file-write
failure registers nothing; inspect/remove the incomplete local file explicitly
before generating another credential.

Inspect the intended identity and available workstream catalog:

```sh
awr-server access inspect --tenant-id tenant-a --project-id project-a \
  --actor-id worker --client-id coding-client
```

For enabled workstream projects, inspect recovery barriers without mutating state.
This is schema-owner only, uses shared locks, and never restores, migrates history,
clears `recovery_blocked`, or inspects host filesystems (local files are not a
server ACL):

```sh
awr-server access recovery-inspect --tenant-id tenant-a --project-id project-a
```

The report lists recovery-blocked work, nonterminal/unknown executions, active
claims, open waits, unattributed legacy rows lacking `workstream_id`, previous-epoch
nonterminal executions, and recorded restore runs. Samples are bounded. Disabled
or non-workstream projects return `Unsupported`/`Forbidden`. Client HTTP/MCP
cannot call this path.

Unattributed history (sessions/claims/events/executions lacking `workstream_id`)
is never adopted automatically. Preview a bounded migration plan, then apply only
with exact digests. This slice attributes sessions, inactive claims, and events
that have a unique current `workstream_ownership` binding. It refuses active
claims, all executions (use execution-attribution with a reviewed
`executor_client_id`), rows whose
`work_id` is absent from ownership, and never modifies completion receipts,
evidence, actors, or trust grades:

```sh
awr-server access history-preview --tenant-id tenant-a --project-id project-a
awr-server access history-apply --tenant-id tenant-a --project-id project-a \
  --request-id migrate-1 --expected-state <state_digest> --expected-plan <plan_digest>
awr-server access history-outcome --tenant-id tenant-a --project-id project-a \
  --request-id migrate-1
```


Active claims and unattributed executions are out of scope for history-migration.
Owner-only recovery preview/apply can release or quarantine active claims (and
attribute-and-release when current ownership uniquely binds the work), and can
quarantine-cancel nonterminal unattributed executions. It never invents
`executor_client_id`, never forges actors/completion receipts, and refuses
terminal unattributed executions (use execution-attribution instead):

```sh
awr-server access quarantine-preview --tenant-id tenant-a --project-id project-a \
  --claim-disposition release
awr-server access quarantine-apply --tenant-id tenant-a --project-id project-a \
  --request-id quarantine-1 --expected-state <state_digest> --expected-plan <plan_digest> \
  --claim-disposition release
awr-server access quarantine-outcome --tenant-id tenant-a --project-id project-a \
  --request-id quarantine-1
```

Use `--claim-disposition quarantine` to revoke active claims that cannot be
attributed. Covered by `pg_operator_recovery` with `--features pg-tests`.

Explicit execution attribution binds CHECK-safe unattributed executions
(`session_id` and `claim_id` present) using a reviewed `executor_client_id` that
must match the recorded session client. It never invents client ids, never
rewrites completion receipts, and refuses already-attributed rows, missing
session/claim, missing ownership, session/client mismatch, and missing executions
clearly. Terminal executions are attributable when CHECK-safe; unsafe terminals
are refused with an explicit reason:

```json
{
  "protocol_version": 1,
  "tenant_id": "tenant-a",
  "project_id": "project-a",
  "attributions": [
    {"execution_id": "<id>", "executor_client_id": "coding-client"}
  ]
}
```

```sh
awr-server access execution-attribution-preview --input /secure/exec-attribution.json
awr-server access execution-attribution-apply --input /secure/exec-attribution.json \
  --request-id attrib-1 --expected-state <state_digest> --expected-plan <plan_digest>
awr-server access execution-attribution-outcome --tenant-id tenant-a --project-id project-a \
  --request-id attrib-1
```

Real PostgreSQL integration coverage lives in
`crates/awr-team-pg/tests/pg_operator_recovery.rs` (recovery-inspect, history
migration, quarantine, execution attribution, backup/fencing restore/rebuild)
and `pg_operator_access.rs`, run with `--features pg-tests`. HTTP/MCP real-client
denial that provisioned workstream bearers cannot reach these operator surfaces
(Unsupported on query/command tools; no operator HTTP routes) is covered by
`crates/awr-server/tests/operator_surface_denial.rs`, which also rechecks
schema-owner `access recovery-inspect` / `history-preview` on the same project.


Enabled-project logical backup metadata, guarded fencing restore, and a bounded
rebuild-from-manifest slice are owner-only. Legacy `ImportStore` backup/restore
already refuse enabled workstreams. This CLI records an
`awr-team-enabled-backup-v1` manifest (projection digests, ownership rows,
work-item inventory id+external_key, completion-receipt digests, source/artifact
digests). Physical `pg_basebackup` remains external. Restore preview refuses
projection/receipt/inventory drift, unattributed history, and outbox replay.
Restore apply performs verified fencing only: it never rewrites completion
receipts, forges credentials/grants, copies table rows from the manifest, or
treats local files as a server ACL.

A separate digest-gated rebuild path can materialize missing `work_items`
(id + external_key only) and `workstream_ownership` rows when the project is
fencing-quiet (no active claims/sessions/live executions), ownership is empty
or already matches the manifest digest, and every ownership work_id is covered
by current rows or the backup inventory. It refuses divergent ownership
overwrite, external_key conflicts, catalogs/contracts/grants/actors/receipt
rewrites, and automatic resume:

```sh
awr-server access backup-create --tenant-id tenant-a --project-id project-a
awr-server access backup-inspect --tenant-id tenant-a --project-id project-a \
  --backup-id <id>
awr-server access backup-restore-preview --tenant-id tenant-a --project-id project-a \
  --backup-id <id>
awr-server access backup-restore-apply --tenant-id tenant-a --project-id project-a \
  --backup-id <id> --request-id restore-1 \
  --expected-state <state_digest> --expected-plan <plan_digest>
awr-server access backup-restore-outcome --tenant-id tenant-a --project-id project-a \
  --request-id restore-1
awr-server access backup-rebuild-preview --tenant-id tenant-a --project-id project-a \
  --backup-id <id>
awr-server access backup-rebuild-apply --tenant-id tenant-a --project-id project-a \
  --backup-id <id> --request-id rebuild-1 \
  --expected-state <state_digest> --expected-plan <plan_digest>
awr-server access backup-rebuild-outcome --tenant-id tenant-a --project-id project-a \
  --request-id rebuild-1
```

Physical `pg_basebackup` and post-restore resource fencing remain operator
responsibilities outside this CLI. Catalogs, contracts, snapshot ownership,
completion receipts and grants are still outside this rebuild subset. Unit
tests cover restore/rebuild planning; `pg_operator_recovery` exercises
enabled-project backup, fencing restore, and bounded rebuild against real
PostgreSQL with `--features pg-tests`.

Save an access plan as local JSON. Use an actual workstream ID and current authority
version from inspection, and replace the hash placeholder with `access token`'s
`secret_hash` output:

```json
{
  "protocol_version": 1,
  "tenant_id": "tenant-a",
  "project_id": "project-a",
  "actor": {"id": "worker", "kind": "agent", "display_name": "Coding worker"},
  "client_id": "coding-client",
  "role": "worker",
  "grants": [{
    "workstream_id": "00000000000000000000000001",
    "authority_version": "1",
    "read": true,
    "write": true,
    "manage": false,
    "attest_execution": false,
    "reconcile_execution": false
  }],
  "credential": {
    "id": "worker-one",
    "secret_hash": "sha256:<64 lowercase hex characters from access token>",
    "expires_at_unix_ms": null
  },
  "revoke_credentials": []
}
```

`actor.kind` is `agent`, `human` or `system`; `role` is `reader`, `reviewer`,
`worker` or `admin`. Existing actor kind/name must match and are not overwritten.
An absent actor is created active. Read permission is required for every listed
grant. Worker/reviewer/admin membership may carry write permission; manage requires
admin. Executor attestation requires a system actor plus write permission.
Reconciliation requires human/system, admin and write/manage. These are explicit
trust delegations: ordinary coding agents should not receive executor/operator
identities. The service also checks authority at actual command execution.

Preview, review the exact scope, then apply using the returned `state_digest` and
`plan_digest`:

```sh
awr-server access preview --input /secure/worker-access.json
awr-server access apply --input /secure/worker-access.json \
  --request-id register-worker-one --expected-state <state_digest> \
  --expected-plan <plan_digest>
```

Preview performs no policy mutation. Apply requires both the reviewed state and
the exact reviewed plan to remain unchanged, and commits policy, versions, audit
event and an immutable receipt together. A stale or edited preview is rejected.
The plan is bounded to 64 KiB and 256 grants/revocations; unknown fields are
rejected. Share the bearer with the intended client through its supported secret
configuration; send it as the service's Authorization bearer. The database stores
its hash, never the raw bearer.

## Change, rotate or revoke access

The grants array is the **complete desired grant set for this actor/client/project**.
Omitted grants are deactivated, with their history and incremented versions retained.
Other clients' grant rows are preserved. An empty array removes all of this client's
workstream grants in the selected project. Reactivation requires another explicit
plan against the current catalog and never rewrites historical receipts.

Membership role is shared by **all clients of this actor in the project**. A role
change therefore affects their effective permissions even when their grant rows
are unchanged. Credential revocation is **tenant-wide across all projects that use
that credential**. Preview labels both boundaries; create separate actors or
credentials when independent administration is required.

Set `credential` to null to preserve existing credentials. To rotate, generate a
new credential ID/file, include its hash in `credential`, and list the old ID in
`revoke_credentials`. Registration and revocation commit atomically. Revocations
must name credentials of the selected actor/client; another client's ID is refused.
A credential ID cannot be rebound to another identity, hash or expiry, and revoked
credentials cannot be resurrected. Use a new ID. Expiry is an optional future Unix
timestamp in milliseconds. Grants to another workstream still require its explicit
current authority version.

## Recover an uncertain outcome

```sh
awr-server access outcome --tenant-id tenant-a --project-id project-a \
  --request-id register-worker-one
```

A committed result returns the original receipt. An unknown result is not proof
that an in-flight request failed. Retry only the identical plan, request ID,
expected-state digest and expected-plan digest. Exact retry returns the historical
receipt without changing policy; it cannot restore privileges that were later
revoked. Reusing a request ID with changed parameters fails. Use a fresh preview
and request ID for a new intent.
Receipts retain previous/current policy and omit credential hashes and bearer strings. Their `state_basis: at_commit`
is historical; use `access inspect` for current policy.

This interface does not bypass project/source activation or migrate old execution
ownership. It preserves admission-time executor trust: granting attestation now
does not retroactively authorize an earlier ordinary execution.

## Migration takeover drill (WS-050)

Fixture-first backup → preview → migrate → restore classification for mainline
takeover (including Team/EVO/DEC/AUTO goal ownership and
pending-confirmation identity rules) is documented in
[migration-takeover](migration-takeover.md). Use the independent fixture under
`tests/fixtures/workstreams/migration-takeover/` before project takeover
dry-runs. Owner-only `backup-*` / `history-*` commands remain the PG operational
path; the core oracle does not replace them.
