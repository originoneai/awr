# Local Project Hub

The development native payload embeds Project Hub and Project Progress Overview.
Both npm and Python distributions carry the same native service and page assets.
Python installations use their native executable to serve the pages.

The published 0.6.0 packages predate this feature. Use a build of this source until
a release containing the hub is published.

## Open your projects

```sh
awr overview
```

The command starts or reuses the current user's service and opens its address.
Use `awr overview --no-open` to print the address, or `awr overview --json` for a
machine-readable service receipt.

The landing page lists registered projects as cards, ordered by the latest AWR
use. Search matches project names and directory paths. Each card shows its name,
directory and last-use time. Select a card to open the project's three progress
views, then use **Back to projects** to return.

The service first tries port 7381 on `127.0.0.1`. An occupied port selects an
available loopback port. The service receipt records the actual address. Recovery
tries that recorded port first, preserving existing links when it is available.

## Register projects

Successful initialization and subsequent CLI use register an initialized project.
The installed MCP server registers its bound projects and updates their last-use
time on project calls. Existing projects appear as they are used with this version.
You can also register known directories explicitly.

```sh
awr overview register /absolute/path/to/alpha /absolute/path/to/beta
awr overview projects --json
```

Registration checks the canonical directory and the actual AWR project identity.
Aliases of the same directory and identity share a card. A replacement project at
the same path receives a separate identity-bound entry. The prior entry stays
visible as unavailable. Moved or unreadable directories also remain visible.

The catalog stores project identity, name, root and usage timestamps. Project
goals, tasks, sessions and evidence remain in their existing project sources and
runtime. Progress views read the selected project through official CLI JSON.

## Background service lifecycle

Ordinary AWR CLI invocations automatically start the local service when needed.
Installed MCP project calls do the same. A service terminated by the user or OS is
started again on the next invocation. Startup coordination and the service's
lifetime use OS file locks. A health response binds the running instance to the
private service receipt. Concurrent callers reuse that instance.

```sh
awr overview status --json
awr overview start --json
awr overview stop --json
```

`status` observes the current service. `stop` shuts it down and retains the
catalog. The next ordinary AWR invocation can start it again. Browser opening is
explicit through `awr overview`.

Set `AWR_LOCAL_WEB=0` to disable automatic catalog observation and startup in test
runs or embedded hosts. Explicit `overview` commands remain available. The local
service's own CLI readers use this setting to keep reads independent of startup
and last-use observation.

## State and boundaries

State is stored under `AWR_LOCAL_HOME` when that absolute path is configured.
Otherwise Windows uses `%LOCALAPPDATA%/AWR`, an absolute `XDG_STATE_HOME` uses its
`awr` child, and the fallback is `$HOME/.local/state/awr`.

The directory contains `projects.json`, `service.json`, coordination locks and
`service.log`. It belongs to the current OS user. The HTTP service binds to
loopback and validates request hosts and origins.

Project selection is explicit in every progress request. The browser URL carries
an identity-bound catalog key. Opening another project navigates to a separate
page, keeping each document's request binding fixed. Available project data and
project identity are checked at the request boundary.

## Source development

```sh
cargo build -p awr-cli -p awr-mcp --locked
./target/debug/awr overview
```

The Node source viewer is available for a directly bound project and explicit
display configuration.

```sh
node tools/project-progress-overview/server.js --project /absolute/path/to/project
```

Native package verification installs both real package artifacts in isolated
directories, checks automatic startup and catalog registration, fetches the
bundled hub and overview assets, and confirms that subsequent calls reuse the
running instance. Native integration tests also exercise concurrent startup and
hard-kill recovery with an occupied default port.
