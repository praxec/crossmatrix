# ADR 0002 — `crossmatrix`: a standalone sparse N-dimensional weighted cross-reference crate

- Status: Proposed
- Date: 2026-06-29
- Related: prior internal design notes (the mutation-first write API this reuses).

## Context
An earlier design placed QFD as one "shape crate" among siblings. Refinement: the highest-leverage, most-reusable capability is a **specialized-but-configurable multi-dimensional weighted cross-reference matrix** (a generalized, N-axis House of Quality) that can carry QFD axes **and** an FMECA axis **in one structure**, so cross-cutting questions (value × characteristic × failure risk) are native operations rather than bespoke joins.

Key realization: this capability's math is **sparse linear algebra** (matrix products = relation composition/contraction; eigenvector = priority; marginalization = rollups), **not graph traversal**. So it stands alone — it does not depend on the graph crate. It is the *linear-algebra view* of cross-references, packaged and sparse-optimized for reuse.

Interop principle (the decisive one): **interoperability = shared structure + shared *operations*.** A maximally-generic graph core gives shared structure but lets every tool invent its own analysis semantics. A specialized cross-matrix with a **fixed operation vocabulary** gives tools shared *operations* — the actual leverage. So we hit the sweet spot: **fix the operations, make the axes/weights/scales/relations configurable.**

## Decision

### Observation-only invariant (no LLM-authored numbers) — ecosystem-wide
The LLM emits **only categorical observation tokens** from a closed, declared vocabulary (+ evidence/rationale); it **never authors a number**. Numbers enter only via (a) a declared deterministic **observation→value table** (`scales`), (b) **measurement** from an authoritative source (`provenanceClass: observed`), or (c) **engine computation**. The engine rejects unknown tokens and any LLM-set numeric field (fail-fast). Nuance: *measured/computed* numbers are fine — the prohibition is on **LLM-generated** numbers; the discriminator is provenance (a numeric `Member.weight` is permitted **only** with a deterministic `sourceRef`, e.g. a tool-computed FMECA criticality). This generalizes a proven pattern (an upstream tool picks severity/probability observations and computes criticality; confidence is derived, not stored), and is the strongest fix for the calibration / fabricated-precision risk. Enforced in-schema: a `Cell` has `observation` (not `weight`).

Corollary: **every scale `Level` carries a `description` rubric anchor.** The LLM chooses an observation from `(token + description)` and never sees the numeric `value` — the descriptions are what calibrate the judgment and keep responses contextualized (strongly recommended on every level).

Build **`crossmatrix`** as a standalone, reusable crate:

- **What it is:** a sparse, N-dimensional, weighted cross-reference model + a fixed operation set. Dimensions are axes (members carry weights); relations are sparse pairwise weighted cells between two axes; roofs are intra-axis correlations; contraction chains declare cascades (e.g. `requirement×characteristic ⊗ characteristic×failure → requirement×failure`).
- **Fixed operations (the shared capability):** `contract` (multilinear cascade over chained relations), `slice` (fix axes → a 2-D HoQ), `marginalize` (weighted rollup over an axis), `priority` (eigenvector/AHP), `roof` (correlation/conflict), `sensitivity` (perturbation), plus deterministic issue-detection (orphans, uncovered targets, unmitigated-critical exposure, conflicts, weak evidence).
- **Configurable, not hardcoded:** the number/identity of axes, their weights/scales/relation-types, and the contraction chains are all declarative. "N-dimensional" is bounded to *weighted cross-reference* — NOT arbitrary semantics.
- **FMECA as a *dimension*, not a separate representation:** failure modes are an axis whose member `weight = criticality`. The FMECA-internal RPN math (severity ⊗ occurrence ⊗ detection) stays **upstream/federated** (in an external FMECA tool) and *feeds the weight*. `requirement × failure` exposure is then **derived by contracting** `requirement×characteristic` with `characteristic×failure` — generalized QFD cascade as multilinear contraction.
- **Stack:** `sprs` (sparse storage + sparse matmul = contraction, minimal size) + `nalgebra` (dense eigen/priority/sensitivity on materialized slices). Both pure-Rust → no system BLAS. Fixed JSON schema → typify types → validating loader → operations (a proven validating-loader pattern).
- **Write API:** reuse the mutation-first design from the prior internal design notes (`propose_dimension` / `propose_node`→member / `propose_relationship`→cell / `propose_correlation`→roof / `propose_constraint`), adding `propose_member`, `propose_cell`, and `propose_contraction`. LLM proposes; engine computes; no LLM-authored scores.

## Why standalone (not a projection of the graph crate)
A pairwise cross-relation *is* a bipartite weighted graph, so it is conceptually a graph projection — but its **characteristic math is multilinear/sparse-matrix**, needing no traversal algorithms. Packaging it standalone (deps: `sprs`+`nalgebra`, not `petgraph`) maximizes reuse for any system wanting weighted cross-reference, without graph baggage. The graph crate (reachability/order/transition) remains its independent dual. This does **not** introduce a new *core structure* — it is the same relational primitive optimized for the linear-algebra view.

## Sparsity / minimal size
- Store only **non-empty cells** as `(from_idx, to_idx, weight)` triplets; intern string ids → `u32` indices. Size ∝ real relations, not `Π|dimensions|`.
- Materialize dense slices on demand for `priority`/`sensitivity`; keep storage + contraction sparse (`sprs` CSR/CSC).

## Dimensionality & forward-compat (N is open; no fixed arity)
- **N-dimensional by construction, not by a fixed tuple.** `dimensions` is arbitrary-length; axes connect via as many pairwise `relations` as needed; `contractions` chain them. Adding a cross-dimension from another consumer (a JTBD `jtbd_step` axis, a new FMECA view, …) is purely additive — register a Dimension + Relations. Nothing assumes 2 or 3 axes.
- **Why pairwise is the default representation:** almost every tie is pairwise-factorable, so `A[i,j] ⊗ B[j,k]` reconstructs the multi-way result by contraction *without materializing a cube* — this is what keeps storage sparse and lets `sprs` do the math.
- **Irreducible N-way ties → arbitrary-arity hyperedges, not a 3-tuple.** When a value depends jointly on 3+ axes and cannot be factored, an optional `hyperedges` collection carries `coords: [{dimension, member}, …]` of **arbitrary length** (scales with however many dimensions exist). It is stored sparsely and sits outside the pairwise contraction fast-path. Dense N-d tensors are deliberately avoided (they'd cost `Π|dims|` memory + a BLAS backend); a dense/`ndarray` decomposition module is added only if CP/Tucker latent-pattern analysis or ML consumption is later required.

## Vetting outcomes (architecture-validity + FMECA, for the multi-domain weighted cross-reference / LLM-MCP use case)

### Component classification (Phase 1 — should each exist for THIS use case?)
| Component | Classification | Verdict |
|---|---|---|
| dimensions / members / relations / cells | Essential (Proven) | keep |
| contraction (chains) | Essential (Proven — QFD cascade) | keep — this *is* "what ties to what" |
| marginalize (weighted rollup) | Essential | keep |
| orphan / uncovered / `critical_exposure` findings | Essential | keep — this *is* "what needs fixing" |
| roof (intra-axis correlation) | Speculative *for this use case* | **demote to optional/later** |
| eigenvector/AHP `priority` | Speculative | **demote to later** (weighted rollup suffices) |
| hyperedges / dense tensors / `sensitivity` | Speculative | keep **dormant** |

Net: the design was slightly over-built on QFD-classic features and under-built on federation + LLM-MCP realities. The rebalance below trades the former for the latter.

### v1 capabilities to ADD (essential; were missing)
- **Observation-only invariant** (done) + **negative observations / bipolar scales** (done) + **confidence propagation**; contraction outputs are **relative rank, not magnitude** (no fabricated precision).
- **Source traceability** (done): structured `Evidence` → `SourceRef` the LLM can re-open; **`contentHash`** = fast non-cryptographic identity+change hash.
- **`stale_reference` finding**: engine re-hashes a source and compares `contentHash`; flags changed/missing references (the on-disk reality — files change under the model).
- **Integrity + fail-fast**: cells must resolve to existing members; contraction chains must be acyclic and scale-compatible; reject with rich diagnostics.
- **Driver queries** `orphans` / `next_unmapped` / `coverage` / `trace`: the engine deterministically picks WHAT to map / surfaces lineage; the LLM only judges. (Closes the "weak tools → LLM overreach" separation-of-concerns gap.)
- **Lineage in findings**: `path` + resolved `sourceRef`s = "what it ties to" (file anchors), so the LLM can act and verify.
- **Require evidence for `inferred`** observations (poka-yoke; heuristics need evidence); cross-source corroboration raises trust; route mappings feeding `block` findings to a human/cross-check gate.

### DEFER (over-engineering for v1)
roof, AHP/eigenvector priority, hyperedges, dense tensors, sensitivity.

### MUST-KEEP (oversimplification guardrails — your named failure mode)
provenance+evidence per observation; source versioning (`contentHash`) + staleness; the federation boundary (do **not** collapse into one owned graph); contraction lineage. Cutting any of these makes the tool fast to build and **untrustworthy to use** — a confident liar for the LLM+user+code loop.

### Separation-of-concerns verdict
Boundary is correct (engine computes; LLM observes/interprets) and now *enforced*: the observation-only invariant removes LLM numbers, and the driver queries + fail-fast diagnostics keep the LLM in the reasoning lane while code owns identity, arithmetic, integrity, and staleness.

## DimensionKind as a deterministic dispatch discriminator
`DimensionKind` is not a cosmetic label — it is a **typed discriminator the engine codes real features against** (a typed node-kind discriminator). It binds a **declared kind-capability profile**: weight-source policy (e.g. `failure_mode` ⇒ criticality imported with a deterministic `sourceRef`, never `weightObservation`), valid relations/scales (e.g. `code_element →(dependsOn, self)→ code_element`), **sibling-core routing** (a `code_element` self-relation's SCC/cycles → the graph core; temporal → the event-log core), and default profile/findings.
- **Guardrail:** kind drives **affordances / validation / routing — never domain ALGORITHMS** (no FMECA RPN, no UX reachability in this engine; those stay federated).
- **Declarative, not hardcoded:** kind→capability bindings are a profile a new consumer can register — preserving "fix the operations, configure the axes." (Validated by the NDepend DSM test: `code_element` + `dependsOn` self-relation represents the matrix + coupling rollups; its cyclic analysis routes to the graph core.)

## Three-lens vet outcome (architecture-validity + FMECA + falsification, on the remediated design)
- **Architecture-validity (Phase 1): APPROVED.** The 5 core components (Dimensions, Relations, Contractions, Findings, Scales) are Essential/Proven. **Roofs + Hyperedges classified Unjustified → REMOVED** (not merely deferred): a Roof is just a **self-relation** (`from == to`); a Hyperedge is an inert, out-of-scope N-way stub. Removing both loses no capability and cuts dead weight.
- **FMECA (re-run): the original 5 design risks are resolved.** Residuals are now implementation-discipline (re-hash I/O robustness, fail-fast diagnostic granularity, acyclic-check correctness, federation source availability) — core-crate tests/error-handling, not schema changes.
- **Falsification (re-run): 8 of 9 contradictions now SURVIVE.** Only 3 still refute — `temporal-evolution`, `feedback-loop`, `irreducible-N-way` — all **explicit out-of-scope boundaries** that route to the event-log / graph cores, independently re-validating the 2-core decomposition.

## Remediations from the design review (poka-yoke + TRIZ)
Run via a structured FMECA review (5 risks) + a use-case-falsification review (11 contradictory use cases). Applied:
- **Bipolar → valence-by-separation (TRIZ separation).** Removed the bipolar/signed scale (it let strong-for + strong-against cancel → `uc-net-zero-masks-conflict`). Support and opposition are now **separate unipolar relations** (`relationType`: e.g. `satisfies`/`supports` vs `undermines`/`opposes`/`blocks`); engine reports **net AND tension** — cancellation is impossible by construction. Cleans contraction (per-valence, no sign-mixing) and reuses the existing multi-relation-per-pair capability.
- **Multi-observation cells (poka-yoke by structure).** A `Cell` holds `observations[]` (source-stamped) — no single slot to collapse — so source disagreement is preserved (`conflicting-observations-collapse`, `uc-source-disagreement`); engine computes consensus + a `tension`/`conflict` finding.
- **Continuous measured metric (TRIZ separation-by-source).** Scales gain `mode: measured`; a `measuredValue` is permitted only with `provenanceClass: observed` + `sourceRef`, and **code buckets it** via Level `range`s — observation-only intact (`uc-continuous-metric`, FMECA `input-type-mismatch`).
- **Confidence in rank.** Rank lexicographically within confidence bands; never collapse to a magnitude (FMECA `contraction-rank-misrepresented`, `uc-confidence-vs-strength`). Findings carry `net`+`tension`+`confidence`, not a raw product.
- **Sensitivity un-deferred (it was free).** Derived findings now carry `contributors[]` (ranked top drivers) — resolving the `uc-sensitivity-analysis-deferred` contradiction with the "what needs fixing" purpose.
- **Hyperedge honesty (poka-yoke vs false capability).** Explicitly labeled **stored, not computed (v1)** (`uc-irreducible-4way`).
- **Staleness as active re-hash.** `contentHash` must be re-hashed on read, not trusted (FMECA `stale-source-reference-undetected`); `stale_reference` finding.
- **Recoverable rejection, not halt.** Fail-fast rejects a bad mutation with diagnostics and lets the LLM retry — never halts the pipeline (FMECA `llm-emits-numeric-instead-of-token`).

Guidance-only (representable; documented patterns): **conditional strength = add a `context` dimension** (not a cell modifier; `uc-conditional-strength`); **cross-axis correlation = a relation, not a roof** (`uc-roof-inter-axis`); dense-density = ops guard (`uc-sparse-dense-breakdown`).

Correctly OUT OF SCOPE — validates the 2-core split: **temporal trend** (`uc-temporal-evolution`) → the event-log core; **feedback loops/cycles** (`uc-feedback-loop`) → the graph core. The falsifier independently rediscovered the boundary.

## Alternatives considered
1. **Two separate 2-D configs (QFD + FMECA) over a generic core** — rejected: cross-cutting (value×characteristic×failure) becomes bespoke multi-matrix joins; tools share structure but not operations → weaker interop.
2. **Generic graph core only** — rejected: powerful multilinear operations aren't first-class, so each consumer reinvents analysis semantics.
3. **Dense N-d tensor** — rejected: cross-reference data is sparse; dense wastes space and forces a BLAS backend.
4. **Fold into the graph crate as a projection module** — rejected for *packaging*: it would drag `petgraph` into every consumer; standalone maximizes reuse. (Still a projection *conceptually*.)

## Consequences
- (+) One structure expresses QFD + FMECA + arbitrary weighted axes; cross-cutting analyses are native contractions; shared operations ⇒ real interop.
- (+) Sparse + pure-Rust stack ⇒ small footprint, low build friction, reusable anywhere.
- (+) Federation preserved (FMECA RPN stays upstream; only criticality weights flow in).
- (−) A bigger engine than thin configs — justified because multi-way value×risk×coverage is the actual goal and is reused across multiple weighted domains.
- (−) Genuinely-irreducible 3-way ties need a hyperedge; default is pairwise + contraction (kept sparse/tractable).

## Follow-ups
- `schemas/crossmatrix.schema.json` (canonical sparse model) + `examples/qfd-fmeca-3axis.*` (worked proof).
- Scaffold the crate (`sprs`+`nalgebra`): schema→typify types→validating loader→operations.
- Earlier framing amended: "QFD/FMECA = configs" becomes "QFD/FMECA = axes+relations in a `crossmatrix` model"; the QFD "shape crate" is now this standalone crate.
