# Agent Work Runtime

Persistent work state and minimal context for long-running AI agents.

AWR 0.6.0 includes the native Rust `awr` CLI and `awr-mcp` server for stdio and shared Streamable HTTP.
It provides command-line tools; context compilation runs locally.

```sh
python -m pip install agent-work-runtime==0.6.0
awr --version
awr --help
awr-mcp --help
```

For an isolated application installation, use `pipx install agent-work-runtime==0.6.0` or `uv tool install agent-work-runtime==0.6.0`.

The project must be initialized before running
`awr-mcp --project /absolute/project`. Follow the
[project documentation](https://github.com/originoneai/agent-work-runtime).

The distribution targets macOS 15+ arm64 and Intel x64, Linux x64 with glibc 2.39+ (Ubuntu 24.04
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
[shared MCP service guide](https://github.com/originoneai/agent-work-runtime/blob/main/docs/dev/reference/mcp-service.md).
