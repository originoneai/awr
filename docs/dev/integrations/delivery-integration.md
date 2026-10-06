# Durable delivery integration authority

The `DeliverySyncStore` library separates permission to integrate a candidate
from observation that a repository changed. It uses the neutral
`IntegrationRequest` and `IntegrationObservation` records. GitHub, pull requests
and webhooks are not required.

This library prepares and dispatches authority, and records adapter outcomes. It
does **not** execute Git, expose new MCP/server routes, or complete a task.
Repository effects require an explicitly configured integrator. Final acceptance
and source publication remain separate operations.

## Prepare an exact candidate

`prepare_integration` accepts a current `DeliveryReadSet`, selected candidate
digest and selection version, connector version, and actual AWR evidence, review
round and decision IDs. Only fast-forward is currently supported. Caller flags
such as `approved`, `resolved` and `passed` cannot replace the original records.

The issuing identity needs current `delivery.finalize` permission and scoped
workstream write access. A developer's submission permission cannot authorize
integration. The store resolves and validates:

- Current source, contract, coordinator epoch, ownership, task eligibility and
  selected candidate, including its manifest, source revision and target.
- Original evidence digest and `payload.delivery_candidate_digest`. The existing
  `input_digest` still describes execution input; it is not a candidate digest.
- Finalized artifact bytes, hash and length, linked to current work evidence.
  Manifest artifact IDs are logical names. Stored evidence artifact IDs are
  independently allocated; their actual bytes bind these identities.
- The pinned, still-approved AWR review and exact decision, evidence, artifact
  and execution. Existing caller-managed settlement verification is reused.
- Every contract-required check, from current authenticated adapter fact heads,
  with exact candidate, connector, source, selection and ownership bindings.
  Missing, failed, unknown, historical and caller-declared checks cannot pass.

Explicit Agent review can use separate simulated members sharing a controller.
Authenticated author/executor/reviewer actor and client identities must differ.
It remains `agent_review`; it does not become human approval. Existing human and
self-review policies keep their original meaning. Ordinary human confirmation
uses its existing workflow rather than this approved-round integration path.

Preparation stores one immutable intent and acquires a target guard keyed by
tenant, opaque repository resource and reference. Different tasks or projects
targeting that same resource/reference contend. RLS hides other projects' owners;
a conflict only blocks the acquisition. A conflicting request rolls back its
entire preparation. Reusing a request key with changed content is an error.

## Lease and dispatch once

`lease_integration` requires the configured system connector principal and its
independent current delivery permission. A lease lasts 5–300 seconds and carries
an opaque fence. Before dispatch, an expired lease can be replaced. A live lease
cannot be overwritten, and an old fence cannot dispatch after replacement.

Both lease and `dispatch_integration` recheck the original issuer through the
common live authentication path. A private credential ID plus irreversible hash
preserves the original credential reference. No raw token is stored, and no caller
can deserialize or provide this reference. Revocation, rotation, expiry, changed
membership/grants/delegation, source, ownership, selection, approval, bytes or
check versions prevent a new effect. Exact admitted delegation and parent records
are pinned as well as their effective actions. The worker's lease also binds its
original credential and authority.

Dispatch persists before returning `DeliveryIntegrationDispatch`. Its first
successful response includes a `DeliveryIntegrationPermit`, which cannot be
cloned or deserialized. A replay returns the receipt with `permit: None`, even
after store reconstruction or under a different request key. A described receipt
does not grant permission to execute. An integrator must consume the permit.

The [local Git integrator](local-git-delivery.md#opt-in-fast-forward-integration)
consumes that permit for one configured fast-forward CAS. After repository
preflight it uses `recheck_integration` to revalidate live authority, eligibility
and the dispatch lease immediately before launch. This method accepts the sealed
permit and returns no new capability. Its successful check cannot make a replay
executable. The library boundary still exposes no new public transport route or
default background service.

## Resolve effects without blind retries

| State | Meaning | Recovery |
| --- | --- | --- |
| `prepared` | Exact approved intent; no dispatch | Lease or reject before dispatch. |
| `leased` | One worker owns the current fence | Dispatch once; replace only after pre-dispatch expiry. |
| `dispatched` | Durable dispatch; outcome not yet established | Query the original attempt; retain the guard. |
| `unknown` | Adapter reports an unresolved effect | Query and reconcile; retain the guard. |
| `confirmed` | Bound adapter fact establishes application | Retain historical evidence; release the guard. |
| `rejected` | Cancelled before dispatch or a bound adapter rejection | Retain the original receipt; release the guard. |

`confirm_integration` resolves an intent using an already-ingested, authenticated
neutral adapter fact bound to its original request and candidate. A verification
record, observation for another request, or inspection begun before dispatch
cannot resolve it. Pending and unknown
observations retain the guard. A rejected observation must come from an adapter
that can establish the original attempt's rejection; absence of a reference
change is not such proof.

After dispatch, lease expiry, reconnect, restart and a missing repository change
cannot authorize another effect. If the response was lost, inspection preserves
the original intent, fence and dispatch receipt. `reject_prepared_integration`
cannot cancel dispatched or unknown work.

Changes after an actual effect do not erase that fact. A still-authorized current
connector can record a historical result when the original issuer or eligibility
changed. The confirmation's `current` describes eligibility at that confirmation;
it is not a finalization grant and must be rechecked before acceptance.
`inspect_integration` exposes descriptions without private authority references.
It includes the immutable original candidate, read set and connector description
so recovery does not accidentally query a replacement selection.

`reserve_integration_inspection` requires a currently authenticated configured
system connector and current read set. It reserves the original dispatched
candidate/source/ownership/selection through the existing neutral inbox even when
ordinary `reserve_inspection` correctly rejects that candidate as stale. This
grants observation only. Old bindings remain historical; currently revoked or
unmapped connectors cannot ingest them, and facts from an inspection begun before
dispatch still cannot resolve the intent.

Integration confirmations never create completion receipts, mark work complete,
or silently update authoritative source status.

## Compatibility and verification

PostgreSQL schema 46 adds durable intents and target guards with forced project
RLS. The migration is transactional and preserves existing delivery records.
Upgrade through the normal migrator; binaries expecting another schema must not
write it.

Synthetic PostgreSQL regressions exercise real task, execution, evidence and
review commands under distinct simulated members, concurrent preparation and
dispatch, stale fences, original identity revocation/rotation, changing records,
lost replies, reconstructed stores, historical observations, target contention
and atomic upgrade. These checks establish mechanism behavior. They do not claim
native client, deployed Team or complete business acceptance.
