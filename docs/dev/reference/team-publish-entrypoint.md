# Team source registry publish entrypoint (AWR-TMCP-041)

The Team project's **authoritative source registry** is maintained through a
**single team publish entrypoint**. Developers contribute via Git PRs; they do
not push ad-hoc full ledgers into the running service, and they do not copy the
runtime PostgreSQL database onto develop branches.

## Single publish path

1. Developer opens a PR against the agreed base with source / contract changes.
2. Maintainer reviews the PR (exact paths, graph integrity, acceptance text).
3. Authorized planning publish (MCP `awr_team_planning_publish` / HTTP
   `planning.publish`) writes the approved candidate through the registered sole
   source. Optional `activate` uses that registered source only — **client paths,
   URLs, and SQL are refused**.
4. Import / activate barriers (WS-022/023/032) keep unactivated candidates out of
   the live executable contract. Failed activation retains the previous
   `active_snapshot_id`.

Daily member clients **read** the activated contract through
`work.prepare` / controlled `source.content`. They do not obtain ledger-directory
write access on the server.

Owner/operator bootstrap of the first source bundle still follows
[team-workstream-service.md](team-workstream-service.md) and
[team-postgres.md](team-postgres.md) (ingest → approve → activate). After the
project is live, treat MCP/HTTP `planning.publish` (plus the registered source
writer) as the **only** normal publish entrypoint.

### Reviewed planning policies

Task creation and edits write the approved `completion_policy` into the sole
source. Recompiling and activating that source retains the reviewed policy;
it must not silently fall back to the independent-review default.

Planning candidates without explicit dependency policies retain the closed
`awr-team-planning-v1` wire format and original digest material. A candidate
with `dependency_acceptance` in a before/after task uses
`awr-team-planning-v2`. Explicit `hard_rules` or `verification_requirements` in a
before/after task select `awr-team-planning-v3`. The service reports all three
supported candidate codecs; omission preserves V1/V2 serialization and digests.

For example, a downstream task may explicitly select this assurance for a
required predecessor:

```json
{
  "required_dependencies": ["API-1"],
  "dependency_acceptance": {
    "API-1": "agent_reviewed_caller_asserted_reconciled"
  }
}
```

This is an excerpt of a task draft, not a complete publish request. Omit the map
to retain the existing source policy. A present map is a nonempty, typed,
explicit replacement; include the exact current map in the before-draft when
replacing one. Null, duplicate keys, unknown modes, and policies for nonrequired
predecessors are rejected. Removing a predecessor while omitting its retained
policy is also rejected.

Preview exposes policy changes and their review requirements. Editing a
candidate changes its digest and clears prior approval. Existing independent
delivery review cannot be downgraded through this planning path. Selecting
dependency assurance grants no review permission and creates no completion or
human-acceptance receipt; the upstream delivery still needs the selected
evidence and reconciliation.

### Required execution context in planning

Use the structured task fields to publish execution constraints, rather than
putting them only in acceptance prose:

```json
{
  "hard_rules": ["Preserve existing issue identities and recorded history"],
  "verification_requirements": ["Run the focused persistence and API regressions"]
}
```

Both fields are optional lists of nonblank strings. Omission retains the current
source field; null is rejected. A present list explicitly replaces the field,
and `[]` explicitly clears it. When replacing an existing field, include its
exact source list in the before-draft. Preview exposes the changes and requires
execution-contract review; any edit clears previous approval. These fields are
persisted through source writeback and contract recompilation. They do not
change the published work-contract codec or grant execution permissions.

After activation, use `work.prepare` to confirm the actual context is complete.
Clearing required hard rules leaves that context incomplete and keeps the
existing execution gate in force; planning does not fabricate a complete task.

## Runtime vs develop versions

| Concern | Runtime host | Develop / PR branches |
| --- | --- | --- |
| Service binary | Pinned digest from a verified candidate | Workspace under development |
| Team schema | Matches that binary's `EXPECTED_SCHEMA_VERSION` | May advance ahead; not auto-pushed |
| Source registry | Activated snapshot in PG + registered sole source | PR diffs only |
| Database | Production / staging PG | Ephemeral lab DBs — **never** a copy of runtime PG |
| Ledgers | Server-authoritative runtime state | Do **not** export a full runtime ledger and commit it back |

### Update candidates (directed replace)

1. Build and test a candidate from the reviewed tip (unit + schema migrate on a
   disposable DB + MCP `capabilities` smoke).
2. Record binary digest, schema version, config hash, and active source
   snapshot id as the **rollback basis**.
3. Take an owner backup manifest (`scripts/team-deploy/backup.sh`).
4. Migrate as owner if the schema advances; `awr-server check` must pass.
5. Directed-replace the runtime binary / unit to the candidate (one writer).
6. Verify HTTPS MCP reachability and a member `capabilities` query.
7. On failure, restore the matching program + schema + config + source set from
   the rollback basis. Do not “partially” mix an old binary with a newer schema.

## Forbidden developer practices

- Copying runtime `AWR_TEAM_DATABASE_URL` data into a laptop develop checkout.
- Pushing an old **full** work-ledger dump back onto the service to “sync”.
- Bypassing publish with arbitrary SQL, `status=done` tools, or server path edits.
- Shipping owner credentials or ledger-directory mounts to coding-agent members.

## Member-visible surface

Members receive only what [team-member-handoff.md](team-member-handoff.md)
lists. Source contributions remain ordinary Git PRs against the public/internal
repo URL you hand them — not direct registry writes.
