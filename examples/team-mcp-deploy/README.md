# Team MCP deploy examples (AWR-TMCP-041)

Templates for an independent Team service deploy and two WS-024 named clients.
These files change nothing until an operator copies them into a real host or
client config.

| Path | Purpose |
| --- | --- |
| `team-service.toml.example` | Loopback `awr-server serve` config |
| `env/owner.env.example` | Ops-only owner DB URL (migrate / access / backup) |
| `env/app.env.example` | Restricted app DB URL for `serve` |
| `member-handoff.example.md` | What to send members (MCP URL, claim method, repo) |
| `clients/codex_cli.mcp.toml.example` | Codex remote Team MCP |
| `clients/claude_code.mcp.json.example` | Claude Code remote Team MCP |

Operator guide: [`docs/dev/reference/team-deploy-pack.md`](../../docs/dev/reference/team-deploy-pack.md).
Scripts: [`scripts/team-deploy/`](../../scripts/team-deploy/).

**Do not** commit real bearers, DB passwords, or copies of runtime ledgers.
