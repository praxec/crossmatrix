---
name: crossmatrix-test-coverage
description: "Trace requirements or journey steps to tests with crossmatrix-mcp: a requirements × tests matrix whose orphans are untested requirements and orphan tests, weighted so the most important untested requirement comes first. Use when the user asks about test coverage of requirements, a traceability matrix, untested features, or which tests to write next."
---

# Requirements / journeys × tests

Read [crossmatrix](../crossmatrix/SKILL.md) first.

## Model

- The WHAT axis: requirements (`dimensionKind: "customer_requirement"`) or journey steps
  (`journey_step`, reused from [crossmatrix-jtbd](../crossmatrix-jtbd/SKILL.md)), weighted
  with `weightObservation`.
- `dimension.register {"dimensionId": "dim_test", "dimensionKind": "test_artifact"}`, then
  `members.sync` the tests from the source: one member per test id, `sourceRef` pointing to
  the file and test name (`{"sourceRef": "tests/checkout.rs", "locator": "fn pays_once"}`)
  plus a `contentHash` of the file. Re-sync after test changes; removed tests are
  deprecated, not deleted.
- `scale.declare` a verification scale, e.g. `none|smoke|partial|full` (0/1/3/9).
- `relation.declare {"relationId": "rel_req_test", "from": "dim_req", "to": "dim_test", "relationType": "verifies", "scale": "verification", "coveragePolicy": "every_from"}`.

## Fill

Read each test, then `observe` how fully it verifies each requirement it touches, with
the assertion lines as evidence. Do not credit a test for a requirement it only executes
without asserting on.

## Read

- `coverage`: with `every_from`, every uncovered requirement is a **violation** (untested).
- `gaps.next`: untested requirements ordered by importance; write tests in that order.
- `gaps.orphans`: tests that verify no requirement (dead or misfiled tests) and
  requirements no test touches.
- `stale {"hashes": {"tests/checkout.rs": "<current hash>"}}`: which recorded test
  evidence changed since it was observed; re-observe those cells.
- `report {"relationId": "rel_req_test"}` for the traceability matrix.

Finish with [crossmatrix-review](../crossmatrix-review/SKILL.md) before a release.
