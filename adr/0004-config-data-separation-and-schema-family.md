# ADR 0004 — Configuration / Dimensions / State separation, linked by stable identifiers

- Status: Accepted (vetted 2026-06-29 — an architecture review = APPROVED, all 3 schemas Essential; a use-case-falsification review = adequate, 12/12 contradictions SURVIVE incl. the `$ref` counterexample and computed-only findings)
- Date: 2026-06-29
- Builds on: ADR 0002 (`crossmatrix` engine — "fix the operations, configure the axes"), ADR 0003 (the `crossmatrix-mcp` 2-tool contract). Refines what the mcp persists and how documents reference each other.

## Context

A crossmatrix model is **durable repository state that glues systems together** — it encodes *how* multiple weighted domains (e.g. UX, JTBD, FMECA, and future domains) cross-reference, and it travels with the repo as a long-lived analysis artifact. The authored model decomposes along **three independent change-drivers**, and we align the persisted schemas to those axes so each can change without churning the others:

| Layer (schema) | Holds | Changes when… | Owner / lifecycle |
|---|---|---|---|
| **Configuration** | `scales`, **relation types**, relation declarations (`from`/`to`/`relationType`/`scale`/`coveragePolicy`), `contractions`, + `configId`/`version`/`aspect` | the analysis *methodology* changes (rare) | reusable template, **synced** across projects/aspects |
| **Dimensions** | dimension declarations (`id`/`kind`) + their `members` (rosters, incl. `weight`/`weightObservation`+`sourceRef`), + `dimensionSetId`/`version` | the domain's *entities* change | **federated** — each domain system (e.g. a UX tool, a JTBD tool, an FMECA tool) owns its dimension's members |
| **State** | the `cells`/observations — the cross-reference *values* — + `configRef` (and dimension/member refs) | someone *observes/updates* the analysis (frequent) | the analysis activity (LLM/human) |

Relation **types** dictate *what relationships are possible*, so they live in Configuration; the **entities** are federated and change on their own clock, so they are their own layer (Dimensions); the **observations** are the activity (State). Aspects (HoQ / JTBD / UX-views-of-quality) are **Configuration instances** (per ADR-0002 — configure the axes, don't fork the engine), distinguished by the `aspect` discriminator.

## Decision

### 1. Three authored schema types: **Configuration**, **Dimensions**, **State**

A loadable engine model = **Configuration ⊕ Dimensions ⊕ State**, merged → `crossmatrix::Model::load` → query. Two existing schemas remain orthogonal: the **engine** schema (`crossmatrix.schema.json` — the *merged* shape the pure core loads) and the **mcp contract** (`crossmatrix-mcp.schema.json`). We do **not** create per-aspect schemas — aspects are Configuration instances.

The boundary (vs today's single combined model):
- **Configuration** ⟵ pulls out `scales` + `relations`(declarations only) + `contractions` + relation-type vocabulary.
- **Dimensions** ⟵ pulls out `dimensions` (declarations + `members`).
- **State** ⟵ keeps `cells`/observations only.
- `findings` is **computed**, never authored.

### 2. Documents reference each other by STABLE IDENTIFIER (id + version), NOT `$ref`

- a Configuration carries `configId` + `version` + `aspect`, and references the entity set by `dimensionSetRef: { dimensionSetId, version }`; its relation `from`/`to` are **dimension ids**;
- a Dimensions doc carries `dimensionSetId` + `version`;
- a State doc binds by `configRef: { configId, version }`; its cells reference relation ids + member ids;
- intra-doc references stay id-based (cell→member, relation→dimension).

**Rationale:** a `$ref` couples to a *location* (breaks when a Configuration or Dimensions doc is synced into another repo or relocated); an **identifier couples to a logical artifact** the mcp resolves from the repo's library wherever it physically lives. That is what makes these durable, relocatable, syncable. *(Within a single schema, `$defs` for type reuse is fine — the constraint is on cross-**document** linkage.)*

### 3. The merge/compatibility contract (fail-fast, recoverable)

`model.open` resolves the three by id+version and merges; it **rejects with a rich diagnostic** when:
- a `configRef` / `dimensionSetRef` resolves to nothing, or to an incompatible `version`;
- a Configuration relation's `from`/`to` names a dimension absent from the resolved Dimensions doc;
- a State cell references a relation id absent from the Configuration, or a member id absent from the Dimensions doc.

(This is the existing dangling-reference integrity check, generalized across the three documents.)

**Version-compatibility policy (v1):** an exact `version` match — a State pins an exact `configRef` version and a Configuration pins an exact `dimensionSetRef` version (checked bidirectionally). This is the strict, unambiguous default; a future semver policy (e.g. major-must-match, minor/patch may diverge) can relax it without changing the schemas. The id+version **resolution** itself (how the mcp finds a doc by `id@version` in the repo library) is an mcp-layer index — an implementation concern, not a schema concern. *(Both per the architecture-vet findings.)*

### 4. The mcp crate is a durable Configuration+Dimensions+State manager (+ query surface)

`crossmatrix-mcp` (core stays pure — no `rmcp`, no fs in core):
- **Configuration library**: `config.put`/`get`/`list`/`sync` (syncable, keyed by `configId@version`);
- **Dimensions store**: `dimensions.put`/`get` (often *imported/federated* from the domain systems);
- **State store**: `state.put`/`get` per project;
- **`model.open`**: resolve `configRef` + `dimensionSetRef` + State → merge (§3) → `Model::load` → handle;
- the **query/analyze surface** over the opened model (`slice`/`trace`/`gaps.*`/`coverage`/`stale`/`conflicts`/`analyze.{marginalize,contract,findings}`/`validate`/`export`);
- the ADR-0003 envelope: `requestId` idempotency, `expectedVersion` CAS, HATEOAS links, observation-only rejection.

Persistence + id-resolution are **MCP-layer I/O** (mcp reads/writes JSON, hands merged JSON to the pure core).

## Consequences

- (+) Each layer changes independently: methodology (synced), entities (federated), observations (activity) — no cross-churn.
- (+) **Dimensions are shared across Configurations/aspects** — one roster, many analyses.
- (+) Identifier linkage → a synced Configuration "just resolves" in a new repo; no path rewrites.
- (+) Federation is structural: domain systems own/emit their Dimensions doc; crossmatrix never becomes the source of truth for entities.
- (−) Three schemas + a 3-way merge/compat step (the §3 contract) to maintain.
- (−) The mcp owns repo file I/O + an id-resolution index (that *is* the durable-state requirement).

## Follow-ups / build order

- **Phase A:** the three schemas (Configuration, Dimensions, State) + id-refs + `aspect` discriminator + typify type-gen; the merge/compat rules (§3).
- **Phase B:** mcp `config.*` / `dimensions.*` / `state.*` stores + `model.open` (resolve+merge → `Model::load`).
- **Phase C:** mcp query/analyze surface + envelope/HATEOAS/observation-only.
- **Phase D:** `config.sync` across projects.
- Build **example Configurations** (HoQ, JTBD, UX-views-of-quality) + Dimensions + State to vet the separation end-to-end.
- (Later, separate mission) incremental mutation (`observe`/`member.propose`) — needs a core write-API; deferred per the immutable-core finding.

## Alternatives considered

- **Two schemas (Configuration + State only)** — rejected: the *entities* (Dimensions) change independently of the methodology and are **federated** (owned by the domain systems), so folding them into either Configuration or State forces cross-churn and muddies ownership.
- **Single combined model doc + `$ref` between files** — rejected: `$ref` couples to location (breaks sync/relocation) and conflates template, entities, and observations.
- **A per-aspect schema family** (`hoq`/`jtbd`/`ux-views` each its own schema) — rejected: re-forks what ADR-0002 unified; aspects are Configuration instances.
