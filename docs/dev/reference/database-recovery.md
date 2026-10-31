# Database health and retained runtime data

Source files are authoritative for project facts. SQLite also holds state that
those files do not contain: sessions, claims, checkpoints, runtime events,
artifact registrations and mutation attempts. Reindexing source files refreshes
their projections and derived search; it preserves retained runtime ownership and
old receipts, apart from the claims on finished work described below. Initializing a new database from the same source files cannot
reconstruct missing runtime history.

## Inspecting a database

```sh
awr --json doctor --database-only
awr --json doctor
```

Doctor opens the database read only and does not migrate, reindex, apply pending
mutations, expire claims or delete orphan files. Database-only mode checks SQLite
integrity, foreign keys, schema version, migration identities and owned schema
definitions. Full diagnosis also inspects the selected sources and runtime/file
bindings. Active sessions are informational; their age does not prove process
death. Missing dependencies, dangling edges, unavailable sources, interrupted
saves and damaged or unregistered artifacts remain explicit findings.

`schema_issues` identifies missing or changed owned tables, columns, indexes and
triggers. Definitions must match the bundled migrations, including immutable
event guards and the unique active-claim index. Extra diagnostic objects can
coexist with the owned schema. This comparison does not infer semantic equivalence
for manually rewritten SQL, repair those definitions or emit their SQL bodies.

If a malformed reference prevents SQLite from running its foreign-key check,
`foreign_key_check_error` is present and the database is unhealthy. In that case,
zero reported violations is only the observed count, not a successful check.
Unreadable database pages or a foreign database produce explicit errors.

Normal read/write opens require the current owned schema. `Store::open` can
upgrade supported older schemas; it validates the base before startup changes,
rechecks it under the writer lock, and verifies the result before committing the
upgrade. A rejected catalog or migration SQL failure cannot leave new tables,
catalog rows or a new schema version committed. SQLite connection/WAL setup is
distinct from the migration transaction. Doctor never invokes that upgrade path.

## Claims and sessions of work the source calls finished

A claim is runtime state and the status of a work is a source fact. When a command that refreshes the sources shows that
a work with an active claim is now `completed` or `cancelled` (its file was edited directly, or it was finished elsewhere),
the claim is released, and the session that is left without a claim is closed as `interrupted` when it was bound to that
work. `source reindex`, the reads that refresh the sources (`status`, `ready`, `work show`, `context compile` and the
like), `session start` and `work claim` do this after the projection is committed, in a runtime transaction of its own
under the live project revision. MCP read tools never do it. A write whose caller revision the refresh has moved is refused
as before, after the release, and the caller reads again.

The reply of a read or of `source reindex` carries `reconciliation` with the `released_claim_ids`, the
`interrupted_session_ids` and what was `skipped` with its reason; nothing is said when there was nothing to do, so a
second call changes nothing and reports nothing. Each release has a receipt: `session.interrupted` (or `claim.released` /
`claim.expired` when the session goes on holding a claim on other work) with `reason: source_terminal` and the
`source_revision` and `source_fingerprint` that were seen. Nothing here writes the source, evidence or the status of a
work, and a session closed this way is `interrupted`, never `ended`: completion belongs to the work, with its evidence. When
the work is reopened in the source it can be claimed again.

It acts only on works that were open when the refresh began, so a claim that already stood on a finished work stays for the
explicit clean-up and the doctor, and only when the new projection is fresh. It leaves alone, and says why, a session with a
checkpoint save in flight (`checkpoint_save_in_flight`), a change waiting to be applied (`change_waiting_to_be_applied`), a
supervised execution that has not ended (`execution_not_ended`) and a claim on a branch other than the current one
(`claim_on_another_branch`); a busy database or a changed runtime is also reported and never fails the command.

## Backup and recovery

The native [`runtime` snapshot commands](runtime-snapshots.md) provide coherent
backups, integrity checks, same-root matching restore previews and explicit offline
database restoration. They retain source/configuration copies for verification,
preserve added files and refuse source drift. The broader manual recovery inventory
below also covers external files and schema upgrades outside automatic restore.

For a recoverable project snapshot, retain all of the following together:

- A coherent SQLite backup of `.awr/state.db`, made with SQLite's backup API or
  equivalent tooling. Copying only the main file while writers are active can omit
  committed WAL records.
- The matching source files, Git revision, `.awr/project.toml`, authorized-root
  configuration and any explicitly authorized external sources.
- Managed artifact files, registered local artifacts outside that directory and
  `.awr/mutations` recovery journals/snapshots referenced by pending operations.

Pause project writers while assembling this set so the database, sources and
external files describe the same recovery point. SQLite's backup API provides a
coherent database image; it does not atomically snapshot the surrounding files.

Restore to a stopped project at its recorded canonical root, retain the displaced
state, then run database-only and full Doctor checks. Database restoration can
recover an artifact registration while its file is still missing; restore the
matching file or keep the finding unresolved. Source reindex is neither runtime
restoration nor a database relocation tool. Review pending mutation journals
before selecting an explicit recovery operation.

Named Doctor repairs require the current project revision, a selected object and
a reason. They preserve source files and unrelated runtime state. Broken database
integrity disables suggested runtime repairs; there is no automatic reconstruction
of missing references or a healthy status inferred from source files alone.

The [recovery matrix](../../../tests/recovery/doctor/README.md) verifies these local
boundaries with temporary projects, real CLI calls and killed live writer
processes. It does not establish machine power-loss or other-platform behavior.
