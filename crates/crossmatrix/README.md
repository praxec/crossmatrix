# crossmatrix

A sparse, configurable, **N-dimensional weighted cross-reference engine** — a
generalized House of Quality. Fix the *operations*, configure the *axes*: carry
QFD, FMECA, JTBD, and UX "views of quality" in one structure, and ask
cross-cutting questions (value × characteristic × failure-risk × coverage) as
native operations instead of bespoke joins.

Pure Rust, no system BLAS (`sprs` for sparse storage + contraction, `nalgebra`
for dense ops). Types are generated from a canonical JSON Schema via `typify`.

## Principles

- **Observation-only** — the model carries *categorical observation tokens* from
  a declared vocabulary; numbers enter only via a declared `scales` table, a
  measured source (`provenanceClass: observed` + `sourceRef`), or engine
  computation. The LLM never authors a number.
- **Valence by separation** — support and opposition are *separate* unipolar
  relations; the engine reports **net AND tension**, so strong-for and
  strong-against never silently cancel.
- **Federation** — each domain system stays authoritative for its own entities;
  the matrix holds rosters (projections) + the mappings between them.
- **Fixed operations** — `marginalize` (weighted rollup), `contract`
  (multilinear cascade over a contraction chain), `findings` (deterministic
  orphan / uncovered / tension / critical-exposure / stale-reference detection).

## Quick start

```rust
use crossmatrix::{Model, Axis};

let model = Model::load(include_str!("../examples/qfd-fmeca-3axis.json"))?;

// Deterministic issue detection (orphans, tension, stale references, ...).
for finding in model.findings() {
    println!("{finding:?}");
}

// Multilinear cascade over a declared contraction chain (relative rank).
let exposure = model.contract("ctr_req_failure_exposure");

// Weighted rollup over one axis of a relation.
let rollup = model.marginalize("rel_req_char", Axis::From)?;
# Ok::<(), crossmatrix::ValidationError>(())
```

`Model::load` validates on ingest (fail-fast, recoverable): unknown observation
tokens, inferred observations lacking evidence, cyclic/scale-incompatible
contraction chains, dangling cells, and the precondition that weighted analyses
have resolvable member weights.

## Configuration / Dimensions / State

A model decomposes into three durable, independently-versioned layers (ADR-0004),
linked by stable id+version (never `$ref`):

- **Configuration** — the methodology (scales, relation types/declarations,
  contractions); reusable + synced across projects.
- **Dimensions** — the axes + members (rosters); federated from the owning systems.
- **State** — the cells/observations (the per-project values).

The LLM-facing `crossmatrix-mcp` crate (separate, depends on this pure core)
persists and merges these and projects the engine as an MCP.

## Status

Experimental. See `adr/` for the design records (0002 engine, 0003 mcp contract,
0004 schema separation). Set `repository`/`homepage` in `Cargo.toml` before
publishing.

## License

MIT
