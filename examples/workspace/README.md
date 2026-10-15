# Two machines, one project

Agents on different machines cannot share a filesystem, so they share bytes
through a store that only needs five object primitives: `HEAD`, `GET`,
`PUT` (with preconditions), `DELETE` and a prefix `LIST`. Cloudflare R2, AWS S3,
Aliyun OSS and MinIO all qualify; the store is not the design.

Copy `remote_workspace.toml.example` to each machine's project root, change
`root` and `host`, and store credentials once per machine:

```sh
awr workspace credential set --stdin <<'JSON'
{"access_key": "...", "secret_key": "..."}
JSON
```

Then the loop is three commands. On the machine that did the work:

```sh
awr --project /path/to/project workspace publish
# optional: awr --project /path/to/project workspace publish --dry-run
```

On the machine that needs to know:

```sh
awr --project /path/to/project workspace sync
awr --project /path/to/project workspace status
```

To stop sharing a path, take it out of `project.track` first, then drop it from
the index. Local files stay:

```sh
awr --project /path/to/project workspace drop --path infra/evidence/dump.json
```

`publish` sends the tracked files whose bytes changed and commits them with one
compare-and-swap on the index, so a killed publish is never half-visible.
`sync` takes the peer's files and any inbound handoffs. A path that changed on
both sides is reported as a conflict and left alone on both sides; it is never
merged automatically.

Once a project has a `remote_workspace.toml`, `awr client hook` also takes what
the peer published when a session starts - before the project's sources are
refreshed, so the context describes the tree as it is now - and
names the paths it moved. Nothing else has to be remembered, and nothing about
that step can fail the session: no config, no credentials, an unreachable store
and a conflict all leave the session open with one line to read. The whole pull
has a ten-second budget, so a store that never answers costs a sentence, not a
session. It also repairs the one entry a store without compare-and-swap can drop
- by registering that entry again, never by publishing the half-edited tree a
session starts with. It pulls and never pushes.

Tracked files stay authoritative on the host that owns them, and `.awr/` - the
runtime database, credentials and the workspace state file - is never
transferred. The design and its measured numbers are in
[docs/dev/reference/workspace-exchange.md](../../docs/dev/reference/workspace-exchange.md).
