# AWR

The open-source project delivery platform for people and AI.

AWR 0.7.0 installs the native Rust `awr` CLI and `awr-mcp` server for stdio and shared Streamable HTTP.
Context compilation runs locally without an AI model call.

Project Hub and Project Progress Overview are bundled with the native payload.
Run `awr overview` to search local AWR-managed projects and inspect their progress.
The first ordinary AWR command or local Agent MCP connection starts one background
service automatically. After that, open **http://127.0.0.1:7381** directly; running
`awr overview` first is optional. Installation alone does not start a process.
If the port is occupied, `awr overview status --json` reports the actual URL.
The next CLI/MCP use recovers a terminated service without duplicate instances.
See the [local hub guide](https://github.com/originoneai/awr/blob/v0.7.0/docs/dev/reference/local-project-hub.md).

```sh
npm install -g @originoneai/agent-work-runtime@0.7.0
awr --version
awr --help
awr-mcp --help
```

Without a global installation, select the command explicitly:

```sh
npx --package=@originoneai/agent-work-runtime@0.7.0 awr --help
npx --package=@originoneai/agent-work-runtime@0.7.0 awr-mcp --project /absolute/project
```

The project must be initialized before starting its MCP server. Follow the
[project documentation](https://github.com/originoneai/agent-work-runtime).

The distribution targets macOS 15+ arm64 and Intel x64, Linux x64/arm64 with glibc 2.39+ (Ubuntu 24.04
baseline), and Windows x64. Node.js 22.14+ is required for the npm launcher. Git is
required for Git-bound work branch operations. SQLite is included in the binaries.
Installation requires access to the npm registry; no Rust compiler or install-time
build is needed on a supported platform. Keep npm optional dependencies enabled.

The platform package contains the binaries, source identity, checksums and
third-party license notices. The core program is licensed under Apache-2.0.

A single shared service can manage multiple initialized projects and clients:

```sh
awr-mcp --registry /etc/awr/service.toml --listen 127.0.0.1:8080
```

Configure project registrations and client credentials using the
[shared MCP service guide](https://github.com/originoneai/agent-work-runtime/blob/main/docs/dev/reference/mcp-service.md).
