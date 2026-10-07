# Optional GitHub delivery observations

This adapter translates bounded, explicitly mapped GitHub HTTPS queries into the
same `awr-delivery` facts used by the [local Git adapter](local-git-delivery.md).
The project, candidate, source contract, authenticated connector and AWR review
remain authoritative. GitHub is optional; an external approval cannot become
an AWR business approval.

## Configure and observe

[`adapter.toml`](../../../examples/github-delivery/adapter.toml) maps one tenant,
project, workstream and work item to an opaque resource, numeric repository ID,
owner/name, API origin and target branch. An optional same-repository PR number
adds a change-request observation. Fork PRs and SHA-256 repositories are not
supported by this first adapter. Omit `pull_number` for a branch-only workflow.

Map each required AWR check to its exact GitHub check name **and numeric producer
App ID**. A successful check with the same name from another application does not
satisfy that mapping. At most 28 producer mappings fit the bounded inbox batch.
Unsupported required checks remain explicit gaps. Legacy commit statuses are
not treated as supported check runs.

The trusted operator loads `GitHubAdapter::open(config, credential)` with an
optional, separately supplied zeroizing credential. The config and reports do
not contain that credential. Opening the library adapter validates configuration
without reading GitHub. `from_transport` is a trusted library extension used by
synthetic tests, not an HTTP/MCP configuration or caller input.

Register a scoped `github` connector using the existing delivery configuration
flow and its actual observer actor/client identity. `reconcile` checks current
selection, source read set, connector version and resource before reserving an
authenticated inspection. It submits observations to the durable inbox; it does
not bypass source publication, grant approval, execute a merge or finalize work.
Keep the request ID and read set when recovering a missing receipt. Reusing the
same inspection returns its original digest-bound report after restart; changed
bindings or corrupt proof are rejected. A fresh request obtains a new observation.

## What the report proves

- Exact immutable source commit; regular tree/blob paths from `git-blob:` artifact
  locators; hashes and byte lengths of actual decoded bytes against the manifest.
  Symlinks, submodules, oversized blobs and unsupported locators cannot pass.
- Complete, bounded check-run pagination on that commit, with unique run IDs,
  producer binding and a fresh reobservation of each matched run. Missing,
  conflicting, skipped, neutral or unfamiliar results are unknown. Changed runs
  cannot carry a passing observation from the preliminary query.
- Pinned target revision and ancestry, plus matching bytes in the target tree.
  Merely seeing `merged: true` on a PR does not prove integration. A target or PR
  that changes during inspection yields uncertainty; repository identity and
  PR/target observations are checked again before publishing the report.

Finite immutable reports contain mapped identities, revisions, digests, outcomes
and observation time. Provider response bodies, check output, commit messages,
repository contents, credentials and provider error messages are excluded.
`report_bytes` resolves proof by SHA-256 and rejects altered bytes. Facts express
what was observed at that time; later polling and source publication remain
separate operations.

## Transport and limits

The production transport uses verified HTTPS, the pinned API origin (including
the optional Enterprise `/api/v3` path), the `2022-11-28` REST API version and GET
only. It disables redirects and environment proxies. URLs and pagination are
constructed locally; response `Link`, blob download URLs and PR URLs are never
followed. Credentials cannot redirect to another host.

Request and inspection deadlines, request count, response and cumulative response
bytes, blob and total blob bytes, and check count are bounded. Blocking HTTPS
runs outside async service threads. A dropped inspection future cannot publish
a late report; its read-only blocking job retains its concurrency permit until
its bounded work exits. Provider/authentication/rate-limit/transport failures do
not manufacture passing or rejected terminal facts.

## Current delivery boundary

This is a library observation adapter with explicit authenticated reconciliation.
Background service admission, signed wake-up notifications, fallback polling and
synchronization timing measurements are separate work. No webhook body is an
authoritative delivery fact.

Repository effects are also separate. GitHub REST PR merge `sha` guards the head
commit only; Git reference PATCH has no expected-old target precondition. Neither
can supply the exact target guard required by the neutral integration permit.
This observer therefore advertises `integration_requests: false`. It does not
weaken a target precondition or claim support for automatic merge/finalization.

Synthetic API conformance, controlled real HTTPS transport, isolated PostgreSQL
mechanism regressions and complete natural-client business acceptance are
different verification scopes. Passing the former does not imply the latter.
