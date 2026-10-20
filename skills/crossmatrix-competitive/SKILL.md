---
name: crossmatrix-competitive
description: "Use when the user asks for competitive benchmarking, a competitor matrix, a QFD planning matrix, or where to differentiate. Benchmark customer needs against competitors (including your own product) with crossmatrix-mcp and find the gaps worth winning: high-importance needs every competitor serves weakly."
---

# Needs × competitors

Read [crossmatrix](../crossmatrix/SKILL.md) first.

Calling the tools: see [Calling the tools](../crossmatrix/SKILL.md#calling-the-tools)
(registered MCP server or a host wrapper such as `junior tools call`).

## Model

- Reuse the weighted needs from a House of Quality ([crossmatrix-hoq](../crossmatrix-hoq/SKILL.md))
  or register `dim_need` with `weightScale: "importance"`.
- `dimension.register {"dimensionId": "dim_competitor", "dimensionKind": "competitor"}` and
  `members.sync` the competitors **and your own product** (`attributes.self: true`). Give
  each a `sourceRef` to the page, review or teardown you assessed.
- `scale.declare` a performance scale, e.g. `absent|poor|fair|good|best` (0/1/3/6/9), with
  rubric descriptions a second reviewer could apply the same way.
- `relation.declare {"relationId": "rel_need_competitor", "from": "dim_need", "to": "dim_competitor", "relationType": "satisfies", "scale": "performance", "coveragePolicy": "both"}`.

## Fill

For each need and competitor, `observe` the performance token with evidence (screenshot
path, review URL, test log) and `provenanceClass: "observed"` when you saw it yourself,
`inferred` when you deduced it. Use `gaps.next` to keep the work in importance order.

## Read

- `slice {"relationId": "rel_need_competitor", "from": "<need>"}`: one need across all
  competitors.
- `analyze.marginalize {"relationId": "rel_need_competitor", "axis": "from"}`: competitors
  ranked by importance-weighted performance (your product's rank is your position).
- Gaps worth winning: needs with high importance where **no** competitor is above `fair`.
  Read them from `report {"relationId": "rel_need_competitor"}` (weights and grid in one
  place) and list them with their evidence. Gaps where only you are weak are catch-up work.
- `coverage` shows needs or competitors you have not assessed yet.

Feed the gaps worth winning into [crossmatrix-prioritize](../crossmatrix-prioritize/SKILL.md)
as value drivers.
