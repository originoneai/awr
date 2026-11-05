# Team Web entry (WS-044)

AWR Team's operator HTTP/MCP service rejects browser `Origin` on the classic
`/v1/projects/*` and MCP surfaces. That remains true. WS-044 adds a **designed
Web entry** under `/v1/web/*` so a self-hosted browser UI can collaborate without
embedding an admin bearer in page JavaScript or "just opening CORS".

## How it differs from MCP bearer auth

| | MCP / classic HTTP | Web entry (`/v1/web/*`) |
|---|---|---|
| Credential in browser | No (native clients) | No (HttpOnly cookie only) |
| Transport proof | `Authorization: Bearer` | Session cookie after one-time exchange |
| `Origin` | Always rejected | Allowlisted via `allowed_web_origins` |
| CSRF guard | N/A (no browser) | Required `X-AWR-Web: 1` |
| Logout / revoke | Credential revoke (access.*) | `/v1/web/logout`, `/v1/web/session/revoke` |
| Writes | Shared command store + idempotent receipts | **Same** store, authz, receipts |

The login body may carry a bearer **once** to the server. The response sets
`awr_web_session` (`HttpOnly; SameSite=Strict; Path=/v1/web`). Subsequent
query/command/access calls use the cookie; the page must clear any bearer field
from the DOM after submit.

## Enable the entry

```toml
version = 1
listen = "127.0.0.1:9908"
allowed_hosts = []
allowed_web_origins = ["http://127.0.0.1:7390"]

[[projects]]
key = "example"
tenant_id = "tenant-a"
project_id = "project-a"
```

Without `allowed_web_origins`, `/v1/web/*` returns `WebEntryDisabled`. Classic
routes still deny any `Origin`.

## Observer workspace

The [Observer workspace](../../../tools/observer/README.md) serves the browser observation UI.
Its bridge lives in `tools/observer/server/team-bridge.js` and forwards authorized requests to the
server's Web entry using the member's cookie session. Cookie paths are rewritten from `/v1/web`
to `/api/team` so the browser sends them to the workspace's bridge routes.

Start the built workspace with `--team-url http://127.0.0.1:9908`. Use `--team-public-url`
when the member-facing MCP service has a different public address. Demo mode uses synthetic fixtures.

Task planning, assignment, claims, review and delivery run through connected Agents. The workspace
shows the resulting facts and their provenance. Its login and connection controls use the existing
Web session entry.

Member, role and grant management continues to use `access.inspect`, `access.preview`, `access.apply`
and `access.outcome` through authorized service clients. The retired Inspector member editor has
been removed. The current Observer member page indicates its availability explicitly.
