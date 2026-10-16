# Changelog

All notable changes to this project are documented here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/) and the project uses
[Semantic Versioning](https://semver.org/).

## [Unreleased]

## [0.3.2] - 2026-10-16

### Security
- `cargo update` of 73 crates; `cargo deny` advisories and `cargo audit` are clean, and clippy (`-D warnings`), fmt and the workspace tests pass (#28).

## [0.3.1] - 2026-10-13

### Fixed
- The container image carries `LABEL io.modelcontextprotocol.server.name="io.github.praxec/crossmatrix"`, which the MCP registry requires to accept the OCI package (the 0.3.0 image lacked it, so 0.3.0 could not be registered).
- `server.json` now carries the version in the OCI identifier tag
  (`ghcr.io/praxec/crossmatrix:X.Y.Z`) instead of a package `version` field,
  which the MCP registry rejects (the v0.3.0 registry publish failed with HTTP
  400). `check-version-sync.sh` checks the tag and refuses a package `version`.

### Added
- `scripts/npm-release.sh`: the maintainer's manual npm release of the launcher tarball attached to a GitHub release (digest, contents and bundled-skills checks; 2FA prompt; post-publish `npx --version` check), and `docs/releasing.md`, the release runbook.

## [0.3.0] - 2026-10-12

### Added
- **AI assist (optional cargo features, off by default; explicit opt-in
  `CROSSMATRIX_ASSIST` plus a key).** `jev`: `assist.suggest` asks Jev (TypeSafe,
  or OpenRouter `~typesafe/jev-latest`) a choice question over a relation's
  scale tokens for each candidate cell and returns `{cell, token, probabilities,
  confidence}`; `record: true` stores them as **proposed** observations by
  actor `{classifier, jev}`; `gaps.next {rankBy: "uncertainty"}`. `rig`:
  `assist.members` proposes members from supplied text with verbatim spans (never
  auto-added) and `assist.narrative` / `report {narrative: true}` narrate the
  findings. Calls are bounded and report usage; without the feature, opt-in or
  key the result is a typed `assist_unavailable` with links.
- **Proposed observations** (`Observation.status: proposed`) and the
  `proposals` query: stored and exported, excluded from every analysis until a
  cell is confirmed with an ordinary `observe`.
- **Repo-committed store** (`.crossmatrix/`): Configuration, Dimensions and
  State documents stored by stable `id@version` (ADR-0004), canonical JSON
  (sorted keys, 2-space indent, trailing newline) written atomically, plus an
  append-only `audit/<stateId>.jsonl` (union-merged). Findings are never stored.
- **CLI**: `crossmatrix-mcp init`, `check` (path-specific diagnostics, non-zero
  exit), `--version`/`-V`, `--help`. No arguments still serves MCP on stdio.
- **State directory resolution**: `CROSSMATRIX_DIR`, nearest ancestor
  `.crossmatrix/`, `$XDG_DATA_HOME/crossmatrix` (or `~/.local/share/crossmatrix`),
  then `./crossmatrix-state`; reported by `model.status`.
- **Core write API** (`crossmatrix::write`): pure functions returning a new
  validated `Model` or a typed `WriteError` with a stable code.
- **Write ops** on `crossmatrix.command`: `model.open`, `model.status`,
  `dimension.register`, `members.sync`, `scale.declare`, `relation.declare`
  (with `coveragePolicy`), `contraction.declare`, `observe`, `member.propose`,
  `evidence.attach`, `deprecate` (`status` + `supersededBy`). Each op is
  validated against `schemas/crossmatrix-mcp.schema.json`, enforces
  `expectedVersion` and `requestId` idempotency (also across restarts), persists
  through the store, writes one audit line and returns `links`. A command that
  changes nothing is a no-op (no version bump, no audit line).
- **Queries**: `conflicts`, `report` (Markdown House of Quality: weights, matrix,
  correlation roof, technical importance, targets, coverage, findings with
  contributors), `export` (merged model JSON), `model.status`; `stale` against
  supplied hashes or (opt-in) hashed `file://` sources, returning changed
  members and their cells, paged.
- **Hierarchies**: a `contains` relation type (structural self-relation, cycle
  detection) and `rollup: true` on `trace`, `slice`, `coverage`, `gaps.next`
  and the analyses, rolling leaf observations up to ancestors.
- **Lookups by source**: `SourceRef.uri`/`hash`; `trace` and `slice` accept a
  member id or a source uri and page with `limit`/`cursor`.
- **Performance budget**: on 10k members × 200 features, `members.sync`,
  `stale` and `trace` each finish under 1 s (enforced by tests).

- **npm launcher** `@matthew-cochran/crossmatrix` (`npx -y @matthew-cochran/crossmatrix`):
  downloads the platform release asset, verifies it against `checksums.sha256`,
  caches it and runs it with stdio inherited and signals forwarded. It bundles
  the agent skills. The release workflow publishes it via npm trusted publishing,
  gated on the tag matching every manifest version (`scripts/check-version-sync.sh`).
- **Agent skills** (`skills/`): `crossmatrix`, `crossmatrix-hoq`, `crossmatrix-jtbd`,
  `crossmatrix-competitive`, `crossmatrix-test-coverage`, `crossmatrix-risk`,
  `crossmatrix-prioritize`, `crossmatrix-review`.
- **`crossmatrix-mcp skills install|uninstall|list`** for claude, codex, cursor,
  copilot, gemini, agents-md or all, with a hash manifest, user-edit protection,
  dry runs and atomic writes; it prints (never writes) MCP registration.

### Changed
- `DimensionKind` is an open snake_case vocabulary; domain meaning lives in
  configurations and skills, not engine code.
- Rejections now carry `structuredContent` `{ok: false, error: {code, message},
  links}`; the text content is still the message.
- ADR-0002 and ADR-0003 are Accepted; their follow-ups (`status`/`supersededBy`,
  `coveragePolicy`, the core write API) are closed.

### Compatibility
- Every existing tool, op and query name and every success response shape is
  unchanged; importing a full model with `crossmatrix.command` and querying it
  in the same session works as before.
