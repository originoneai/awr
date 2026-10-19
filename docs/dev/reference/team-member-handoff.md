# Team member handoff (AWR-TMCP-041)

What a **project member** receives after deploy. Keep this list short so
operators do not accidentally over-share.

Local Team commands and MCP tools are off by default on personal installations;
see [Team runtime switch](team-switch.md).

## Members receive

1. **MCP address** — HTTPS Streamable HTTP URL for their project alias, e.g.
   `https://team.example/v1/projects/<alias>/mcp`.
2. **Personal credential claim method** — how to obtain their bearer safely:
   - Operator (or project admin after first admin) generates a credential with
     `awr-server access token --credential-id <id> --output /secure/<id>.token`
     (mode `0600`, never overwritten).
   - Deliver the bearer **out of band** (encrypted channel / one-time secret
     store). Register only `secret_hash` in PostgreSQL.
   - Member sets a local environment variable (e.g. `AWR_TEAM_BEARER`) and
     points their client config at that env — **never** pastes the bearer into
     Git, chat logs, MCP tool arguments, or audit-visible fields.
3. **Repository** — Git remote for ordinary PR contributions to source/contracts.

## Members must NOT receive

| Forbidden | Why |
| --- | --- |
| PostgreSQL accounts / `AWR_TEAM_DATABASE_URL` | App and owner DB roles are ops-only |
| Schema-owner / `awr-server access` on the server | Recovery and first-admin stay ops |
| Ledger-directory write access on the host | Registry writes go through the [publish entrypoint](team-publish-entrypoint.md) |
| Raw credentials in MCP messages or shared configs | Secret boundary ([secret-boundaries.md](secret-boundaries.md)) |

## After claim

Configure one of the WS-024 named clients:

- [Codex / `codex_cli`](../integrations/team-mcp-codex-cli.md)
- [Claude Code / `claude_code`](../integrations/team-mcp-claude-code.md)

Example templates: [`examples/team-mcp-deploy/`](../../../examples/team-mcp-deploy/).

Confirm with `awr_team_query` → `{"protocol_version":1,"op":"capabilities"}`
before any mutation. Closing the MCP connection does **not** end a durable
Team session.
