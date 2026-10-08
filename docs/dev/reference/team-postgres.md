# Team PostgreSQL store

Team V1 coordination state lives in PostgreSQL. Personal CLI/MCP still use
SQLite and do not link this store.

## Local verification

```sh
docker compose -f docker/team-postgres.yml up -d
export AWR_TEAM_TEST_DATABASE_URL='postgres://postgres:awr-test@127.0.0.1:55432/postgres'
cargo test -p awr-team-pg --features pg-tests -- --test-threads=1
cargo test -p awr-server --features pg-tests -- --test-threads=1
```

The PostgreSQL-backed suites read `AWR_TEAM_TEST_DATABASE_URL` only, never the
service's `AWR_TEAM_DATABASE_URL`, and refuse a non-loopback host. Each test
process creates its own database, but the fixture also creates the cluster-wide
role `awr_app` with a fixed test password and tests drop and rebuild schemas, so
use a throwaway PostgreSQL 17 (the compose file above, or the service container
of the CI workflow) and never a shared, Beta or production server, even through
a tunnel that makes it look like loopback. Use one test thread. A failing test
no longer poisons the shared fixture lock, so the first failure in the output is
the real one.

CI runs the same two commands in `.github/workflows/team-postgres.yml` against a
PostgreSQL 17 service container, for every change under `crates/`.

To run the service itself against that database:

```sh
export AWR_TEAM_DATABASE_URL='postgres://postgres:awr-test@127.0.0.1:55432/awr_team_test?sslmode=disable'
cargo run -p awr-server -- migrate
```

`awr-server check` exits non-zero when `awr_team.schema_state` is missing or
the version does not match. The command entry (`TeamStore::execute`) runs the
same check before opening a write transaction.

Upgrading a database bootstrapped by an older build: run
`awr-server migrate --app-role <role>` once with owner credentials. This
re-applies the application grants (idempotent, no schema rebuild, no data
loss); older grant sets did not allow the app role to read `schema_state`. `awr-server migrate` applies owner migrations on
a clean database and returns successfully when the expected version is
already present. The application role is not table owner and does not
receive `BYPASSRLS`. Event history is insert-only for that role.

Source publish is ingest → approve → activate. Path checks, hashing and
parser binding happen before the project lock. The lock only writes already
hashed, immutable rows. An unactivated candidate cannot be read as the
current contract. A failed activation keeps the previous `active_snapshot_id`.

## Connection pooling and TLS

Domain stores acquire connections from a `deadpool-postgres` pool instead of
opening one TCP connection per operation (ADR-0004). Pool size defaults to 8
per store instance and can be overridden with `AWR_TEAM_PG_POOL_MAX_SIZE`.
Acquire/create/recycle timeouts are fixed at 10s/5s/5s. Scope binding uses
transaction-local `set_config`, so fast connection recycling is safe.

Owner migration commands (`migrate`, `check_schema`) keep a dedicated single
connection and do not use the pool.

TLS is an opt-in `tls` cargo feature (rustls + webpki-roots). With the feature
enabled, `sslmode=require` in `AWR_TEAM_DATABASE_URL` selects a verified TLS
connection. The connector explicitly uses the AWS-LC rustls provider, so its
construction does not depend on a process-global provider when another
workspace dependency also enables `ring`. `disable`/`prefer` or an omitted
sslmode stays plaintext. The driver accepts only `disable`, `prefer`, and
`require`; it rejects libpq's `verify-ca` and `verify-full` spellings. Without
the feature, a TLS-requiring URL fails with an explicit error instead of
silently downgrading.

This is not a production high-availability topology.

## Consistent reads

`work.prepare`, `work.graph`, `session.inspect` and `events.list` run in
`REPEATABLE READ`. Event cursors are `awr-team-cursor-v1:{epoch}:{revision}:{index}`.
A changed coordinator epoch returns `EPOCH_CHANGED` instead of skipping history.
Required hard rules are never dropped to fit a context budget.

Experimental entry:

```sh
cargo run -p awr-server -- query --op capabilities
cargo run -p awr-server -- query --op work.prepare --body '{"tenant_id":"...","project_id":"...","work_id":"work-a"}'
```

Unknown query names return `Unsupported` without changing personal CLI/MCP.

## Sessions and leases

Claims are unique per work item while `state='active'`. Lease expiry uses
`clock_timestamp()` after the project lock, not transaction `now()`. Renew
replays keep the original `expires_at`. Wait records do not extend the lease.
Handoff increments the work fence so the previous actor cannot write.

## Dependencies and conflicts

Required dependency graphs are rejected if they cycle or reference missing
work. Resource reservations treat directory prefixes as overlapping path
segments (`src/foo` vs `src/foo/bar`), not raw string prefixes (`src/a` vs
`src/abc`). Splitting a work item does not complete the parent. Unknown
scopes are rejected instead of falling back to `main`.


## Execution protocol

`execution.prepare` writes the execution row, effect key and outbox record in one
transaction. Outbox delivery is claimed with `SKIP LOCKED` after the project lock
and sent outside that transaction. The reference runner persists `execution_id`
before side effects; a duplicate delivery returns the journaled outcome without a
new effect key. `unknown` sets `recovery_blocked` and keeps resource reservations.
`cancel_requested` is not `cancelled`. Callers cannot mint `trusted_executor`
receipts. Uncontrolled third parties do not receive an exactly-once claim.


## Evidence and completion

Completion receipts are written only through the domain `complete` entry.
`caller_asserted` reports cannot satisfy `trusted_execution_and_review`.
Authors cannot approve their own review round; a new bundle hash invalidates
the previous round. `work_runtime.state='completed'` requires
`selected_completion_id`. Ordinary confirmation is allowed only when the
current contract already selects that policy.


## Import and restore

Import is freeze → export → dry-run → load → activate. The same
`import_key` and manifest hash replay the original job and do not create
duplicate work. Divergent local sources are rejected instead of last-write
wins. Historical self-reports stay `caller_asserted`. Restore mints a new
coordinator epoch, revokes restored credentials, fails pending outbox rows
instead of replaying them, and refuses a SQLite file rollback after the
team project has accepted new revisions.


## Import, freeze and restore integrity (schema 9)

`ImportStore` is the administrative cutover boundary. Drain running work before
freezing: ordinary command, source, session, execution, graph and review writes
serialize on the project row and require `status=active`. Freeze is idempotent.
Export requires a frozen project and uses a repeatable-read transaction plus the
coordination lock, returning the source project, snapshot, epoch and revision.
Read-only inspection remains available while frozen.

The `awr-team-import-v1` JSON manifest has `scopes: ["main"]`, a `works` array,
a complete `dependency_edges` array, and an `evidence` array. Each edge has
`from`, `to`, `relation`, and boolean `required`; endpoints must exist and the
required graph must be acyclic. Relation strings, including `split-child` and
optional edges, are preserved. Explicit graphs must include every contract
`required_dependencies` edge as required `requires`. Older hand-authored manifests
without the array retain contract-only semantics; current exports always include
the complete graph. Activation compares the full stored set. Each work requires explicit `id` and `external_key`, and
an activatable manifest also requires a valid `awr-team-contract-v1` `contract`.
An optional `contract_hash` must recompute. Dependencies must name works in the
manifest and form an acyclic graph. Import into an existing project must cover
its existing work identities without remapping them. No local claim becomes a
Team lease.

Each evidence item requires `id`, `work_id`, `contract_hash`, `evidence_kind`,
and an object `payload_json`. Optional `input_digest`, `output_digest` and
`execution_result_digest` preserve their separate meanings. `artifact_bytes`
is a JSON byte array; when supplied, its SHA-256 must equal `output_digest`.
Missing bytes stay visible in the saved validation report and prevent activation.
There is no invented default work. Imported trust is always `caller_asserted`;
original trust, actor, execution and artifact identities remain in the immutable
manifest/source reference as provenance, never as live target-project authority.
New local artifact identities and evidence digests are computed on load. Historical
material needs new local execution/review verification to meet strict completion.

Call `freeze`, `load`, then `activate`. Load validates and stores the exact manifest,
report, source snapshot, contracts, dependency edges and evidence bytes. Its retry
identity is `(tenant, project, import key, canonical manifest hash)`. Only an exact
retry returns the old job without another event. Activation verifies the report,
stored projection, evidence and actual claims/recovery/executions in its own
transaction before switching the active snapshot. The boolean unknown hint can
veto activation, but `false` cannot override database facts. Metadata-only legacy
manifests can be staged but cannot activate. Already-staged incomplete imports
remain unavailable pending administrative repair/recovery; this API does not
silently drop missing material or overwrite an existing evidence identity.

Backup registration records a verified logical inventory, not a physical database
backup. Physical snapshot/restore remains the operator's responsibility. The
inventory binds the schema, active source, all source content/projections and all
artifact bytes. Requested source/artifact digests must exist. Restore rechecks
that inventory; a caller's `artifacts_present=true` is insufficient. Legacy backups
without an inventory cannot pass verification. A failed integrity check for an
existing backup records a blocked run, leaves the project degraded and revokes old
authorization. Repair the physical materials before retrying restore.

Successful restore changes the coordinator epoch, revokes active claims, interrupts
sessions, advances work fences, marks unfinished executions unknown and reservations
unknown, fails pending/sending dispatches, and leaves work recovery-blocked. New
execution admission checks both the recorded epoch and recovery flag. Credentials
are tenant-scoped in V1, so restoration conservatively revokes tenant credentials;
operators must account for other projects using those credentials.

**Lock order (WS-023):** writers that need multiple row locks follow a fixed order so opposite presentation orders cannot deadlock: project barrier (`workstream_modes` admission, then `projects`) → auth/mainline → graph coordination → sorted tasks (`work_runtime` by work id) → sorted resources (`resource_reservations` by kind/key/worktree) → receipts/events. Claim rows are task-scoped: lock the owning work runtime before the claim. Freeze, import and restore keep the project barrier for the whole transition; narrowing ordinary writers to a shared project lock must not reorder import/restore lifecycle events or let an old epoch regain authority. Long work runs outside these locks. Helpers live in `awr_team_pg::lock_order`. On the personal SQLite store, acquire the cross-process source lock before `BEGIN IMMEDIATE` writer transactions.

**Resource boundary:** a database cannot retract commands already delivered to an
offline resource. Install every returned `fencing_barriers` entry at each resource
before clearing recovery state. `ReferenceRunner::install_recovery_barrier` persists
a project-wide coordinator generation under the same OS lock held throughout
resource effects. Each delivery carries the epoch from its execution row. After
installation, **all other epochs are rejected regardless of numeric fence**,
including delayed tokens equal to or larger than the restored counter and work
identities created after the backup. Numeric fences order commands only within
the accepted epoch. Epochs never select separate ledgers. An empty historical
project returns a project barrier with empty scope/work identity too.

Barrier installation is a privileged recovery-controller operation, not a delivery
operation: verify it belongs to the current restore (`require_epoch`), retain the
resource journal across process restarts, and install before admitting work. Normal
delivery cannot rotate a persisted generation. Previously observed generations
remain retired and cannot be reinstalled. Missing delivery epochs fail closed;
legacy numeric-only work ledgers need an explicit barrier before reuse. Reference
Runner serializes effects within a project to keep this admission boundary simple.
If any resource is unavailable, retain recovery blocks until it acknowledges the
barrier; never discard its journal to make a token pass. Import activation also
changes the epoch, so an existing resource needs an authorized generation barrier
at that cutover before new deliveries can run. Confirm uncertain effects and use
the authorized reconciliation path; do not replay the pre-restore outbox. A
completed restore run proves inventory verification and coordinator isolation,
not resource acknowledgement or business completion.

**Old source artifacts:** pre-schema-9 ingestion stored a manifest digest but NULL
artifact content and a file-total length. Run the explicit administrative
`ImportStore::repair_source_artifacts(tenant, project)` after upgrading such a
project, before registering a new backup. Under the project lock it reconstructs
manifest bytes from each linked snapshot, verifies every original file digest and
length, the entire manifest and existing artifact digest/identity/state, then
fills content and corrects byte length. All repairs and their lifecycle event are
one transaction, followed by full inventory validation before commit. A repeat is
a no-op. Missing or corrupt original materials, ambiguous artifact ownership and
unrelated missing artifacts fail closed; re-ingesting a new source is not a repair.

Reproduce the historical integration checks (isolated fixtures only):

```sh
# Requires an already available postgres:17-alpine Docker image. Creates its own
# unique container; pg_basebackup and the restored instance run only there.
cargo test -p awr-team-pg --locked --features pg-tests --test pg_physical_restore -- --ignored

# Compile the real pre-fix implementation with a synthetic ingestion driver.
# Use a new empty temporary directory, not an existing checkout.
baseline=$(mktemp -d)
git archive 41cde74677948e5ef7aab0f759083abb8ecd7e04 | tar -x -C "$baseline"
cp crates/awr-team-pg/tests/fixtures/baseline_ingest.rs "$baseline/crates/awr-team-pg/examples/baseline_ingest.rs"
cargo build --manifest-path "$baseline/Cargo.toml" --locked -p awr-team-pg --example baseline_ingest
AWR_TEAM_BASELINE_INGEST_BIN="$baseline/target/debug/examples/baseline_ingest" \
  cargo test -p awr-team-pg --locked --features pg-tests --test pg_import baseline_real_ingest_upgrade_and_repair -- --ignored
```

The second test uses the normal loopback-only `AWR_TEAM_TEST_DATABASE_URL`
fixture, creates its own unique database, invokes baseline ingestion at schema 8,
then preserves those rows through schema 9 and verifies repair/backup plus a
corrupt-source negative control. Both special tests are explicit opt-ins; normal
PG regression also covers the legacy persisted shape and repair transactionality.

Freeze, load, activation, backup and restore write lifecycle events and project
revisions in the same transaction. Event failure rolls back the entire transition.
Schema 9 quarantines old `import_jobs.project_id=NULL` rows behind project RLS and
rejects new unbound jobs. Existing non-null jobs must reference a real project;
repair invalid legacy project references before migrating. Pre-9 executions have
no verified coordinator epoch and cannot obtain new accept/start admission.
