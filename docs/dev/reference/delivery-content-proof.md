# Complete integration content proofs

`IntegrationContentProof` is a provider-neutral delivery record. It describes
an adapter's complete-content observation; parsing it performs no Git, network
or model operation and grants no repository, review or acceptance permission.
Existing delivery records retain their wire shape and remain readable.

The record binds the entire candidate, original request when present, observation
reference, result revision and provenance. Its witness has three explicit forms:

| Witness | Meaning |
| --- | --- |
| `exact_revision` | Source and result are the same immutable Git object in the same resource. |
| `matching_complete_snapshots` | All source/result tree identities match, and the declared exact target base has been verified in both histories. |
| `unavailable` | No usable proof was obtained, with a typed reason such as missing history, changed content/base or an unstable target. |

Complete Git tree identities cover every path, object identity and file mode,
including files absent from the delivery manifest. SHA-1 and SHA-256 are explicit
formats. Tree identity is distinct from commit identity: equal trees alone do
not prove the required base remains in both histories. A rewritten result with
a changed base needs a new candidate and applicable verification/review. A
missing target cannot establish the exact-base witness; exact-source creation
remains compatible. Subset digests and caller flags are not proof formats.

## Ingestion and original-attempt confirmation

The configured observer principal ingests proof through the existing reserved
inspection, scoped connector, authenticated inbox and RLS boundaries. The store
assigns recording time; unknown external observation time remains unknown.
A proof has its own stable fact slot keyed by observation reference. New
unavailable observations replace that slot without replacing verification run
IDs, approvals or immutable earlier facts.

An `Applied` observation of the exact source Git revision retains the ordinary
confirmation flow. A different Git result requires exactly one matching usable
proof **in the same inbox batch as that post-dispatch observation**, under the
original connector version. Candidate, original request, observation reference,
result and report provenance must match. The store never borrows an older proof
head, a separate observation batch or a confirmation argument.

Missing, unavailable, ambiguous, early, wrong-version or mismatched proof refuses
confirmation and preserves the original unknown-effect guard. It does not issue
another execution permit. Exact request replay retains the original receipt;
current identity and permissions remain required. An actual historical effect
can be confirmed with `current=false` when the original issuer or eligibility
has changed. That is distinct from current approval or task acceptance.

## Inspection and boundaries

The ordinary neutral view exposes a bounded proof summary, its fact ID and
provenance. Authorized `delivery.integration.inspect` exposes the confirmation's
original `content_proof_facts` envelopes from that inbox, including unavailable
descriptions, without reinterpreting an existing terminal receipt. Confirmation
receipts add only the short proof basis and fact reference.

This record and store gate establish the common protocol. Actual local-Git and
optional GitHub observation support must verify complete trees, retained base,
target stability and artifacts through their bounded repository interfaces
before emitting a usable witness. Protocol fixtures are synthetic descriptions;
they do not establish real repository proof or native business acceptance.
This mechanism adds no squash, rebase or merge mutation capability. Integration,
AWR acceptance, human approval and confirmed source publication remain distinct.
