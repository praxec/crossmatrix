---
name: crossmatrix-review
description: "Review and sign off a crossmatrix model before it is relied on: run crossmatrix-mcp check, validate, conflicts, stale and coverage, produce the Markdown report, and walk a sign-off checklist. Use when the user asks to review, audit, validate or sign off a House of Quality or any crossmatrix model, or before committing .crossmatrix/ changes."
---

# Review and sign-off

Read [crossmatrix](../crossmatrix/SKILL.md) first. Review is read-only: it never writes
except to record fixes the reviewer approves.

## 1. Integrity

- `crossmatrix-mcp check` at the repo root: exit 0, no diagnostics. Fix any path it names.
- `model.open {"stateId": "<id>"}`, then `model.status`: right `root` (the repo's
  `.crossmatrix/`), expected version, counts.
- `validate` returns `validated: true`.

## 2. Evidence

- `stale {"hashes": {...}}` with the current content hashes of every source you can
  re-hash: each stale entry is a cell or weight whose source changed. Re-observe or
  deprecate it.
- `slice` the most important rows and confirm each `inferred` observation has evidence
  that still says what the rationale claims.

## 3. Findings

- `conflicts`: every `conflict` (opposition ≥ support) needs a decision recorded in the
  report or a design change.
- `analyze.findings`: read the contributors of each finding; they name the cells that
  drive it.
- `coverage` / `gaps.orphans`: violations of a `coveragePolicy` must be closed or the
  policy consciously relaxed.

## 4. Report

`report {}` (whole aspect) or `report {"relationId": "..."}`. Attach it to the PR or the
review record. `export {"format": "model"}` gives the merged JSON for another tool.

## Sign-off checklist

- [ ] `crossmatrix-mcp check` passes and `.crossmatrix/` is committed.
- [ ] Every weighted dimension has a weight on every active member, from a token or a
      deterministic import.
- [ ] No unresolved `conflict` findings, or each has a recorded decision.
- [ ] No coverage violations, or each is accepted in writing.
- [ ] No stale references.
- [ ] Every `inferred` observation has evidence.
- [ ] The top of the technical-importance or priority ranking matches the team's
      understanding, or the disagreement is explained.
- [ ] The report is attached to the review.
