# crossmatrix

Multidimensional **House of Quality (QFD)** — cross-dimensional relationship matrices for structured trade-off and requirement analysis. A Rust workspace: `crossmatrix` (engine) + `crossmatrix-mcp` (MCP server). Part of [Praxec](https://github.com/praxec/praxec).

## Install

### npm / npx (recommended for MCP clients)

```sh
npx -y @matthew-cochran/crossmatrix --version
claude mcp add crossmatrix --scope user -- npx -y @matthew-cochran/crossmatrix
```

The `@matthew-cochran/crossmatrix` package is a small launcher: on first run it
downloads the release binary for your platform, verifies it against the
release's `checksums.sha256`, caches it, and runs it over stdio (signals are
forwarded; stdout belongs to MCP). `CROSSMATRIX_MCP_BINARY` points it at a binary
you installed yourself. The package also bundles the agent skills.

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

## Repo-committed models: `.crossmatrix/`

crossmatrix keeps a model as three documents linked by stable `id@version`
(ADR-0004), plus an append-only audit log, in a directory you commit:

```sh
crossmatrix-mcp init            # creates .crossmatrix/ in the current directory
crossmatrix-mcp check           # validates every document and cross-reference
```

```
.crossmatrix/
  config/<configId>@<version>.json            # methodology: scales, relations, contractions
  dimensions/<dimensionSetId>@<version>.json  # dimensions and their members
  state/<stateId>.json                        # cells/observations + revision
  audit/<stateId>.jsonl                       # one line per accepted mutation (merge=union)
```

Writes are canonical JSON (sorted keys, 2-space indent, trailing newline) and
atomic, so diffs stay small. Findings are computed on every read and never
stored. `check` exits non-zero with path-specific diagnostics (for example
`state/demo.json: cells[3].to: unknown member 'x' in dimension 'dim_part'`);
run it in CI.

The server finds its store from, in order: `CROSSMATRIX_DIR`, the nearest
ancestor directory containing `.crossmatrix/`, `$XDG_DATA_HOME/crossmatrix`
(or `~/.local/share/crossmatrix`), then `./crossmatrix-state`. `model.status`
reports the resolved `root`.

## CLI

| Command | Effect |
|---|---|
| `crossmatrix-mcp` | serve MCP on stdio |
| `crossmatrix-mcp init [--dir D]` | create `D/.crossmatrix/` (idempotent) |
| `crossmatrix-mcp check [--dir D]` | validate the store; exit 1 on any problem |
| `crossmatrix-mcp skills install --target <t> (--project D \| --user) [--dry-run] [--force]` | install the bundled agent skills |
| `crossmatrix-mcp skills uninstall --target <t> (--project D \| --user)` / `skills list` | remove / list installs |
| `crossmatrix-mcp --version` / `--help` | version / usage |

## Tools

Two MCP tools, `crossmatrix.command` (writes) and `crossmatrix.query` (reads).
Every response, including a rejection, carries `links` to the legal next ops;
a rejection's `structuredContent` is `{ok: false, error: {code, message}, links}`.

### Writes (`crossmatrix.command`)

```json
{"request": {"schemaVersion": "0.2.0", "requestId": "r-17", "modelId": "checkout",
  "expectedVersion": 16, "actor": {"actorType": "llm", "persona": "observer"},
  "op": {"kind": "observe", "relationId": "rel_need_char", "from": "need_fast",
         "to": "char_latency", "observation": "strong", "provenanceClass": "normative"}}}
```

| Op | Layer | Purpose |
|---|---|---|
| `model.open` | — | open (or create) `state/<stateId>.json`; survives restarts |
| `model.status` | — | version, store root, counts |
| `scale.declare` | Configuration | observation token → value table |
| `relation.declare` | Configuration | `from`/`to` dimensions, `relationType`, scale, `coveragePolicy` |
| `contraction.declare` | Configuration | a chain of relations for derived views |
| `dimension.register` | Dimensions | an axis (`dimensionKind` is any snake_case word) |
| `members.sync` | Dimensions | upsert a roster; missing members are deprecated, not deleted; an unchanged roster is a no-op |
| `member.propose` | Dimensions | add one member |
| `observe` | State | a token + evidence on a cell (never a number) |
| `evidence.attach` | State | add evidence to an observation |
| `deprecate` | any | retire a member or relation (`supersededBy`) |

Observation-only: a numeric member `weight` is accepted only with
`sourceRef.system` (a deterministic import), and a `measuredValue` only with
`provenanceClass: observed` plus a `sourceRef`. `expectedVersion` is an
optimistic lock; `requestId` makes a command idempotent, also across restarts.
Each accepted mutation appends one audit line `{ts, requestId, actor, op,
beforeVersion, afterVersion, summary}`.

The legacy one-call flow still works: a command carrying a full `model`
imports it into the session (in memory) for the queries that follow.

### Reads (`crossmatrix.query`)

`model.status`, `describe`, `slice`, `trace`, `explain`, `coverage`,
`gaps.orphans`, `gaps.next`, `stale`, `conflicts`, `analyze.marginalize`,
`analyze.contract`, `analyze.findings`, `validate`, `report`, `export`.

- `report {relationId?}` returns a Markdown House of Quality: weights, the
  relationship matrix, the correlation roof (a self-relation), technical
  importance, targets (`attributes.target`), coverage and findings with
  contributors. Without `relationId` it covers every relation.
- `export {format: model|report|findings}` returns the merged model JSON.
- `conflicts {relationId?}` lists valence-by-separation tensions and conflicts.
- `trace` and `slice` accept a member id or a `sourceRef.uri`, page with
  `limit`/`cursor`, and with `rollup: true` roll leaf observations up a
  `contains` hierarchy (a directory implements a feature if any file under it
  does).
- `stale {hashes?, readFiles?}` compares stored source hashes with current ones
  (`readFiles` hashes `file://` sources inside the project) and returns the
  changed members plus every cell that references them, paged.

## Agent skills

crossmatrix ships [Agent Skills](skills/) that teach an agent the method and
common uses. Install them into a project (or `--user`) for one client or all:

```sh
npx -y @matthew-cochran/crossmatrix skills install --target claude --project .
crossmatrix-mcp skills install --target all --project .      # .claude/skills + .agents/skills
```

Targets: `claude`, `codex`, `cursor`, `copilot`, `gemini`, `agents-md`, `all`. A
`.crossmatrix-skills.json` manifest records file hashes, so re-installs update
only files you have not edited (`--force` overwrites them) and `uninstall`
removes only unmodified files. The installer never edits MCP configuration; it
prints the registration command instead
(`claude mcp add crossmatrix --scope user -- npx -y @matthew-cochran/crossmatrix`).

| Skill | Use it for |
|---|---|
| `crossmatrix` | the method: Configuration / Dimensions / State, observation-only writes, the driver loop, scoped reads, `.crossmatrix/` |
| `crossmatrix-hoq` | a classic House of Quality: weighted needs × characteristics, roof, technical importance, targets |
| `crossmatrix-jtbd` | jobs to be done × user segments × journeys, joined by a contraction |
| `crossmatrix-competitive` | needs × competitors benchmarking and gaps worth winning |
| `crossmatrix-test-coverage` | requirements/journeys × tests; orphans are untested requirements |
| `crossmatrix-risk` | failure modes × components/requirements (pairs with fmeca) |
| `crossmatrix-prioritize` | deliverables × value drivers → a ranked list for a planner (pairs with cpm) |
| `crossmatrix-review` | check, conflicts, stale, the report and a sign-off checklist |

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
