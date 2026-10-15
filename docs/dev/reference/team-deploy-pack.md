# Team MCP independent deployment access pack (AWR-TMCP-041)

This pack is the operator path for a **pinned source/artifact version** of the
Team workstream HTTP/MCP service on an independent host with its own PostgreSQL.
It complements, and does not replace:

- [Team workstream HTTP/MCP service](team-workstream-service.md)
- [Team PostgreSQL store](team-postgres.md)
- [Team operator access](team-operator-access.md)
- [Source publish entrypoint](team-publish-entrypoint.md)
- [Member handoff](team-member-handoff.md)

Examples and scripts: [`examples/team-mcp-deploy/`](../../../examples/team-mcp-deploy/),
[`scripts/team-deploy/`](../../../scripts/team-deploy/).

## What this pack is / is not

| In scope | Out of scope |
| --- | --- |
| Fixed binary + schema version deploy | Auto-scaling / multi-region HA |
| Same-host private-net PG **or** separate intranet PG | Public PostgreSQL on the internet |
| HTTPS termination at the deploy boundary | Built-in TLS on the AWR listener |
| Restricted **app** DB role for `serve` | Giving members DB accounts |
| Owner ops for migrate / first admin / backup | Wrapping personal `awr team` CLI placeholders as remote Team MCP |

Personal CLI `awr team command` / `awr-server command` validation stubs remain
`Unsupported` for live dispatch ([team-access.md](team-access.md)). Members use
the **Team Streamable HTTP MCP** tools (`awr_team_query` / `awr_team_command`),
not those placeholders.

## Fixed version pin

1. Choose a reviewed Git tag or commit of this repository (or a release artifact
   built from it). Record `git describe --always --dirty` and Cargo
   `workspace.package.version` (currently `0.4.0` on develop tips).
2. Build the server with PostgreSQL TLS support when the DB URL will use
   `sslmode=require`:

```sh
cargo build --locked -p awr-server --release --features tls
```

3. Record the binary digest and the expected Team schema version
   (`awr_team_pg::EXPECTED_SCHEMA_VERSION`, currently **31** on this tip).
4. Deploy **that** binary. Do not hot-swap an unreviewed develop checkout onto
   the runtime host. Candidate updates are verified first, then
   [directed-replaced](team-publish-entrypoint.md#runtime-vs-develop-versions) with a
   matching rollback basis.

## Topology

```text
[Coding Agent clients] --HTTPS--> [reverse proxy] --HTTP loopback--> [awr-server serve]
                                                                        |
                                                                  app-role PG URL
                                                                        v
                                                         [PostgreSQL private net]
```

- Default listener is loopback (`127.0.0.1`). Non-loopback listeners **require**
  explicit `allowed_hosts` and operator TLS termination
  ([team-workstream-service.md](team-workstream-service.md)).
- PostgreSQL may share the host on a **private** address / Unix socket, or sit
  on a separate intranet instance. Do **not** publish PG to `0.0.0.0` with
  fixed credentials. The compose file under `docker/team-postgres.yml` binds
  `127.0.0.1` only and is a local verification aid, not production.
- Members never receive the database URL, owner role, or ledger-directory write
  access. See [member handoff](team-member-handoff.md).

## Connection identities

| Identity | Env / use | Privileges |
| --- | --- | --- |
| **Owner / ops** | `AWR_TEAM_DATABASE_URL` for `migrate`, `check`, `access`, backup/restore | Schema owner; never for long-running `serve` |
| **App / runtime** | Separate URL for `awr-server serve` | Restricted application role (no `BYPASSRLS`, not table owner) |

```sh
# Owner (ops shell only — not the service unit)
export AWR_TEAM_DATABASE_URL='postgres://awr_owner:...@127.0.0.1:5432/awr_team?sslmode=require'

# App (service unit / container)
export AWR_TEAM_DATABASE_URL='postgres://awr_app:...@127.0.0.1:5432/awr_team?sslmode=require'
```

### Database TLS build + connection params

- **Build:** `--features tls` on `awr-server` (passthrough to `awr-team-pg/tls`).
  Without the feature, `sslmode=require` fails explicitly instead of downgrading.
- **URL `sslmode`:** driver accepts `disable`, `prefer`, `require`. Prefer
  `require` for any non-loopback or multi-tenant host. Libpq spellings
  `verify-ca` / `verify-full` are **rejected** by the driver; use `require` with
  webpki roots (and private CA only via the documented test harness, not as a
  production silent downgrade).
- **Local-only smoke:** `sslmode=disable` on loopback is acceptable for operator
  labs; never ship that as the member-facing default.

## Minimal init (owner)

Scripts wrap the same entrypoints (see [`scripts/team-deploy/`](../../../scripts/team-deploy/)):

```sh
# 1) Migrate as owner and grant the app role
./scripts/team-deploy/migrate.sh --app-role awr_app

# 2) Verify schema matches the binary
./scripts/team-deploy/version-check.sh

# 3) First project admin (owner `access` apply plan)
./scripts/team-deploy/first-admin.sh --plan /secure/first-admin.plan.json --preview-only
# review state_digest + plan_digest from preview, then:
./scripts/team-deploy/first-admin.sh --plan /secure/first-admin.plan.json \
  --request-id first-admin-1 --expected-state <state_digest> --expected-plan <plan_digest>

# 4) Serve with the **app** URL and a reviewed config
AWR_TEAM_DATABASE_URL="$APP_URL" awr-server serve --config /etc/awr/team-service.toml
```

Config skeleton: [`examples/team-mcp-deploy/team-service.toml.example`](../../../examples/team-mcp-deploy/team-service.toml.example).

Activate an approved `workstreams.json` source bundle before members claim work
([workstreams.md](workstreams.md), [team-publish-entrypoint.md](team-publish-entrypoint.md)).

## HTTPS and host bind

1. Bind `awr-server` to loopback (recommended) or a private interface with
   non-empty `allowed_hosts`.
2. Terminate TLS at nginx / Caddy / cloud LB. Forward to the loopback port.
3. Advertise to members only the **HTTPS MCP URL**:
   `https://team.example/v1/projects/<alias>/mcp`.
4. Reject browser `Origin` by default; this is not a cookie/CORS login surface.

## Backup / restore / disaster recovery

Ops-only (`awr-server access` as owner). Application MCP cannot reach these:

| Step | Command family |
| --- | --- |
| Logical backup manifest | `awr-server access backup-create` |
| Inspect | `backup-inspect` |
| Guarded restore preview / apply | `backup-restore-preview` / `backup-restore-apply` |
| Rebuild preview / apply | `backup-rebuild-preview` / `backup-rebuild-apply` |

Wrapper: [`scripts/team-deploy/backup.sh`](../../../scripts/team-deploy/backup.sh).

Restore mints a new coordinator epoch, revokes restored credentials, and fails
pending outbox rather than replaying it ([team-postgres.md](team-postgres.md)).
Keep matching **program + schema + config + source** snapshots for rollback
([team-publish-entrypoint.md](team-publish-entrypoint.md)).

## Version verification

```sh
./scripts/team-deploy/version-check.sh
# equivalent:
AWR_TEAM_DATABASE_URL=... awr-server check
```

`serve` refuses an incompatible schema without migrating. Candidate upgrades:
migrate explicitly as owner with `--app-role`, run `check`, smoke MCP
`capabilities`, then cut over the binary.

## Related client guides

- [Team MCP · Codex (`codex_cli`)](../integrations/team-mcp-codex-cli.md)
- [Team MCP · Claude Code (`claude_code`)](../integrations/team-mcp-claude-code.md)
