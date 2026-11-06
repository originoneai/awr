# AWR Project Progress Overview

A local project hub and viewer for AWR project progress and dependencies.

The development native CLI embeds the hub and the three views. Open `awr overview`
to search this user's registered projects, select a card, and return to the list
from its overview. Projects are ordered by their latest AWR use. The native
service and assets ship together in the npm and Python payloads built from this
source. Published 0.6.0 packages predate this feature.

See the [local hub guide](../../docs/dev/reference/local-project-hub.md) for
registration, automatic single-instance startup, recovery and state locations.

| View | What it shows |
| --- | --- |
| Project nodes | Work grouped by display lanes, with status, blockers and claim summaries |
| Mainline dependencies | Dependencies within configured panels and their critical paths |
| Collaboration swimlanes | Unfinished work and cross-lane dependencies grouped by recorded milestones or mainlines |

## Start the bundled hub

```sh
awr overview
```

Use `awr overview --no-open` to print its address. Initialize or use an existing
project with AWR to add it automatically, or register known roots explicitly.

```sh
awr overview register /absolute/path/to/project
```

## Start the source project viewer

Use Node.js 18 or later and an installed `awr` command.

```sh
node tools/project-progress-overview/server.js --project /absolute/path/to/project
```

The tool opens at <http://127.0.0.1:7381>. From the tool directory, `./start.sh /absolute/path/to/project`
uses the same entry point. The npm binary name is `awr-project-progress-overview` when linked locally.

For a synthetic project, run `node server.js --demo` from this directory.
A missing or unusable CLI also selects the visibly labelled demonstration mode.

| Option | Purpose |
| --- | --- |
| `--project <dir>` | AWR project directory, defaulting to the current directory |
| `--port <port>` | Local port, defaulting to 7381. Use 0 to select a free port |
| `--map-config <file>` | Display lanes, dependency panels, milestones and stale threshold |
| `--demo` | View the built-in synthetic project |
| `--no-open` | Start without opening a browser |

For Team project observation and browser login, use the [Observer workspace](../observer/README.md).

## Explore and share

- Select a card or table row to inspect recorded status, ownership, claims, blockers and dependencies.
- Hover or focus a card to highlight its dependency chain. Follow predecessor and successor buttons in the detail panel.
- Fold lanes and groups, search, filter by status, zoom or pan. Refresh retains those view choices.
- Download HTML to share a self-contained snapshot with the same interactions. A viewer without scripts still shows the diagrams and table.
- Download snapshot JSON to retain the exact data and its fingerprint. Offline exports show the state at their generation time.
- English and Simplified Chinese are available from the language selector. Query parameter `lang=en` or `lang=zh-CN` takes precedence.

The work-item table and graph detail panel support the three views. All interactions support keyboard use.

## Data and display configuration

Native progress views read one bounded, coherent snapshot of the local SQLite
projection. The source viewer uses the same `overview snapshot --json` command when
the CLI advertises `local.progress_snapshot`. Tasks, dependencies, claims, sessions
and runtime findings come from the same project revision.

**Refresh map** reads recorded state without synchronizing source files. **Sync
project sources** explicitly refreshes the source projection before reading. The
page distinguishes these freshness conditions. Full source/artifact hashing and
database-integrity checks remain available through `awr doctor`.

Older CLIs retain the compatible `work graph`, `nav --cached`, search, session,
event and `doctor` extraction. Those answers must agree on a revision, with bounded
retries on concurrent changes. **Recorded state only** remains available there.
Unavailable fields remain explicit in the diagram's notes.

Display lanes group work for this view. Their labels and prefixes can be configured in
[project-map.config.json](examples/project-map.config.json). Without configuration, work-key prefixes
supply the lane groups. Without `explore_lanes`, the swimlane view uses recorded milestone membership
when available and otherwise uses the same mainline groups as the project view. This adds no CLI reads.
Explicit `explore_lanes` selects milestones whose membership is resolved with `nav --cached --milestone`.
An empty list disables the swimlane view and milestone reads. Display grouping leaves recorded task
ownership and milestone values intact.

Broad mainline dependency layers wrap into bounded rows while keeping upstream work above its
consumers. Long edges follow outside gutters. The table and source notes use the same supporting
panels in the live view and standalone export.

The `stale_days` display threshold defaults to 7 days for in-progress work without recorded activity.

## Export from the command line

```sh
node export-map.js --help
node export-map.js --project /absolute/path/to/project --out /tmp/project-progress.html
```

The exporter and browser share the snapshot, renderer and interaction modules.

## Verification

```sh
node --test test/*.test.js
```

Coverage includes snapshot fingerprints, revision consistency, CLI bridge boundaries, frozen rendering
fixtures, detail navigation, folding, filtering, zoom, keyboard use and offline export.
