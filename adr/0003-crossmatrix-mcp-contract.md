# ADR 0003 — `crossmatrix-mcp`: the LLM-facing MCP contract

- Status: Proposed
- Date: 2026-06-29
- Builds on: ADR 0002 (`crossmatrix` core); prior internal design notes (the mutation-first envelope + canonical-state/audit split this extends).

## Decision
A **separate `crossmatrix-mcp` crate** (depends on `crossmatrix` core; core stays pure — no `rmcp` dep) projects the core as an LLM-optimized MCP. Two stable entry tools + HATEOAS links, mirroring a standard MCP gateway pattern:

- **`crossmatrix.command`** (writes, mutation-first): envelope `{ schemaVersion, requestId, modelId, expectedVersion, actor{actorType, persona}, op, payload }` — optimistic-CAS (`expectedVersion`) + `requestId` idempotency (from the foundation envelope).
- **`crossmatrix.query`** (reads/analyses; engine-computed, never mutating).
- Every response returns `links` (legal next ops) + diagnostics → the LLM navigates + recovers without guessing.

### Feature set (families)
- **Admin/lifecycle:** `model.open`/`model.status`, `dimension.register`, `scale.declare`, `relation.declare`, `contraction.declare`, `kindProfile.register` *(deferred)*, `members.sync`.
- **Mutation (observation-only):** `observe` (token+evidence, never numbers), `member.propose` (weightObservation), `evidence.attach`, `deprecate`.
- **Driver queries (anti-overreach):** `gaps.next`, `gaps.orphans`, `coverage`, `stale`, `conflicts`.
- **Read/slice:** `slice` (scoped, token-bounded — never the whole model), `describe`, `trace` (lineage / "what it ties to"), `explain` (derivation).
- **Analyze (engine-computed):** `analyze.marginalize`, `analyze.contract`, `analyze.findings` (net+tension+contributors+path), `analyze.priority` *(deferred)*, `analyze.compare` *(deferred)*.
- **Validate/export:** `validate`(+`.profile`), `export`/`report`.

### LLM-optimization invariants (poka-yoke, per consumption failure mode)
1. No whole-model reads — scoped slices, token-bounded.
2. Observation-only at the boundary — `command` rejects any LLM number; only tokens + `measuredValue`(observed+sourceRef).
3. Driver-led — `gaps.next` picks what to observe.
4. Rich diagnostics + legal-next-ops links — every rejection recoverable.
5. Optimistic CAS + `requestId` idempotency.
6. Staleness surfaced in reads + `stale` query.
7. Cross-core routing, not faking — cycles/temporal queries return a typed "route to graph-core/event-log-core" pointer.
8. `audit_log` + `status` lifecycle + `actor{persona}` (governance/provenance).
9. `computed_views` engine-only — no command writes a number.

## Pressure test — is every feature implementable against the core IA?
**Verdict: yes.** A use-case-falsification review of the contract: **10/11 use cases Supported** — observe, admin declares, driver queries, trace, cross-core routing, optimistic-CAS concurrency, governance-as-MCP-layer, and (given the additions below) deprecate + coveragePolicy + deprecated-elements-excluded-from-findings. The single refutation (`analyze` "member weights missing") was a **false negative from an incomplete subject summary** — `Member` does carry `weight`/`weightObservation` — but it surfaced one genuine refinement: **weighted analyses must precondition-check that members are weighted** (a `validate` rule, not a structure). **Applied:** the two core additions below are now in `crossmatrix.schema.json` (`LifecycleStatus` on Member/Relation + `supersededBy`; `coveragePolicy` on Relation), and the example still validates.

- **Deterministic over the core IA (the analytical surface):** `dimension.register`/`scale.declare`/`relation.declare`/`contraction.declare`/`members.sync`, `observe`/`member.propose`/`evidence.attach`, `gaps.next`/`orphans`/`coverage`/`conflicts`, `slice`/`describe`/`trace`/`explain`, `analyze.*`, `validate`, `export`. All are pure functions over `{dimensions, relations(cells/observations), contractions, scales, findings}`. ✓
- **MCP-layer state, NOT core fields (correct by separation):** `status`/lifecycle, `audit_log`, `actor{persona}` are server/session governance — they belong to the MCP crate, not the pure data model. `stale`'s *fetch+re-hash* is MCP-layer I/O; the *comparison* is deterministic. ✓
- **Two minor core gaps to close for a fully-backed contract:**
  1. **Element-level `status` (`active|deprecated` + `supersededBy`)** — `deprecate` wants append-only supersession; the core currently has none. Add a lightweight optional `status` to Member/Relation (and per-observation), OR record deprecation only in the MCP `audit_log`. (Recommend the optional `status` for auditable, non-destructive retraction.)
  2. **`coveragePolicy` (orphan-as-violation)** — `coverage`/`orphans` as a *report* is computable now; flagging an orphan as a *violation* needs a declared expectation. Add an optional `coveragePolicy` on `Relation` (or a profile). Not a blocker (report works without it).
- **Deferred (not gaps):** `kindProfile.register` (engine defaults until a consumer needs profiles), `analyze.priority` (AHP), `analyze.compare`.

So the contract does not exceed the IA: the analytical/structural surface is fully implementable as deterministic ops over `crossmatrix`; governance/provenance is MCP-layer (as it should be); only `status` + `coveragePolicy` are small, optional core additions.

## Consequences
- Core stays pure + reusable; MCP crate owns the LLM ergonomics + governance.
- Mutation-first + observation-only + driver queries + HATEOAS enforce the LLM/tools separation at the contract boundary.
- Cross-core queries route honestly rather than faking — preserving the federated-cores architecture.

## Follow-ups
- `schemas/crossmatrix-mcp.schema.json` (this ADR's companion): the command/query envelope + op payloads, extending the prior internal envelope to the observation/cell model.
- Add optional `status`/`supersededBy` + `coveragePolicy` to the core schema (ADR 0002 amendment) when scaffolding.
