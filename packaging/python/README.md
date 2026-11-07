# AWR

The open-source project delivery platform for people and AI.

AWR 0.7.0 includes the native Rust `awr` CLI and `awr-mcp` server for stdio and shared Streamable HTTP.
It provides command-line tools; context compilation runs locally.

Project Hub and Project Progress Overview are bundled with the native payload.
Run `awr overview` to search local AWR-managed projects and inspect their progress.
The first ordinary AWR command or local Agent MCP connection starts one background
service automatically. After that, open **http://127.0.0.1:7381** directly; running
`awr overview` first is optional. Installation alone does not start a process.
If the port is occupied, `awr overview status --json` reports the actual URL.
The next CLI/MCP use recovers a terminated service without duplicate instances.
See the [local hub guide](https://github.com/originoneai/awr/blob/v0.7.0/docs/dev/reference/local-project-hub.md).

```sh
python -m pip install originoneai-awr==0.7.0
awr --version
awr --help
awr-mcp --help
```

Upgrading an existing installation? Uninstall `agent-work-runtime` with the same
package manager before installing `originoneai-awr`. Do not install both in the
same environment: they share Python files and command entry points. Commands and
project state keep their existing names. The bare PyPI package `awr` is unrelated.
See the [migration guide](https://github.com/originoneai/awr/blob/v0.7.0/docs/dev/release/DISTRIBUTIONS.md#migrate-from-the-previous-package-names).

For an isolated application installation, use `pipx install originoneai-awr==0.7.0` or `uv tool install originoneai-awr==0.7.0`.

The project must be initialized before running
`awr-mcp --project /absolute/project`. Follow the
[project documentation](https://github.com/originoneai/awr).

The distribution targets macOS 15+ arm64 and Intel x64, Linux x64/arm64 with glibc 2.39+ (Ubuntu 24.04
baseline), and Windows x64. Python 3.9+ is required for the launcher. Git is required
for Git-bound work branch operations. SQLite is included in the binaries. Supported
platforms install a prebuilt wheel and do not need a Rust compiler.

The wheel includes source identity, binary checksums and third-party license
notices. The core program is licensed under Apache-2.0.

A single shared service can manage multiple initialized projects and clients:

```sh
awr-mcp --registry /etc/awr/service.toml --listen 127.0.0.1:8080
```

Configure project registrations and client credentials using the
[shared MCP service guide](https://github.com/originoneai/awr/blob/main/docs/dev/reference/mcp-service.md).
