# A small AWR source project

Copy this directory to a scratch project. The Markdown goals and rules plus the
YAML work item are synthetic inputs; initialization leaves their bytes unchanged.

```sh
awr --project /path/to/copy init --manifest project.toml
awr --project /path/to/copy init --manifest project.toml --accept
awr --project /path/to/copy status
awr --project /path/to/copy ready
awr --project /path/to/copy context compile --work EXAMPLE-001 --goal 'goal#demo'
```

The first command previews the source mapping. Acceptance creates the local
manifest and SQLite database, indexes sources and adds runtime ignore entries.
Edit the copied work ledger, then use `source scan` to inspect changes and
`source reindex` to refresh them. Reindexing unchanged sources preserves revisions.
CLI reads also check source freshness. `--json` returns structured reports.

Try the [L0 session workflow](../../docs/dev/integrations/session-workflow.md) or the [CLI lifecycle example](../codex/README.md) for session continuity. This is a
local demonstration fixture, not a completed real-client business scenario.
