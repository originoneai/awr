# Project navigation and member activity

## Connect once

Inspector administrators can create a human member, select their role and
workstreams, and copy one client-neutral connection instruction. The browser
generates a personal project-scoped credential; only its hash is registered.
The plaintext remains in the one-time panel until explicitly cleared, logout or
project change. It cannot be retrieved later. Lost credentials require explicit
rotation. Protect the copied instruction as a secret.

After connecting, call `awr_team_query` with `op: "capabilities"` to confirm the
current identity and permissions, then `op: "work.next"`. Both use
`protocol_version: 1`. This project-level entry lists up to 20 resumable sessions
owned by the current actor/client and a paged set of visible unfinished tasks.
It distinguishes preparation, observation, held leases, dependency waits and
recovery needs without exposing private dependency names. Follow the returned
`next_query`; consume `work.prepare` before claiming or executing. Navigation
grants no execution authority and does not certify dependency acceptance.

Own-session continuation links use `work.observe`. A single current, live run
owned by this client is marked `resume`; expired leases, changed execution
bindings and unresolved effects still require recovery. `session.inspect` and
`work.recovery` retain historical checkpoint actions and include the same bounded
current `guidance` as observation. Follow that guidance rather than replaying the
checkpoint action. Expired leases cannot be renewed or revive an old execution;
settle pending effects before preparing a fresh claim and a new admitted run.

After accepting an assignment or self-claiming available work, an active own
session with a live current claim receives `execution.prepare` guidance. Supply
current versions, a measured input digest and the contract's declared scope.
Once that exact execution is prepared, the next action is `execution.start`;
effects require successful admission. Both hints require current scoped execution
authority and a write grant. Each includes its condition, factual basis and
recheck trigger within the existing bounded guidance packet.

Context gaps, recovery, changed bindings, lease expiry, user waits and pending
review or integration keep their existing priority. Running executions retain
boundary progress and truthful terminal-settlement instructions; terminal runs
are not automatically prepared again. Advice grants no authority, and every
command rechecks its versions and permissions.

Repeat `work.next` after progress, a resolved wait, claim conflicts, source or
permission changes. Commands retain stable request IDs and recheck current
authority; inspect unknown outcomes before retrying. The connected Agent performs
this loop. AWR does not start arbitrary Agent applications or wake a stopped
client.

## Activity queries

`audit.requests` lists authenticated HTTP/Web/MCP operation metadata: actor,
client, credential ID, action, visible task, time and result. Recording starts
before dispatch; interruption or a missing final record remains `unknown`.
Malformed or unauthenticated transport traffic is not attributed to a member.
The oldest request records are pruned at a soft capacity of 10,000 per project.
This is operational history, not a permanent compliance archive.

`audit.development` projects committed events with operation attribution. It
covers recorded session, claim, execution, checkpoint and delivery actions;
legacy events can lack client attribution. Neither view contains conversation
text or arbitrary tool input/output. Project audit authority is required to view
other members; ordinary members see their own records, further restricted to
current workstream visibility.

Keyset cursors bind the caller, filters and grants. Use `next_cursor`; changing
filters requires a fresh first page. A page does not provide an immutable export:
request outcomes can settle and old request metadata can expire between pages.
Existing `audit.history/export/count` retain their permission, planning and
delivery semantics.

Schema 33 adds only request metadata storage. Reapply the documented app-role
grants after migration. Business development history is read from existing
committed events and receipts; it is not reconstructed from browser activity.
