# crossmatrix schemas

JSON Schemas (draft 2020-12) for the **crossmatrix** engine and its MCP contract.
All are `typify`-clean (they generate Rust types via `import_types!`). Authoring
rules common to every schema: **observation-only** (the LLM emits categorical
tokens + evidence, never numbers — numbers enter only via a declared scale table,
a measured source with `sourceRef`, or engine computation) and **valence by
separation** (support and opposition are separate unipolar relations, never a
signed/bipolar scale, so a strong-for and a strong-against never cancel).

## The schema family

| Schema | Layer | What it is |
|---|---|---|
| `crossmatrix.schema.json` | **engine (merged)** | The shape the **pure core** (`crates/crossmatrix`) loads via `Model::load`. The *merged* result of Configuration ⊕ Dimensions ⊕ State. A sparse N-dimensional weighted cross-reference model (generalized House of Quality): `dimensions` + pairwise `relations` + `contractions`, multi-observation cells, computed `findings`. This is the canonical engine model. |
| `crossmatrix-config.schema.json` | **Configuration** (ADR-0004) | The reusable, **syncable** *methodology* — `scales`, relation types + relation declarations, contraction chains. No members, no cells. Aspects (HoQ / JTBD / UX-views-of-quality) are **instances** of this one schema, tagged by `aspect`. References the entity set by `dimensionSetRef` (stable id+version, never `$ref`). |
| `crossmatrix-dimensions.schema.json` | **Dimensions** (ADR-0004) | The axes + their members (rosters) — the **entities** being cross-referenced. **Federated**: each domain system (e.g. a UX tool, a JTBD tool, an FMECA tool) owns its dimension's members. Identified by `dimensionSetId` (id+version); one Dimensions doc can be shared across many Configurations. |
| `crossmatrix-state.schema.json` | **State** (ADR-0004) | The cross-reference **values** — the cells/observations for **one project**; the frequently-changing analysis activity. Binds to its methodology by `configRef` (id+version, never `$ref`); each cell names a relation id + member ids. |
| `crossmatrix-mcp.schema.json` | **MCP contract** | The LLM-facing request/response envelope for the `crossmatrix-mcp` crate. Two tools: `crossmatrix.command` (mutation-first writes, observation tokens only) and `crossmatrix.query` (reads/analyses). HATEOAS `links`, `requestId` idempotency, `expectedVersion` CAS. Governance (status/audit_log/actor) is MCP-layer state, not core data. |

## How they fit together

```
Configuration  ─┐
                ├─ model.open (resolve id+version, merge, fail-fast) ──▶ crossmatrix.schema.json ──▶ Model::load
Dimensions     ─┤        (this is the mcp's job; the core stays pure)        (engine model)
State          ─┘
```

- The three authored layers (Configuration / Dimensions / State) are linked by
  **stable identifier + version**, never `$ref` — so a synced Configuration "just
  resolves" in a new repo and documents stay relocatable (ADR-0004 §2).
- `crossmatrix-mcp.schema.json` describes the *wire protocol*; the four above
  describe *data at rest* and the *merged engine model*.

See `adr/0002` (standalone engine — "fix the operations, configure the axes"),
`adr/0003` (the 2-tool MCP contract), and `adr/0004` (the three-layer separation).
