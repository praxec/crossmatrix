---
name: crossmatrix-risk
description: "Cross failure modes against components or requirements with crossmatrix-mcp: import FMECA criticality as deterministic weights, separate exposes from mitigates, and find unmitigated critical exposure. Pairs with fmeca-mcp. Use when the user wants a risk × component matrix, to tie an FMECA to the design, or to see which requirements carry the most failure exposure."
---

# Failure modes × components / requirements

Read [crossmatrix](../crossmatrix/SKILL.md) first. Criticality comes from an FMECA (for
example fmeca-mcp `report.export`), never from you.

## Model

- `dimension.register {"dimensionId": "dim_fail", "dimensionKind": "failure_mode", "system": "fmeca"}`.
- `members.sync` the failure modes with their **imported** criticality as `weight` and a
  deterministic `sourceRef` (`{"system": "fmeca", "sourceRef": "<session>/<fm id>", "contentHash": "..."}`).
  A numeric weight without `sourceRef.system` is rejected (`observation_only`).
- The design axis: components (`implementation_asset`) or requirements
  (`customer_requirement`), synced from the repo or the requirements doc.
- Two relations, never one signed scale:
  - `rel_comp_exposes_fail` (`relationType: "exposes"`): the component can produce the
    failure mode.
  - `rel_comp_mitigates_fail` (`relationType: "mitigates"`): the component prevents or
    detects it.
- Optional chain to requirements: `contraction.declare {"contractionId": "ctr_req_exposure", "chain": ["rel_req_comp", "rel_comp_exposes_fail"]}`.

## Fill

Observe exposure and mitigation separately, with evidence (code path, design doc, test).
Re-sync `dim_fail` whenever the FMECA changes; `stale` with the new hashes shows which
cells rest on outdated failure modes.

## Read

- `analyze.contract`: requirement → failure exposure, with the components on the path.
- `conflicts` / `analyze.findings`: failure modes where exposure meets or exceeds
  mitigation (`conflict`, severity `block`), most critical first.
- `analyze.marginalize {"relationId": "rel_comp_exposes_fail", "axis": "to"}`: components
  ranked by criticality-weighted exposure.
- Send unmitigated critical exposures back to fmeca-mcp as actions, and their components
  to [crossmatrix-prioritize](../crossmatrix-prioritize/SKILL.md).
