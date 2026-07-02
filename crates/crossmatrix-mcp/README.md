# crossmatrix-mcp

An LLM-facing **Model Context Protocol (MCP)** server for the
[`crossmatrix`](../crossmatrix) weighted cross-reference engine. The core crate
stays pure (no `rmcp`, no filesystem); this crate projects it as an MCP and owns
the LLM ergonomics, persistence, and governance.

## Two tools

- **`crossmatrix.command`** — mutation-first writes. Observation-only: write ops
  carry categorical observation *tokens* (+ evidence), never numbers — the engine
  rejects numeric weights and unknown tokens (fail-fast, recoverable). Optimistic
  CAS via `expectedVersion`; `requestId` idempotency.
- **`crossmatrix.query`** — reads and engine-computed analyses
  (`analyze.contract`, `analyze.findings`, `validate`, ...).

Every response carries HATEOAS `links` (the legal next ops) so the LLM can
navigate and recover without guessing.

## Durable Configuration / Dimensions / State

Per ADR-0004, a model is persisted as three independently-versioned documents
linked by stable id+version (never `$ref`): **Configuration** (methodology),
**Dimensions** (federated entity rosters), and **State** (per-project
observations). `model.open` resolves and merges the three (fail-fast on
dangling/incompatible references) into a loadable `crossmatrix` engine model.

## Status

Experimental. Mutation ops (`observe` / `member.propose`) are deferred pending a
core write API; the current build covers import/validate + the query/analyze
surface. See `../../adr/` (0003 MCP contract, 0004 schema separation). This crate
depends on `crossmatrix` by path+version — publish `crossmatrix` first, then this.
Set `repository`/`homepage` in `Cargo.toml` before publishing.

## License

MIT
