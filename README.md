# crossmatrix

Multidimensional **House of Quality (QFD)** — cross-dimensional relationship matrices for structured trade-off and requirement analysis. A Rust workspace: `crossmatrix` (engine) + `crossmatrix-mcp` (MCP server). Part of [Praxec](https://github.com/praxec/praxec).

## Install

### Prebuilt binary (no Rust, Cargo, or Git required)

```sh
# Linux / macOS
curl -fsSL https://github.com/praxec/crossmatrix/releases/latest/download/install.sh | sh

# Windows (PowerShell)
irm https://github.com/praxec/crossmatrix/releases/latest/download/install.ps1 | iex
```

The installer resolves your OS and CPU architecture, downloads the matching
release asset, verifies its SHA-256 against the release's `checksums.sha256`,
extracts it safely, and atomically installs the binary into a user-local
managed directory (`$HOME/.local/bin` on Linux/macOS,
`%LOCALAPPDATA%\Programs\crossmatrix-mcp` on Windows). It never compiles from
source and fails loudly on an unsupported OS/architecture.

Pin a specific release instead of the current latest stable:

```sh
curl -fsSL https://github.com/praxec/crossmatrix/releases/latest/download/install.sh \
  | sh -s -- --version v0.2.1
```

The installer scripts are published as release assets alongside the binaries.

Or download the archive directly. Every release publishes a `checksums.sha256`
and a machine-readable `release-manifest.json` listing each target, asset,
digest, version, and source SHA:

| OS | Arch | Target triple | Asset | Build support | Runtime smoke tested |
|----|------|---------------|-------|---------------|----------------------|
| Linux | x86_64 | `x86_64-unknown-linux-gnu` | `crossmatrix-mcp-x86_64-unknown-linux-gnu.tar.gz` | native CI | native CI |
| Linux | arm64 | `aarch64-unknown-linux-gnu` | `crossmatrix-mcp-aarch64-unknown-linux-gnu.tar.gz` | native CI | native CI |
| macOS | x86_64 | `x86_64-apple-darwin` | `crossmatrix-mcp-x86_64-apple-darwin.tar.gz` | native CI | native CI |
| macOS | Apple Silicon | `aarch64-apple-darwin` | `crossmatrix-mcp-aarch64-apple-darwin.tar.gz` | native CI | native CI |
| Windows | x86_64 | `x86_64-pc-windows-msvc` | `crossmatrix-mcp-x86_64-pc-windows-msvc.zip` | native CI | native CI |
| Windows | arm64 | `aarch64-pc-windows-msvc` | `crossmatrix-mcp-aarch64-pc-windows-msvc.zip` | native CI | native CI |

"Native CI" means each asset is built and its MCP `initialize`/`tools/list`
handshake smoke-tested on that platform's own runner. No other CPU/OS
combination is claimed; unsupported platforms are rejected rather than
silently cross-compiled.

### Updates

Re-run the installer to update. Only the binary in the managed install
directory is replaced (atomically); application state and configuration live
outside that directory and are never overwritten.

### From source

```sh
cargo build --release
```

produces the `crossmatrix-mcp` MCP stdio server.

## Using it with Praxec

This is an MCP tool used by [Praxec](https://github.com/praxec/praxec) packs. The easiest way to
get it — and a workflow pack that uses it — up and running is the one-command setup:

```bash
curl -fsSL https://raw.githubusercontent.com/praxec/packs/main/setup.sh | bash
```

See the [pack registry](https://github.com/praxec/packs) for this tool's provider coordinates
(container image / release binary) and which packs depend on it.

## License
[Apache-2.0](LICENSE).
