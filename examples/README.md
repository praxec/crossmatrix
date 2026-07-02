# Worked example — QFD + FMECA in ONE cross-matrix (3 axes)

`qfd-fmeca-3axis.json` is a single `crossmatrix` model with **three weighted dimensions** —
customer requirements × engineering characteristics × FMECA failure modes — proving that
FMECA is carried **as a dimension** (not a separate representation), and that cross-cutting
value × risk questions are **native operations**, not bespoke joins.

It is **sparse**: only the non-empty cells are stored (8 cells across 3 relations), not the
`3 × 4 × 3 = 36`-cell dense product.

**Observation-only:** every LLM-supplied value is a categorical token, never a number —
relation cells carry `observation: "strong"` (resolved to `9` by the `qfd` scale), requirement
importance is `weightObservation: "high"` (resolved by the `importance` scale), and the FMECA
failure-mode criticalities are *imported numbers* carrying a deterministic `sourceRef`
(`system: fmeca`), not LLM guesses. Code does every number→arithmetic step below.

## The fixed operations this model unlocks

**1. `marginalize` — per-characteristic technical importance** (weighted rollup of `rel_req_char`
over requirements, scaled by requirement importance `Σ req.weight × cell`):
- `char_response_latency` = 9×9 = **81**
- `char_encryption`       = 9×9 = **81**
- `char_idempotent_api`   = 9×3 = **27**
- `char_return_workflow`  = 3×9 = **27**
→ latency and encryption are the dominant engineering investments.

**2. `contract` — requirement → failure exposure** (`ctr_req_failure_exposure` = `rel_req_char`
⊗ `rel_char_exposes_fail`, product, weighted by requirement importance). The derived
`requirement × failure` matrix:
- `req_secure_payment` →(encryption)→ `fail_data_breach`: 9×9 ×9 = **highest**
- `req_secure_payment` →(idempotent_api)→ `fail_double_charge`: 3×9 ×9
- `req_fast_checkout`  →(response_latency)→ `fail_latency_spike`: 9×9 ×9
→ **secure_payment carries the most failure exposure** — derived deterministically, never
hand-joined.

**3. intra-axis trade-off as a SELF-relation**: `char_encryption` `opposes` `char_response_latency`
within `dim_char` (a relation with `from == to`) → improving security worsens latency. This is the
former "roof", now just another relation (no dedicated construct).

**4. issue-detection — `critical_exposure`** (the FMECA payoff): `fail_double_charge` has
`fmeca_criticality = 9` and is **exposed** (`rel_char_exposes_fail`) but appears in **no**
`mitigates` relation → emitted as a `block`-severity finding. `fail_data_breach` (also crit 9)
*is* mitigated by encryption → OK. `fail_latency_spike` is unmitigated but low criticality (3)
→ `info`. This "unmitigated high-criticality exposure on a high-value requirement" finding falls
out of the model **by existence + mappings** — no bespoke rule.

**5. `sensitivity` / cut-list**: `req_easy_returns` (importance 3) is the only driver of
`char_return_workflow`; deprioritizing it makes the workflow a low-value candidate for cutting.

## Why this is the "sweet spot"
QFD (steps 1–3) and FMECA (step 4) share **one structure and one operation set**. A second tool
(e.g. a JTBD system) adds a `jtbd_step` dimension + a `rel_jtbd_req` relation, and *the same*
`contract`/`marginalize`/`critical_exposure` operations immediately answer
"which high-importance job steps inherit the most unmitigated failure criticality?" — interop by
shared **operations**, configurability by added **axes**.

## Other examples in this directory

- **`travel-mug-3axis.json`** — the *same* QFD+FMECA structure and operations in an
  unrelated, everyday product domain (a travel mug: customer needs × engineering
  characteristics × failure modes). Demonstrates that the engine is **not** tied to any
  one domain — a new domain is just new axes + cells, same fixed operations.
- **`split/`** — the same e-commerce HoQ model authored as the **three ADR-0004 layers**
  (`hoq.config.json` + `qfd-fmeca.dimensions.json` + `demo.state.json`), linked by stable
  id+version. This is the form the `crossmatrix-mcp` `model.open` resolves and merges into
  a loadable engine model.
