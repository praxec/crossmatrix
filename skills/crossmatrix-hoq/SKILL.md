---
name: crossmatrix-hoq
description: "Use when the user asks for a House of Quality, QFD, needs-to-characteristics matrix, or which engineering characteristics matter most. Build a classic House of Quality (QFD) with crossmatrix-mcp: weighted customer needs (WHATs) against engineering characteristics (HOWs), the correlation roof between characteristics, technical-importance ranking and targets."
---

# House of Quality with crossmatrix

Read [crossmatrix](../crossmatrix/SKILL.md) first for the envelope, observation-only rules
and the driver loop. Every op below is a `crossmatrix.command` `op`.

Calling the tools: see [Calling the tools](../crossmatrix/SKILL.md#calling-the-tools)
(registered MCP server or a host wrapper such as `junior tools call`).

## 1. Frame

- `model.open {"stateId": "<product>-hoq", "aspect": "hoq", "title": "<product> House of Quality"}`
- `scale.declare` the relationship scale (classic 0/1/3/9):

```json
{"kind": "scale.declare", "scale": {"id": "qfd", "levels": [
  {"observation": "none", "value": 0, "description": "No relationship."},
  {"observation": "weak", "value": 1, "description": "Indirect or minor effect."},
  {"observation": "moderate", "value": 3, "description": "Contributes, not the main lever."},
  {"observation": "strong", "value": 9, "description": "Primary mechanism."}]}}
```

- `scale.declare` an importance scale (`low` 1, `medium` 3, `high` 9) with rubric
  descriptions the customer evidence can be checked against.

## 2. Rooms

- `dimension.register {"dimensionId": "dim_need", "dimensionKind": "customer_requirement", "weightScale": "importance"}`
- `members.sync` the needs in the customer's words, each with `weightObservation` and a
  `sourceRef` to the interview, survey or ticket it came from.
- `dimension.register {"dimensionId": "dim_char", "dimensionKind": "engineering_characteristic"}`
- `members.sync` measurable characteristics. Put the target and unit in `attributes`
  (`{"target": "< 200", "unit": "ms"}`); the report prints a Targets table from them.

## 3. Relations

- `relation.declare {"relationId": "rel_need_char", "from": "dim_need", "to": "dim_char", "relationType": "satisfies", "scale": "qfd", "coveragePolicy": "every_from"}`
  (every need must be served by something).
- The roof is a self-relation on the characteristics. Use `supports` for synergies and
  `opposes` for trade-offs, as two separate relations:
  `relation.declare {"relationId": "roof_conflict", "from": "dim_char", "to": "dim_char", "relationType": "opposes", "scale": "qfd"}`.

## 4. Fill (driver loop)

`gaps.next` → `observe` one cell with rationale and evidence → repeat. Observe the roof
only for pairs where a real trade-off or synergy exists. Do not fill `none` everywhere.

## 5. Read the house

- Technical importance: `analyze.marginalize {"relationId": "rel_need_char", "axis": "from"}`
  rolls the needs up into a ranked list of characteristics (weight × strength).
- Trade-offs: `conflicts` lists characteristics where opposition meets or beats support.
- Gaps: `coverage` with the `every_from` policy flags needs no characteristic serves.
- The full house: `report {"relationId": "rel_need_char"}` returns Markdown with weights,
  the matrix, the roof, technical importance, targets, coverage and findings.

Hand the ranked characteristics to [crossmatrix-prioritize](../crossmatrix-prioritize/SKILL.md)
or to a planner, and finish with [crossmatrix-review](../crossmatrix-review/SKILL.md).
