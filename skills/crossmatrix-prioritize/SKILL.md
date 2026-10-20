---
name: crossmatrix-prioritize
description: "Use when the user asks what to build first, wants a value-weighted backlog, or needs a priority order derived from needs, gaps, risks or competitive gaps rather than gut feel. Rank deliverables against weighted value drivers with crossmatrix-mcp and hand the ranked list to a planner such as cpm-planner."
---

# Deliverables × value drivers

Read [crossmatrix](../crossmatrix/SKILL.md) first.

Calling the tools: see [Calling the tools](../crossmatrix/SKILL.md#calling-the-tools)
(registered MCP server or a host wrapper such as `junior tools call`).

## Model

- `dimension.register {"dimensionId": "dim_value", "dimensionKind": "business_metric", "weightScale": "importance"}`:
  the value drivers (customer needs, gaps worth winning, risks to retire, revenue or cost
  goals), each with a `weightObservation` and a `sourceRef` to where its importance comes
  from. Reuse members from other crossmatrix models where they exist.
- `dimension.register {"dimensionId": "dim_deliverable", "dimensionKind": "deliverable"}`
  and `members.sync` the candidate deliverables (epics, features, fixes). Put effort or
  duration hints in `attributes`; the engine does not use them, the planner does.
- `scale.declare` a contribution scale (`none|weak|moderate|strong`, 0/1/3/9).
- `relation.declare {"relationId": "rel_value_deliverable", "from": "dim_value", "to": "dim_deliverable", "relationType": "satisfies", "scale": "contribution", "coveragePolicy": "every_from"}`.
- If a deliverable works against a driver (it adds risk, slows a goal), record that in a
  separate `opposes` relation. Never lower the support token instead.

## Fill

Use the driver loop: `gaps.next`, then observe the contribution with rationale.

## Rank and hand off

1. `analyze.marginalize {"relationId": "rel_value_deliverable", "axis": "from"}`: the
   ranked deliverables (value-weighted score, highest first).
2. `conflicts`: deliverables with strong opposition; decide or split them before
   planning.
3. `coverage`: value drivers no deliverable serves (`every_from` makes them violations).
4. Hand the ranked list to the planner. For cpm-planner, keep the order as the priority,
   carry each deliverable's id, and add durations and dependencies there. Do not invent
   numeric scores for the planner; pass the rank and the engine's score.

Record the hand-off in the report: `report {"relationId": "rel_value_deliverable"}`.
