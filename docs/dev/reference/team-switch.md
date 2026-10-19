# Team runtime switch

Team code is built into every AWR binary. A runtime switch, **off by default**,
decides whether a personal installation offers Team surfaces.

## What the switch controls

| Surface | Switch off | Switch on |
|---|---|---|
| `awr team ...`, `awr remote ...` | Exit 1 with `Unsupported`: "team features are disabled; enable them with ..." No state is created. | Unchanged behaviour. |
| Local/shared `awr-mcp` tools `awr_team_handoff`, `awr_team_review` | Not advertised in `tools/list` or domain manifests; a direct call returns an "invalid request" error with the same message. | Advertised and callable as before. |

`awr-server` and the Team PostgreSQL service are Team deployments and are not
affected by this switch.

## How it is resolved

1. `AWR_TEAM_ENABLED` environment variable, if set: `1`, `true`, `yes`, `on`
   enable; `0`, `false`, `no`, `off` or empty disable. Any other value is
   rejected (the value is not echoed).
2. Otherwise the configuration key:
   - CLI and single-project `awr-mcp`: `.awr/project.toml`

     ```toml
     [team]
     enabled = true
     ```

   - Shared `awr-mcp --registry`: top-level `team_enabled = true` in the
     registry file.
3. Otherwise off.

The single-project MCP server falls back to off when `.awr/project.toml` cannot
be read, so `awr_project_status` can still explain how to repair the project.
`awr-mcp` reads the switch once at startup; restart it after changing the switch.
