# Agent Work Runtime

Persistent work state and minimal context for long-running AI agents.

AWR 0.6.0 installs the native Rust `awr` CLI and `awr-mcp` server for stdio and shared Streamable HTTP.
Context compilation runs locally without an AI model call.

```sh
npm install -g @originoneai/agent-work-runtime@0.6.0
awr --version
awr --help
awr-mcp --help
```

Without a global installation, select the command explicitly:

```sh
npx --package=@originoneai/agent-work-runtime@0.6.0 awr --help
npx --package=@originoneai/agent-work-runtime@0.6.0 awr-mcp --project /absolute/project
```

The project must be initialized before starting its MCP server. Follow the
[project documentation](https://github.com/originoneai/agent-work-runtime).

The distribution targets macOS 15+ arm64 and Intel x64, Linux x64 with glibc 2.39+ (Ubuntu 24.04
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
