# AWR Observer

The Agent-first observation workspace shows project goals, work, delivery evidence, activity and connection state.

## Run from source

Use Node.js 24 or later.

```sh
cd tools/observer
npm ci
npm run build
node server.js --demo
```

The local server listens on <http://127.0.0.1:7390>. For a configured Team service, use
`node server.js --team-url http://127.0.0.1:9908`. Set `--team-public-url` when members connect to a
public MCP address that differs from the proxy's upstream service address. Configure the Team
service's allowed Web origin for this server.

Browser login exchanges a member credential for an HttpOnly session cookie. The workspace uses
that session for authorized reads. Project work continues through the member's connected Agent.

## Service ownership

`server/team-bridge.js` owns the existing `/api/team/*` bridge routes, report mapping and upstream
session handling. `server/observation-contract.js` is the single source for read freshness, ordering
and paging semantics. The Vite application imports that same module through its contract alias.

Bridge regressions and their evidence fixtures live under `test/`. The shared display derivations
remain in `src/core/observation/`.

For local project maps and standalone HTML export, use
[Project Progress Overview](../project-progress-overview/README.md).

## Verification

```sh
npm run typecheck
npm test
npm run build
node --test test/*.test.js
```

Build before server tests so the static production assets are available.
