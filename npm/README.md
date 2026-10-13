# @matthew-cochran/crossmatrix

An `npx` launcher for [crossmatrix](https://github.com/praxec/crossmatrix), an
MCP server for multidimensional House of Quality (QFD) cross-reference
matrices. The package contains no server code. On first run it downloads the prebuilt `crossmatrix-mcp` release binary for
your platform from GitHub, verifies its SHA-256 checksum, caches it, and runs it
over stdio.

## Use it as an MCP server

Point your MCP client at:

```sh
npx -y @matthew-cochran/crossmatrix
```

For example, in Claude Code:

```sh
claude mcp add crossmatrix --scope user -- npx -y @matthew-cochran/crossmatrix
```

Other clients take the same command (`npx`) and arguments (`-y`,
`@matthew-cochran/crossmatrix`) in their MCP configuration. See the
[crossmatrix README install section](https://github.com/praxec/crossmatrix#install)
for every client.

All arguments are passed through to the binary, so the CLI subcommands work too:

```sh
npx -y @matthew-cochran/crossmatrix --version
npx -y @matthew-cochran/crossmatrix skills install --target claude --project .
```

The package installs one command, `crossmatrix-mcp`, which runs the launcher.

## Bundled skills

The package ships the repository's skills under `skills/`. Install them into a
project with the binary's `skills install` subcommand, which receives all
arguments unchanged:

```sh
npx -y @matthew-cochran/crossmatrix skills install --target claude --project .
```

## What it does

1. Maps `process.platform`/`process.arch` to a release target:

   | Node platform/arch | Release asset |
   | --- | --- |
   | linux/x64 | `crossmatrix-mcp-x86_64-unknown-linux-gnu.tar.gz` |
   | linux/arm64 | `crossmatrix-mcp-aarch64-unknown-linux-gnu.tar.gz` |
   | darwin/x64 | `crossmatrix-mcp-x86_64-apple-darwin.tar.gz` |
   | darwin/arm64 | `crossmatrix-mcp-aarch64-apple-darwin.tar.gz` |
   | win32/x64 | `crossmatrix-mcp-x86_64-pc-windows-msvc.zip` |
   | win32/arm64 | `crossmatrix-mcp-aarch64-pc-windows-msvc.zip` |

   An x64 Node running under Rosetta on Apple Silicon gets the native arm64
   binary, as with `install.sh`. Other platforms fail with a clear error. The
   Linux binaries need glibc, so they do not run on musl systems such as Alpine.
2. Downloads
   `https://github.com/praxec/crossmatrix/releases/download/v<version>/<asset>`
   and `checksums.sha256` from the same release, where `<version>` is this
   package's version. Only HTTPS is used. Redirects are followed only to
   `github.com`, `objects.githubusercontent.com` and
   `release-assets.githubusercontent.com` (or the mirror's own host), at most
   five times. Each download has a timeout and a 128 MiB size cap. A user and
   password in a mirror URL are never printed.
3. Verifies the archive's SHA-256 against `checksums.sha256` before extracting
   it. On any failure the partial files are deleted.
4. Extracts the binary with the system tools. A `.tar.gz` goes through `tar`,
   which ships with Linux, macOS and Windows 10 and later. A `.zip` (Windows)
   goes through PowerShell `Expand-Archive`. Archives with absolute paths, `..`
   segments or links are refused.
5. Moves the binary into a per-version cache with an atomic rename:

   | OS | Cache directory |
   | --- | --- |
   | Windows | `%LOCALAPPDATA%\crossmatrix-mcp\<version>\` |
   | Linux, macOS | `$XDG_CACHE_HOME/crossmatrix-mcp/<version>/`, else `~/.cache/crossmatrix-mcp/<version>/` |

   A lock file makes concurrent first runs (two MCP clients starting at once)
   download only once. The holder refreshes the lock every 30 seconds; a lock
   that names a dead process, or has not been refreshed for 2 minutes, is
   taken over, and the new holder removes any partial download left behind.
   An interrupted download removes its lock and partial files before exiting. Later runs start the cached
   binary with no network access.

   The binary's SHA-256 is stored in `.verified` next to it and checked on
   every start. On Linux and macOS the version directory is created private
   (mode 700), and a cached binary is refused if that directory belongs to
   another user or is writable by group or others. Delete the directory to
   download the binary again.
6. Starts the binary with stdin, stdout and stderr inherited, and forwards its
   exit code and the signals SIGINT, SIGTERM and (not on Windows) SIGHUP.

The launcher never writes to stdout, because stdout is the MCP channel.
Progress and errors go to stderr.

The package has no runtime dependencies. It uses only Node built-ins plus the
system `tar` and, on Windows, PowerShell. Node 18 or later is required.

## Install-time pre-fetch

A `postinstall` script downloads the binary during `npm install`, so the first
MCP start does not wait for it. It never fails the install: errors become a
warning, and the launcher downloads again on first run. It is skipped when
scripts are disabled (`--ignore-scripts`), when `CI` is set, when
`CROSSMATRIX_MCP_BINARY` is set, or when `CROSSMATRIX_MCP_SKIP_DOWNLOAD=1`.

## Environment variables

| Variable | Effect |
| --- | --- |
| `CROSSMATRIX_MCP_BINARY` | Path to a `crossmatrix-mcp` executable to run instead of downloading one. It must be a native executable (`crossmatrix-mcp.exe` on Windows); `.cmd` and `.bat` files are refused. |
| `CROSSMATRIX_MCP_DOWNLOAD_BASE` | Release base URL for a mirror, in place of `https://github.com/praxec/crossmatrix/releases`. Files are fetched from `<base>/download/v<version>/`. |
| `CROSSMATRIX_MCP_CACHE_DIR` | Cache root in place of the default above. |
| `CROSSMATRIX_MCP_SKIP_DOWNLOAD=1` | Skip the install-time pre-fetch. |
| `PRAXEC_ALLOW_INSECURE=1` | Allow a non-HTTPS download base and redirects. This is for tests and local mirrors only, and has the same meaning as in `install.sh` and `install.ps1`. |

## When the download fails

The error on stderr names the cause and the remedy. If you cannot reach GitHub,
install the binary yourself with `install.sh`/`install.ps1` or from the
[releases page](https://github.com/praxec/crossmatrix/releases), then set
`CROSSMATRIX_MCP_BINARY` to its path, or put the binary in your MCP client
configuration directly.

## Development

```sh
node --test "test/*.test.mjs"   # from npm/; offline, uses a local fixture server
npm pack --dry-run              # check the published file list
```

The tests serve fixture releases over plain HTTP on `127.0.0.1` and set
`PRAXEC_ALLOW_INSECURE=1`, the same override the installer tests use. The
launcher has no test-only switches.

## License

Apache-2.0
