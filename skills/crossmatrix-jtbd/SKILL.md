---
name: crossmatrix-jtbd
description: "Map jobs to be done against user segments and journey steps with crossmatrix-mcp, then contract the axes to see which journey steps carry the most weighted job importance per segment. Use when the user wants a JTBD map, a jobs-by-segment matrix, journey coverage of jobs, or to find under-served jobs and journey steps."
---

# Jobs to be done × segments × journeys

Read [crossmatrix](../crossmatrix/SKILL.md) first. This is a three-axis model joined by a
contraction.

## Axes

- `dimension.register {"dimensionId": "dim_segment", "dimensionKind": "user_segment", "weightScale": "importance"}`:
  segments weighted by business importance (`weightObservation`), each with a `sourceRef`
  to the research that defines it.
- `dimension.register {"dimensionId": "dim_job", "dimensionKind": "jtbd_step"}`: jobs
  phrased as "When <situation>, I want to <motivation>, so I can <outcome>" in the
  `label`.
- `dimension.register {"dimensionId": "dim_journey", "dimensionKind": "journey_step"}`:
  the product's journey steps, in order (`attributes.order`).

## Relations

- `rel_segment_job` (`dim_segment` → `dim_job`, `relationType: "supports"`, scale
  `importance-of-job`): how much this segment needs this job done. Declare a scale such as
  `none|nice|important|critical` (0/1/3/9) with rubric descriptions.
- `rel_job_journey` (`dim_job` → `dim_journey`, `relationType: "satisfies"`, scale `qfd`,
  `coveragePolicy: "every_from"`): which journey steps get the job done.
- `contraction.declare {"contractionId": "ctr_segment_journey", "chain": ["rel_segment_job", "rel_job_journey"], "weightCombination": "product"}`.

## Fill

Use the driver loop: `gaps.next`, then observe from interview notes or analytics, with
evidence. Leave a cell empty rather than guessing `none`.

## Read

- `analyze.contract`: segment → journey-step findings, each with the path through the
  jobs that carries the weight. The top rows are the steps to invest in for each segment.
- `analyze.marginalize {"relationId": "rel_job_journey", "axis": "from"}`: journey steps
  ranked by how much job importance they serve.
- `coverage` / `gaps.orphans`: jobs that no journey step serves are the under-served jobs.
- `report {"relationId": "rel_job_journey"}` for the Markdown matrix.

Pairs with [crossmatrix-test-coverage](../crossmatrix-test-coverage/SKILL.md), which reuses
the journey steps, and with [crossmatrix-prioritize](../crossmatrix-prioritize/SKILL.md).
